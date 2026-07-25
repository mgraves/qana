//! Subtree IDENTITY as a key that cannot dangle.
//!
//! This crate's caches are keyed by subtree identity — pointer
//! equality, because two structurally equal subtrees at different
//! places are different cache entries, and reuse across an edit is
//! exactly "the same allocation came back". That is sound only while
//! the subtree is ALIVE: a freed address can be recycled by a later
//! allocation and silently answer for a different tree (an ABA on
//! addresses). This project shipped that bug once — a cache keyed by
//! `usize` whose values did not own the tree, so a signature edit
//! could replay another item's stale types, reproducible only under
//! particular allocation histories.
//!
//! The fix is not discipline ("remember to keep an `Arc` beside the
//! key"), because discipline is a comment and comments do not fail
//! the build. The fix is that THE KEY OWNS THE TREE. There is no
//! constructor from an address, and no way to obtain a `NodeKey`
//! except by handing over an `Arc`, so the unsound state — an entry
//! keyed by something nothing keeps alive — is not expressible. The
//! invariant lives in the type, and the compiler enforces it.

use rantlr_grammar::GreenNode;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

/// A cache key that IS an owning handle to the subtree it identifies.
/// Equality and hashing are by address; liveness is by construction.
#[derive(Clone, Debug)]
pub struct NodeKey(Arc<GreenNode>);

impl NodeKey {
    /// The only way in: you must give up an owned handle, which is
    /// what makes the address stable for as long as the key lives.
    pub fn new(node: Arc<GreenNode>) -> NodeKey {
        NodeKey(node)
    }

    /// The subtree this key identifies (and keeps alive).
    pub fn node(&self) -> &Arc<GreenNode> {
        &self.0
    }

    fn addr(&self) -> usize {
        Arc::as_ptr(&self.0) as usize
    }
}

impl PartialEq for NodeKey {
    fn eq(&self, other: &NodeKey) -> bool {
        self.addr() == other.addr()
    }
}
impl Eq for NodeKey {}

impl Hash for NodeKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.addr().hash(state);
    }
}

impl From<Arc<GreenNode>> for NodeKey {
    fn from(node: Arc<GreenNode>) -> NodeKey {
        NodeKey::new(node)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rantlr_grammar::green::GreenChild;

    fn leaf() -> Arc<GreenNode> {
        Arc::new(GreenNode {
            nt: 0,
            prod: 0,
            width: 0,
            terms: 0,
            n_toks: 0,
            has_err: false,
            children: Vec::<GreenChild>::new(),
        })
    }

    /// Identity, not structure: equal contents at different
    /// allocations are different keys, and a clone is the same key.
    #[test]
    fn keys_are_identities() {
        let a = leaf();
        let b = leaf();
        assert_eq!(a, b, "structurally equal");
        assert_ne!(NodeKey::new(a.clone()), NodeKey::new(b), "…but distinct identities");
        assert_eq!(NodeKey::new(a.clone()), NodeKey::new(a.clone()), "same allocation");
    }

    /// The point of the type: a key keeps its subtree alive, so its
    /// address cannot be recycled while the key exists.
    #[test]
    fn a_key_pins_its_subtree() {
        let key = {
            let node = leaf();
            let k = NodeKey::new(node.clone());
            drop(node); // the caller's handle goes away…
            k // …and the key still owns one.
        };
        assert_eq!(Arc::strong_count(key.node()), 1, "the key is the sole owner");
        assert_eq!(key, key.clone());
    }
}
