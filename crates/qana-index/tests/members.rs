//! A file's member accesses, each with what it is a member of as the file
//! writes it (`qana_index::members`, `member_at`, `path_at`).

use std::sync::{Arc, OnceLock};

use qana_index::{
    member_at, members, path_at, MemberAccess, MemberKind, Path, Receiver, Root, TypeRef,
};
use qana_lang::live::LiveDoc;
use qana_lang::EmbeddedLang;

const RUST_QANA: &str = include_str!("../../../examples/rust/rust.qana");

fn rust() -> &'static EmbeddedLang {
    static LANG: OnceLock<EmbeddedLang> = OnceLock::new();
    LANG.get_or_init(|| EmbeddedLang::from_qana_source(RUST_QANA).expect("rust.qana certifies"))
}

const GRID: &str = "use crate::cell::Cell;

pub struct Grid<T> {
    cells: Vec<T>,
    size: (usize, usize),
}

pub struct Px(pub f32, Cell);

impl<T> Grid<T> {
    pub fn new() -> Self {
        Self { cells: Vec::new(), size: (0, 0) }
    }

    fn render(&self, c: &Cell, item: T) -> usize {
        self.cells.len();
        c.width();
        Cell::new().width();
        Self::new().render(c, item);
        item.go();
        let made = Cell::new();
        made.height;
        let again = made;
        again.height;
        let typed: Cell = Cell::new();
        typed.depth();
        (*c).width();
        (&made).width();
        (self.size as Px).0.width();
        self.size.0.count();
        let p = Px(1.0, Cell::new());
        p.1.width();
        p.0.1;
        Grid { cells: Vec::new(), size: (1, 1) }.cells.len();
        [1, 2].iter().map(|x| x.unknown());
        self.cells.first()?.unknown();
        let Some(part) = Some(Cell::new()) else { return 0 };
        part.whole();
        let mut fresh = Cell::new();
        fresh.whole();
        0
    }
}

pub trait Shape {
    fn area(&self) -> f32;

    fn twice(&self) -> f32 {
        self.area() * 2.0
    }
}

mod inner {
    fn f(cell: crate::cell::Cell) {
        cell.width();
    }
}
";

fn named(segments: &[&str]) -> Path {
    Path { root: None, segments: segments.iter().map(|s| s.to_string()).collect() }
}

fn ty(segments: &[&str], args: Vec<TypeRef>) -> TypeRef {
    TypeRef::Path { path: named(segments), args, bindings: Vec::new() }
}

fn plain(name: &str) -> TypeRef {
    ty(&[name], Vec::new())
}

fn member(of: Receiver, name: &str, kind: MemberKind) -> Receiver {
    Receiver::Member { of: Box::new(of), name: name.to_string(), kind }
}

fn call(of: Receiver) -> Receiver {
    Receiver::Call(Box::new(of))
}

fn position(of: Receiver, index: u32) -> Receiver {
    Receiver::Position { of: Box::new(of), index }
}

/// The accesses named `name`, in order.
fn accesses<'a>(all: &'a [MemberAccess], name: &str) -> Vec<&'a MemberAccess> {
    all.iter().filter(|a| a.name == name).collect()
}

#[test]
fn a_members_receiver_is_what_the_file_writes_of_its_value() {
    let all = members(rust(), GRID);
    let grid = ty(&["Grid"], vec![plain("T")]);
    let cell_new = call(Receiver::Path(named(&["Cell", "new"])));

    let len = accesses(&all, "len");
    assert_eq!(
        len[0].receiver,
        member(Receiver::Typed(grid.clone()), "cells", MemberKind::Field),
        "self is of the impl block's type"
    );
    assert_eq!(len[0].kind, MemberKind::Method);
    assert_eq!(len[0].generics, ["T"], "the impl block's parameters");

    let width = accesses(&all, "width");
    assert_eq!(width[0].receiver, Receiver::Typed(TypeRef::Ref(Box::new(plain("Cell")))));
    assert_eq!(width[1].receiver, cell_new, "what calling a path gives");
    assert_eq!(width[2].receiver, Receiver::Typed(TypeRef::Ref(Box::new(plain("Cell")))), "*c");
    assert_eq!(width[3].receiver, cell_new, "&made, by the value made holds");
    assert_eq!(
        width[4].receiver,
        position(Receiver::Typed(plain("Px")), 0),
        "a cast's type, then a positional field"
    );
    assert_eq!(
        width[5].receiver,
        position(call(Receiver::Path(named(&["Px"]))), 1),
        "a binding's value, a tuple struct called"
    );
    assert_eq!(
        width[6].receiver,
        Receiver::Typed(TypeRef::Path {
            path: Path { root: Some(Root::Crate), segments: vec!["cell".into(), "Cell".into()] },
            args: Vec::new(),
            bindings: Vec::new(),
        }),
        "a parameter's type, in a module"
    );
    assert_eq!(width[6].units, ["inner"]);

    let render = accesses(&all, "render");
    assert_eq!(
        render[0].receiver,
        call(Receiver::Path(named(&["Grid", "new"]))),
        "Self::new is the impl block's type's"
    );
    assert_eq!(accesses(&all, "go")[0].receiver, Receiver::Typed(plain("T")));

    let height = accesses(&all, "height");
    assert_eq!(height[0].receiver, cell_new);
    assert_eq!(height[0].kind, MemberKind::Field);
    assert_eq!(height[1].receiver, cell_new, "a binding of a binding");
    assert_eq!(accesses(&all, "depth")[0].receiver, Receiver::Typed(plain("Cell")), "written");

    assert_eq!(
        accesses(&all, "count")[0].receiver,
        position(member(Receiver::Typed(grid.clone()), "size", MemberKind::Field), 0),
        "`.0.` is one token"
    );
    assert_eq!(
        len[1].receiver,
        member(Receiver::Typed(plain("Grid")), "cells", MemberKind::Field),
        "a struct literal's type"
    );

    let whole = accesses(&all, "whole");
    assert_eq!(whole[0].receiver, Receiver::Unknown, "a pattern that destructures binds a part");
    assert_eq!(whole[1].receiver, cell_new, "`mut fresh` is the whole value");

    let unknown = accesses(&all, "unknown");
    assert_eq!(unknown[0].receiver, Receiver::Unknown, "a closure's parameter");
    assert_eq!(unknown[1].receiver, Receiver::Unknown, "`?`");

    assert_eq!(
        accesses(&all, "area")[0].receiver,
        Receiver::Typed(plain("Shape")),
        "in a trait, self is of a type with it"
    );
    assert!(accesses(&all, "1").is_empty(), "a positional field names no member");
}

#[test]
fn the_member_at_an_offset_and_a_path_from_self() {
    let mut doc = LiveDoc::open(Arc::new(rust().clone()), "grid.rs", GRID);
    let at = |word: &str, nth: usize| GRID.match_indices(word).nth(nth).expect(word).0 as u32;

    let access = member_at(&mut doc, at("width", 0) + 2).expect("a member at c.width");
    assert_eq!(access.name, "width");
    assert_eq!(access.span, (at("width", 0), at("width", 0) + 5));
    assert!(member_at(&mut doc, at("Cell::new", 1)).is_none(), "a path is no member");

    // `Self::new()`: the path `new` spells, said from the impl block.
    let (units, path, imported) = path_at(&mut doc, at("Self::new", 0) + 6).expect("a path");
    assert_eq!((units.len(), imported), (0, false));
    assert_eq!(path, named(&["Grid", "new"]));
}

// ---------------------------------------------------------------------------
// Typing across a project (`Resolver::member`)
// ---------------------------------------------------------------------------

const LIB: &str = "mod cell;
mod grid;
mod shapes;
";

const CELL: &str = "#[derive(Clone)]
pub struct Cell {
    pub width: f32,
    pub inner: Inner,
}

pub struct Inner {
    pub depth: u8,
}

impl Cell {
    pub fn new() -> Self {
        todo!()
    }

    pub fn width(&self) -> f32 {
        self.width
    }

    pub fn inner(&self) -> &Inner {
        &self.inner
    }

    pub fn boxed(self) -> Box<Self> {
        todo!()
    }
}

impl Inner {
    pub fn depth(&self) -> u8 {
        self.depth
    }
}

pub trait Draw {
    fn draw(&self) -> Cell;

    fn redraw(&self) -> Cell {
        self.draw()
    }
}

impl Draw for Cell {
    fn draw(&self) -> Cell {
        Cell::new()
    }
}

pub trait Hidden {
    fn draw(&self);
    fn secret(&self);
}

impl Hidden for Cell {
    fn draw(&self) {}
    fn secret(&self) {}
}

pub trait Shape: Draw {
    fn area(&self) -> f32;
}

pub type Alias = Cell;

pub const ORIGIN: Cell = Cell { width: 0.0, inner: Inner { depth: 0 } };

pub struct Px(pub f32, pub Cell);

pub enum Mark {
    Dot(Cell),
    Blank,
}

/// A type named as a generic parameter often is.
pub struct T;

impl T {
    pub fn default() -> Cell {
        todo!()
    }
}
";

const GRID_RS: &str = "use crate::cell::{Alias, Cell, Draw, Mark, Px, Shape, ORIGIN};

pub struct Grid {
    first: Cell,
    pair: (Cell, u8),
}

impl Grid {
    fn new() -> Self {
        todo!()
    }

    fn go(&self, c: &Cell, s: &dyn Shape, a: Alias) {
        self.first.width();
        c.inner().depth();
        Cell::new().width;
        Self::new().first.width();
        c.draw();
        c.redraw();
        c.clone();
        c.secret();
        s.area();
        s.draw();
        a.width();
        ORIGIN.width();
        self.pair.0.width();
        Px(1.0, Cell::new()).1.width();
        Mark::Dot(Cell::new()).zap();
        Mark::Blank.zap();
        c.boxed().width();
        unknown.width();
    }
}

fn pick<T>() {
    T::default().width();
    crate::cell::T::default().width();
}
";

const SHAPES: &str = "use crate::cell::{Cell, Hidden as _};

fn f(c: Cell) {
    c.secret();
}
";

const CORE: &str = "pub mod prelude {
    pub use crate::clone::Clone;
}

pub mod clone {
    pub trait Clone {
        fn clone(&self) -> Self;
    }
}
";

const FILES: [(&str, &str); 5] = [
    ("/p/src/lib.rs", LIB),
    ("/p/src/cell.rs", CELL),
    ("/p/src/grid.rs", GRID_RS),
    ("/p/src/shapes.rs", SHAPES),
    ("/core/lib.rs", CORE),
];

/// The project: crate `geo` at `/p/src/lib.rs` with three modules in files,
/// and `core`, which `geo` names and whose prelude it has.
fn project() -> qana_index::Resolver {
    use std::path::{Path as FilePath, PathBuf};
    use std::rc::Rc;

    use qana_index::{summarize, CrateInfo, CrateSource, Resolver, Summary, UnitGraph};

    let summaries: Vec<(PathBuf, Rc<Summary>)> = FILES
        .iter()
        .map(|(path, text)| (PathBuf::from(path), Rc::new(summarize(rust(), text))))
        .collect();
    let krate = |name: &str, root: &str, dependencies: Vec<(String, u32)>, preludes| CrateInfo {
        name: name.into(),
        kind: "lib".into(),
        root: root.into(),
        edition: "2021".into(),
        dependencies,
        loadable: Vec::new(),
        source: CrateSource::Workspace,
        preludes,
        builtins: Vec::new(),
        closed_by: Vec::new(),
        inert: Vec::new(),
        imports_from_root: false,
        unindexed: Vec::new(),
    };
    let mut graph = UnitGraph::new();
    let core = graph.add_crate(krate("core", "/core/lib.rs", Vec::new(), Vec::new()));
    let geo = graph.add_crate(krate(
        "geo",
        "/p/src/lib.rs",
        vec![("core".into(), core)],
        vec![named(&["core", "prelude"])],
    ));
    let lib = &summaries[0].1;
    for name in ["cell", "grid", "shapes"] {
        let unit = lib.offers().units().iter().position(|u| u.name == name).unwrap() as u32;
        graph.add_body(
            geo,
            FilePath::new("/p/src/lib.rs"),
            unit,
            format!("/p/src/{name}.rs").into(),
            vec![name.into()],
        );
    }
    Resolver::new(Rc::new(graph), summaries)
}

/// A declaration: its file's name, its container, its name and its kind.
type Named = (String, String, String, String);

/// What the `nth` access named `name` in `file` names, each declaration
/// said as [`Named`], or the segment it escaped at.
fn typed_member(
    resolver: &qana_index::Resolver,
    file: &str,
    name: &str,
    nth: usize,
) -> Result<Vec<Named>, usize> {
    use qana_index::{summarize, Resolution};

    let text = FILES.iter().find(|(path, _)| *path == file).unwrap().1;
    let all = members(rust(), text);
    let access = accesses(&all, name).get(nth).copied().unwrap_or_else(|| panic!("{name} {nth}"));
    match resolver.member(std::path::Path::new(file), access) {
        Resolution::Found { declarations, modules } => {
            assert!(modules.is_empty());
            Ok(declarations
                .into_iter()
                .map(|(path, index)| {
                    let path = path.to_str().unwrap().to_string();
                    let text = FILES.iter().find(|(p, _)| *p == path).unwrap().1;
                    let d = summarize(rust(), text).offers().declarations()[index as usize].clone();
                    let file = path.rsplit('/').next().unwrap().to_string();
                    (file, d.container.join("::"), d.name, d.kind)
                })
                .collect())
        }
        Resolution::Escapes { segment } => Err(segment),
        other => panic!("a member is never {other:?}"),
    }
}

/// A function declared in `file`, in `container`.
fn at(file: &str, container: &str, name: &str) -> Named {
    (file.into(), container.into(), name.into(), "function".into())
}

/// A field declared in `file`, in `container`.
fn field(file: &str, container: &str, name: &str) -> Named {
    (file.into(), container.into(), name.into(), "field".into())
}

#[test]
fn a_member_is_found_on_the_type_its_value_is_written_to_have() {
    let r = project();
    let grid = "/p/src/grid.rs";
    let width = |nth| typed_member(&r, grid, "width", nth);
    let cell_width = Ok(vec![at("cell.rs", "Cell", "width")]);

    assert_eq!(width(0), cell_width, "self.first: a field of the impl block's type");
    assert_eq!(
        typed_member(&r, grid, "depth", 0),
        Ok(vec![at("cell.rs", "Inner", "depth")]),
        "a method's written return type, through a reference"
    );
    assert_eq!(width(1), Ok(vec![field("cell.rs", "Cell", "width")]), "a field, read");
    assert_eq!(width(2), cell_width, "Self::new returns Self, the impl block's type");
    assert_eq!(width(3), cell_width, "an alias, the type it is of");
    assert_eq!(width(4), cell_width, "a constant's type");
    assert_eq!(width(5), cell_width, "a tuple's positional field");
    assert_eq!(width(6), cell_width, "a tuple struct's");
    assert_eq!(width(7), Err(0), "Box is not in the project: its value is not typed");
    assert_eq!(width(8), Err(0), "a name no file writes the type of");
    assert_eq!(width(9), Err(0), "`T::default()` is the generic parameter's, not the struct's");
    assert_eq!(width(10), cell_width, "the struct's, by its path");
}

#[test]
fn a_traits_method_is_found_where_the_trait_is_in_scope() {
    let r = project();
    let grid = "/p/src/grid.rs";
    assert_eq!(
        typed_member(&r, grid, "draw", 0),
        Ok(vec![at("cell.rs", "Cell", "draw")]),
        "the impl block's, of the trait imported; not Hidden's, not in scope"
    );
    assert_eq!(
        typed_member(&r, grid, "redraw", 0),
        Ok(vec![at("cell.rs", "Draw", "redraw")]),
        "a provided method: the trait's own"
    );
    assert_eq!(
        typed_member(&r, grid, "clone", 0),
        Ok(vec![at("lib.rs", "clone::Clone", "clone")]),
        "a derived trait, in scope by the prelude"
    );
    assert_eq!(typed_member(&r, grid, "secret", 0), Err(1), "Hidden is not in scope here");
    assert_eq!(
        typed_member(&r, "/p/src/shapes.rs", "secret", 0),
        Ok(vec![at("cell.rs", "Cell", "secret")]),
        "brought into scope without a name"
    );
    assert_eq!(
        typed_member(&r, grid, "area", 0),
        Ok(vec![at("cell.rs", "Shape", "area")]),
        "a trait object's trait's"
    );
    assert_eq!(
        typed_member(&r, grid, "draw", 1),
        Ok(vec![at("cell.rs", "Draw", "draw")]),
        "and its supertrait's"
    );
    assert_eq!(typed_member(&r, grid, "zap", 0), Err(1), "a tuple variant: of its enum");
    assert_eq!(typed_member(&r, grid, "zap", 1), Err(1), "a unit variant: of its enum");
    assert_eq!(
        typed_member(&r, "/p/src/cell.rs", "draw", 0),
        Ok(vec![at("cell.rs", "Draw", "draw")]),
        "in a trait, self has the trait's items"
    );
}

#[test]
fn a_paths_member_of_a_type_is_a_traits_where_the_trait_is_in_scope() {
    use qana_index::{summarize, Resolution};

    let r = project();
    let at_path = |file: &str, segments: &[&str]| -> Result<Vec<Named>, usize> {
        match r.resolve(std::path::Path::new(file), &[], &named(segments), Some("")) {
            Resolution::Found { declarations, .. } => Ok(declarations
                .into_iter()
                .map(|(path, index)| {
                    let path = path.to_str().unwrap().to_string();
                    let text = FILES.iter().find(|(p, _)| *p == path).unwrap().1;
                    let d = summarize(rust(), text).offers().declarations()[index as usize].clone();
                    let file = path.rsplit('/').next().unwrap().to_string();
                    (file, d.container.join("::"), d.name, d.kind)
                })
                .collect()),
            Resolution::Escapes { segment } | Resolution::Unresolved { segment } => Err(segment),
            Resolution::Builtin => Err(usize::MAX),
        }
    };
    assert_eq!(
        at_path("/p/src/grid.rs", &["Cell", "draw"]),
        Ok(vec![at("cell.rs", "Cell", "draw")]),
        "Draw's, imported; not Hidden's"
    );
    assert_eq!(at_path("/p/src/grid.rs", &["Cell", "secret"]), Err(1), "Hidden is not in scope");
    assert_eq!(
        at_path("/p/src/shapes.rs", &["Cell", "secret"]),
        Ok(vec![at("cell.rs", "Cell", "secret")]),
        "brought in without a name"
    );
    assert_eq!(
        at_path("/p/src/grid.rs", &["Cell", "redraw"]),
        Ok(vec![at("cell.rs", "Draw", "redraw")]),
        "a provided method, the trait's"
    );
}
