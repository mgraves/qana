//! SYNTACTIC CLASSES: a token painted by where it stands in the tree.
//!
//! A token's own `@style` paints every token of its kind alike. What a
//! name is depends on where it stands: `Vec` in `x: Vec<u8>` is a type,
//! `push` in `v.push(1)` a method, `println` before `!` a macro. An
//! alternative says so with `@style(class, label…)`, and the class is
//! read off the tree:
//!
//!  * A token at a labelled position takes the class.
//!  * A rule at a labelled position is GIVEN the class, and passes it on
//!    where its own alternative says, `@style(inherit, label…)`: a path
//!    passes what it is given to the segment that names it, a name to its
//!    identifier. A rule whose alternative passes nothing on paints
//!    nothing with what it was given. (The grammar compiler refuses a
//!    class given to a rule that never passes one on.)
//!  * A list or an option passes what it is given to each element. An
//!    error node (what recovery set aside) gives nothing.
//!
//! A token is painted by its parent's alternative alone, so no two
//! classes ever meet on one token; where the syntax gives one, it is the
//! token's class, over its kind's. Symbol positions count every symbol
//! of the alternative, a token recovery inserted (which has no text, so
//! nothing to paint) among them.
//!
//! KEPT ACROSS EDITS. What a subtree paints depends on nothing outside it
//! but the class it is given. The incremental parser hands back the same
//! subtrees, by pointer, for what an edit did not rebuild, so a
//! [`SyntaxLines`] keeps what each subtree painted, by subtree, as the
//! structure of a live document is kept (`qana_lang`'s `structure`): a
//! subtree up to [`PIECE_BYTES`] is read whole, a larger one child by
//! child, and one that is the same allocation, given the same class, is
//! not read at all. A line that holds no token read again paints as it
//! did (its tokens are all in subtrees painted as before); the lines that
//! do are painted again from the subtrees on them and compared.

use crate::Styles;
use qana_grammar::green::{is_seq_prod, GreenChild, GreenNode, ERROR_NT, NEWLINE};
use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};
use std::sync::Arc;

/// A subtree up to this many bytes is read whole; a larger one, child by
/// child (the size `qana_lang`'s structure keeps by, for the same
/// reason: reading one again is cheap next to a keystroke, and a document
/// of megabytes is a few thousand of them).
pub const PIECE_BYTES: u32 = 4096;

/// Hashes the address a subtree is kept by. Addresses are distinct and
/// aligned, so one multiplication spreads them over the table; a keyed
/// hash buys nothing here.
#[derive(Default)]
struct AddressHasher(u64);

impl Hasher for AddressHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0.rotate_left(8) ^ b as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        }
    }

    fn write_usize(&mut self, address: usize) {
        self.0 = (address as u64)
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .rotate_right(29);
    }
}

type ByAddress = HashMap<usize, Kept, BuildHasherDefault<AddressHasher>>;

/// A place in a document, or in a subtree: the line breaks before it,
/// and the bytes since the last one (since the start, before the first).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct At {
    line: u32,
    col: u32,
}

impl At {
    /// The place `rel` (relative to a subtree) is at, where the subtree
    /// starts at `self`.
    fn plus(self, rel: At) -> At {
        if rel.line == 0 {
            At {
                line: self.line,
                col: self.col + rel.col,
            }
        } else {
            At {
                line: self.line + rel.line,
                col: rel.col,
            }
        }
    }

    /// Past a token of `len` bytes, or a line break.
    fn step(&mut self, id: qana_grammar::TokenId, len: u32) {
        if id == NEWLINE {
            self.line += 1;
            self.col = 0;
        } else {
            self.col += len;
        }
    }
}

/// A token a syntax paints: where it is, its length, its class.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Painted {
    at: At,
    len: u32,
    class: u8,
}

/// A subtree kept on its own, where it starts within its parent.
#[derive(Clone, Copy, Debug)]
struct Child {
    at: At,
    key: usize,
}

/// A subtree as a revision read it.
#[derive(Clone)]
struct Kept {
    /// The subtree: identity and keepalive in one (while it is held, its
    /// address cannot be handed to another subtree).
    node: Arc<GreenNode>,
    /// The class it was given.
    given: Option<u8>,
    /// Where it ends, relative to its start.
    end: At,
    /// What it painted outside the children kept on their own, relative
    /// to its start, in order.
    own: Box<[Painted]>,
    /// Its children kept on their own (a subtree read child by child), in
    /// order.
    children: Box<[Child]>,
}

/// The class each child of `n` is given when `n` is given `given`, in
/// order; `None` for trivia and for a child given nothing.
fn for_children<'n>(
    styles: &Styles,
    n: &'n GreenNode,
    given: Option<u8>,
    mut f: impl FnMut(&'n GreenChild, Option<u8>),
) {
    if n.nt == ERROR_NT {
        for c in &n.children {
            f(c, None);
        }
    } else if is_seq_prod(n.prod) {
        // A list's elements are rules (separators are tokens): each is
        // given what the list is.
        for c in &n.children {
            match c {
                GreenChild::Node(m) if m.nt != ERROR_NT => f(c, given),
                _ => f(c, None),
            }
        }
    } else {
        let mut position = 0usize;
        for c in &n.children {
            let symbol = match c {
                GreenChild::Token(t) => !t.trivia,
                GreenChild::Node(m) => m.nt != ERROR_NT,
            };
            if symbol {
                f(c, styles.route(n.prod, position, given));
                position += 1;
            } else {
                f(c, None);
            }
        }
    }
}

/// Read `n` whole, given `given`: what it paints, at places relative to
/// where `at` started.
fn read_whole(
    styles: &Styles,
    n: &GreenNode,
    given: Option<u8>,
    at: &mut At,
    out: &mut Vec<Painted>,
) {
    for_children(styles, n, given, |c, class| match c {
        GreenChild::Node(m) => read_whole(styles, m, class, at, out),
        GreenChild::Token(t) => {
            let len = t.text.len() as u32;
            if let (Some(class), true) = (class, len > 0) {
                out.push(Painted {
                    at: *at,
                    len,
                    class,
                });
            }
            at.step(t.id, len);
        }
    });
}

/// What a revision's walk found: the tokens it read again, painted, and
/// the subtrees it kept as they were.
struct Walked {
    /// Painted tokens read again, at their places in the document, in
    /// order.
    fresh: Vec<Painted>,
    /// The lines holding a token read again, ascending, each once.
    lines: Vec<u32>,
    /// The subtrees kept as they were, where they now start, in order.
    kept: Vec<Child>,
    /// The bytes read again: of the subtrees read whole, and of the
    /// tokens of those read child by child.
    read: usize,
}

struct Build<'a> {
    styles: &'a Styles,
    old: ByAddress,
    kept: ByAddress,
    /// Where the walk is in the document.
    at: At,
    walked: Walked,
}

impl Build<'_> {
    fn mark_line(&mut self, line: u32) {
        if self.walked.lines.last() != Some(&line) {
            debug_assert!(self.walked.lines.last().is_none_or(|&l| l < line));
            self.walked.lines.push(line);
        }
    }

    /// Read `node`, given `given`, where the walk is; returns its key.
    fn subtree(&mut self, node: &Arc<GreenNode>, given: Option<u8>) -> usize {
        let key = Arc::as_ptr(node) as usize;
        let start = self.at;
        if let Some(k) = self.old.remove(&key) {
            if Arc::ptr_eq(&k.node, node) && k.given == given {
                // The same subtree, given the same class: as it was.
                for c in k.children.iter() {
                    self.carry(c.key);
                }
                self.at = start.plus(k.end);
                self.walked.kept.push(Child { at: start, key });
                self.kept.insert(key, k);
                return key;
            }
        }
        let whole = node.width <= PIECE_BYTES
            || !node
                .children
                .iter()
                .any(|c| matches!(c, GreenChild::Node(_)));
        let kept = if whole {
            let mut rel = At::default();
            let mut own = Vec::new();
            read_whole(self.styles, node, given, &mut rel, &mut own);
            self.walked.read += node.width as usize;
            for line in start.line..=start.line + rel.line {
                self.mark_line(line);
            }
            self.walked.fresh.extend(own.iter().map(|p| Painted {
                at: start.plus(p.at),
                ..*p
            }));
            self.at = start.plus(rel);
            Kept {
                node: node.clone(),
                given,
                end: rel,
                own: own.into(),
                children: Box::default(),
            }
        } else {
            let mut own: Vec<Painted> = Vec::new();
            let mut children: Vec<Child> = Vec::new();
            let relative = |at: At| -> At {
                if at.line == start.line {
                    At {
                        line: 0,
                        col: at.col - start.col,
                    }
                } else {
                    At {
                        line: at.line - start.line,
                        col: at.col,
                    }
                }
            };
            let mut pieces: Vec<(&GreenChild, Option<u8>)> =
                Vec::with_capacity(node.children.len());
            for_children(self.styles, node, given, |c, class| pieces.push((c, class)));
            for (c, class) in pieces {
                match c {
                    GreenChild::Node(m) => {
                        let at = relative(self.at);
                        let key = self.subtree(m, class);
                        children.push(Child { at, key });
                    }
                    GreenChild::Token(t) => {
                        let len = t.text.len() as u32;
                        self.walked.read += len as usize;
                        if !t.trivia && len > 0 {
                            self.mark_line(self.at.line);
                            if let Some(class) = class {
                                let p = Painted {
                                    at: self.at,
                                    len,
                                    class,
                                };
                                self.walked.fresh.push(p);
                                own.push(Painted {
                                    at: relative(self.at),
                                    ..p
                                });
                            }
                        }
                        self.at.step(t.id, len);
                    }
                }
            }
            Kept {
                node: node.clone(),
                given,
                end: relative(self.at),
                own: own.into(),
                children: children.into(),
            }
        };
        self.kept.insert(key, kept);
        key
    }

    /// Keep the kept subtree `key` of a subtree kept as it was, and its
    /// kept children.
    fn carry(&mut self, key: usize) {
        let Some(k) = self.old.remove(&key) else {
            return;
        };
        for c in k.children.iter() {
            self.carry(c.key);
        }
        self.kept.insert(key, k);
    }
}

/// The subtrees of a document's last revision, kept with what each
/// painted.
#[derive(Clone, Default)]
struct Keeper {
    kept: ByAddress,
}

impl Keeper {
    /// Walk `tree`: read again what is not kept, and keep the rest.
    fn walk(&mut self, tree: &Arc<GreenNode>, styles: &Styles) -> Walked {
        let old = std::mem::take(&mut self.kept);
        let mut b = Build {
            styles,
            kept: ByAddress::with_capacity_and_hasher(old.len(), Default::default()),
            old,
            at: At::default(),
            walked: Walked {
                fresh: Vec::new(),
                lines: Vec::new(),
                kept: Vec::new(),
                read: 0,
            },
        };
        b.subtree(tree, None);
        self.kept = b.kept;
        b.walked
    }

    /// What the kept subtree `key`, starting at `start`, paints on `line`,
    /// as (column, length, class).
    fn paint_on(&self, key: usize, start: At, line: u32, out: &mut Vec<(u32, u32, u8)>) {
        let k = &self.kept[&key];
        if line < start.line || line > start.line + k.end.line {
            return;
        }
        let rel = line - start.line;
        let from = k.own.partition_point(|p| p.at.line < rel);
        for p in k.own[from..].iter().take_while(|p| p.at.line == rel) {
            out.push((start.plus(p.at).col, p.len, p.class));
        }
        // The children on the line: those starting on it or before it,
        // back to the first that reaches it.
        let upto = k.children.partition_point(|c| c.at.line <= rel);
        for c in k.children[..upto].iter().rev() {
            let child_end = c.at.line + self.kept[&c.key].end.line;
            if child_end < rel {
                break;
            }
            self.paint_on(c.key, start.plus(c.at), line, out);
        }
    }
}

/// What a document's syntax paints, line by line, kept across edits.
#[derive(Clone)]
pub(crate) struct SyntaxLines {
    keeper: Keeper,
    /// By line: (column, length, class) of each token the syntax paints,
    /// in order.
    lines: Vec<Box<[(u32, u32, u8)]>>,
    /// How many bytes the last revision read again.
    read: usize,
}

impl std::fmt::Debug for SyntaxLines {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SyntaxLines")
            .field("lines", &self.lines.len())
            .field("kept", &self.keeper.kept.len())
            .finish()
    }
}

/// Two are equal when they paint the same.
impl PartialEq for SyntaxLines {
    fn eq(&self, other: &Self) -> bool {
        self.lines == other.lines
    }
}

impl Eq for SyntaxLines {}

impl SyntaxLines {
    /// Paint `tree`, a document of `lines` lines.
    pub(crate) fn new(tree: &Arc<GreenNode>, styles: &Styles, lines: usize) -> SyntaxLines {
        let mut keeper = Keeper::default();
        let walked = keeper.walk(tree, styles);
        let mut by_line: Vec<Vec<(u32, u32, u8)>> = vec![Vec::new(); lines];
        for p in &walked.fresh {
            by_line[p.at.line as usize].push((p.at.col, p.len, p.class));
        }
        SyntaxLines {
            keeper,
            lines: by_line.into_iter().map(Vec::into_boxed_slice).collect(),
            read: walked.read,
        }
    }

    /// How many bytes of the document the last revision read again: what
    /// keeping the rest saved is measured by.
    pub(crate) fn read(&self) -> usize {
        self.read
    }

    /// What the syntax paints on line `line`, as (column, length, class).
    pub(crate) fn line(&self, line: usize) -> &[(u32, u32, u8)] {
        &self.lines[line]
    }

    /// Bring the classes to `tree`, after an edit whose new lines
    /// `[new_lo, new_hi)` replaced the old lines `[old_lo, old_hi)`
    /// (`window`; `None` when no line was replaced). The lines of the
    /// window are painted again; the lines outside it whose classes
    /// changed are returned, ascending (new line numbers).
    pub(crate) fn update(
        &mut self,
        tree: &Arc<GreenNode>,
        styles: &Styles,
        window: Option<(usize, usize, usize, usize)>,
    ) -> Vec<usize> {
        let walked = self.keeper.walk(tree, styles);
        self.read = walked.read;
        let (new_lo, new_hi) = match window {
            Some((old_lo, old_hi, new_lo, new_hi)) => {
                self.lines
                    .splice(old_lo..old_hi, (new_lo..new_hi).map(|_| Box::default()));
                for line in new_lo..new_hi {
                    self.lines[line] = self.paint_line(&walked, line as u32);
                }
                (new_lo, new_hi)
            }
            None => (0, 0),
        };
        debug_assert_eq!(
            self.lines.len(),
            self.keeper.kept[&(Arc::as_ptr(tree) as usize)].end.line as usize + 1,
            "a line for each of the tree's"
        );
        let mut changed = Vec::new();
        for &line in &walked.lines {
            let line = line as usize;
            if (new_lo..new_hi).contains(&line) {
                continue;
            }
            let painted = self.paint_line(&walked, line as u32);
            if painted != self.lines[line] {
                self.lines[line] = painted;
                changed.push(line);
            }
        }
        changed
    }

    /// What the syntax paints on `line` of the revision `walked` is of:
    /// the tokens read again on it, and what the kept subtrees on it
    /// paint there.
    fn paint_line(&self, walked: &Walked, line: u32) -> Box<[(u32, u32, u8)]> {
        let mut out: Vec<(u32, u32, u8)> = Vec::new();
        let from = walked.fresh.partition_point(|p| p.at.line < line);
        for p in walked.fresh[from..]
            .iter()
            .take_while(|p| p.at.line == line)
        {
            out.push((p.at.col, p.len, p.class));
        }
        let upto = walked.kept.partition_point(|c| c.at.line <= line);
        for c in walked.kept[..upto].iter().rev() {
            if c.at.line + self.keeper.kept[&c.key].end.line < line {
                break;
            }
            self.keeper.paint_on(c.key, c.at, line, &mut out);
        }
        out.sort_unstable_by_key(|&(col, _, _)| col);
        out.into_boxed_slice()
    }
}

/// Every token `tree`'s syntax paints: its byte span and its class (an
/// index into `styles.legend`), in document order. The classes a live
/// painter keeps across edits ([`crate::paint::Painter`]) are these.
pub fn syntax_classes(tree: &GreenNode, styles: &Styles) -> Vec<((u32, u32), u32)> {
    let mut out = Vec::new();
    let mut off = 0u32;
    fn walk(
        styles: &Styles,
        n: &GreenNode,
        given: Option<u8>,
        off: &mut u32,
        out: &mut Vec<((u32, u32), u32)>,
    ) {
        for_children(styles, n, given, |c, class| match c {
            GreenChild::Node(m) => walk(styles, m, class, off, out),
            GreenChild::Token(t) => {
                let len = t.text.len() as u32;
                if let (Some(class), true) = (class, len > 0) {
                    out.push(((*off, *off + len), class as u32));
                }
                *off += len;
            }
        });
    }
    walk(styles, tree, None, &mut off, &mut out);
    out
}
