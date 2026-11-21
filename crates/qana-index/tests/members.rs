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
    assert_eq!(
        unknown[1].receiver,
        Receiver::Try(Box::new(member(
            member(Receiver::Typed(grid.clone()), "cells", MemberKind::Field),
            "first",
            MemberKind::Method
        ))),
        "`?`: what the value gives up"
    );

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
mod deref;

/// A module of the crate's own, named like a crate the crate names.
mod core {}
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

/// Its impl block's trait's path walks through it: finding the trait asks
/// for its members, which ask for the trait.
#[allow(non_camel_case_types)]
pub struct hash;

impl hash::Hash for hash {
    fn digest(&self) {}
}

fn g(h: hash) {
    h.digest();
}
";

const CORE: &str = "pub mod prelude {
    pub use crate::clone::Clone;
    pub use crate::default::Default;
    pub use crate::iter::Iterator;
    pub use crate::option::Option;
    pub use crate::string::ToString;
}

pub mod clone {
    pub trait Clone {
        fn clone(&self) -> Self;
    }
}

pub mod default {
    pub trait Default {
        fn default() -> Self;
    }
}

pub mod marker {
    pub unsafe auto trait Send {}
}

pub mod any {
    pub trait Any {}

    impl dyn Any {
        pub fn is(&self) -> bool {
            todo!()
        }
    }

    impl dyn Any + crate::marker::Send {
        pub fn downcast_ref(&self) -> bool {
            todo!()
        }
    }
}

pub mod ops {
    pub trait Deref {
        type Target;
        fn deref(&self) -> &Self::Target;
    }

    pub trait Try {
        type Output;
    }
}

pub mod option {
    pub enum Option<T> {
        None,
        Some(T),
    }

    impl<T> Option<T> {
        pub fn unwrap(self) -> T {
            todo!()
        }
    }

    impl<T> crate::ops::Try for Option<T> {
        type Output = T;
    }
}

pub mod rc {
    pub struct Rc<T>(T);

    impl<T> crate::ops::Deref for Rc<T> {
        type Target = T;
        fn deref(&self) -> &T {
            &self.0
        }
    }
}

pub mod cell {
    pub struct RefCell<T>(T);

    pub struct Ref<'b, T> {
        value: &'b T,
    }

    impl<T> RefCell<T> {
        pub fn borrow(&self) -> Ref<'_, T> {
            todo!()
        }
    }

    impl<'b, T> crate::ops::Deref for Ref<'b, T> {
        type Target = T;
        fn deref(&self) -> &T {
            self.value
        }
    }
}

pub mod boxed {
    pub struct Box<T>(T);

    impl<T> crate::ops::Deref for Box<T> {
        type Target = T;
        fn deref(&self) -> &T {
            &self.0
        }
    }
}

pub mod vec {
    pub struct Vec<T> {
        first: T,
    }

    impl<T> crate::ops::Deref for Vec<T> {
        type Target = [T];
        fn deref(&self) -> &[T] {
            todo!()
        }
    }
}

pub mod slice {
    pub struct Iter<'a, T> {
        rest: &'a [T],
    }

    impl<T> [T] {
        pub fn first(&self) -> crate::option::Option<&T> {
            todo!()
        }

        pub fn iter(&self) -> Iter<'_, T> {
            todo!()
        }
    }

    impl<'a, T> crate::iter::Iterator for Iter<'a, T> {
        type Item = &'a T;
        fn next(&mut self) -> crate::option::Option<&'a T> {
            todo!()
        }
    }
}

pub mod iter {
    pub trait Iterator {
        type Item;
        fn next(&mut self) -> crate::option::Option<Self::Item>;
    }
}

pub mod fmt {
    pub trait Display {}

    /// Not in the prelude: there, `Debug` is the derive.
    pub trait Debug {}
}

pub mod string {
    /// A module named like a type the language provides.
    use crate::str;

    pub struct String;

    impl crate::ops::Deref for String {
        type Target = str;
        fn deref(&self) -> &str {
            todo!()
        }
    }

    pub trait ToString {
        fn to_string(&self) -> String;
    }

    impl<T: crate::fmt::Display> ToString for T {
        fn to_string(&self) -> String {
            String
        }
    }
}

pub mod str {
    impl str {
        pub fn len(&self) -> usize {
            0
        }
    }
}
";

/// A type's members through what it dereferences to, its arguments, its
/// associated types, and the types the language provides.
const DEREF: &str = "use crate::cell::Cell;
use core::any::Any;
use core::boxed::Box;
use core::cell::RefCell;
use core::marker::Send;
use core::rc::Rc;
use core::string::String;
use core::vec::Vec;

pub struct Grid<T> {
    cells: Vec<T>,
}

pub struct Walk;

#[derive(Default)]
pub struct Tally {
    count: Cell,
}

pub struct Panel<H, B> {
    h: H,
    b: B,
}

#[derive(Debug)]
pub struct Wide;

/// An alias whose impl block is its type's, its parameter one of its
/// type's arguments.
pub type Split<H> = Panel<H, Wide>;

impl<H> Split<H> {
    pub fn left(&self) -> H {
        todo!()
    }

    pub fn new() -> Self {
        todo!()
    }
}

pub type Main = Split<Cell>;

/// An `Rc` of what the call infers.
fn fresh<T>() -> Rc<T> {
    todo!()
}

impl Iterator for Walk {
    type Item = Cell;

    fn next(&mut self) -> Option<Self::Item> {
        todo!()
    }
}

fn reach(v: Vec<Cell>, b: Box<Cell>, g: Grid<Cell>, mut w: Walk, a: [Cell; 2], mut it: Box<dyn Iterator<Item = Cell>>, s: String, p: Main, any: Box<dyn Any + Send>, plain: &dyn Any, shared: Rc<RefCell<Cell>>) -> Option<f32> {
    v.first().unwrap().width();
    v.iter().next().unwrap().width();
    b.width();
    g.cells.first().unwrap().width();
    w.next().unwrap().width();
    a.first().unwrap().width();
    it.next().unwrap().width();
    \"x\".len();
    s.len();
    \"x\".to_string();
    let found = v.first()?;
    found.width();
    p.left().width();
    Main::new().left().width();
    Tally::default().count.width();
    any.downcast_ref();
    any.is();
    plain.is();
    plain.downcast_ref();
    shared.borrow().width();
    fresh().width();
    b.missing();
    todo!()
}
";

const FILES: [(&str, &str); 6] = [
    ("/p/src/lib.rs", LIB),
    ("/p/src/deref.rs", DEREF),
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
        builtins: ["str", "u8", "usize", "bool", "char", "f32"].map(String::from).to_vec(),
        closed_by: Vec::new(),
        inert: Vec::new(),
        imports_from_root: false,
        unindexed: Vec::new(),
        deref_trait: Some((core_ops(name, "Deref"), "Target".to_string())),
        try_trait: Some((core_ops(name, "Try"), "Output".to_string())),
        derive_traits: vec![("Debug".to_string(), core_path(name, &["fmt", "Debug"]))],
        macro_use: Vec::new(),
        macro_export: Vec::new(),
        macro_prelude: Vec::new(),
        conditions: Vec::new(),
    };
    let mut graph = UnitGraph::new();
    let core = graph.add_crate(krate("core", "/core/lib.rs", Vec::new(), Vec::new()));
    let geo = graph.add_crate(krate(
        "geo",
        "/p/src/lib.rs",
        vec![("core".into(), core)],
        vec![global(&["core", "prelude"])],
    ));
    let lib = &summaries[0].1;
    for name in ["cell", "grid", "shapes", "deref"] {
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

/// The path of core's `ops::item` for crate `name` (see [`core_path`]).
fn core_ops(name: &str, item: &str) -> Path {
    core_path(name, &["ops", item])
}

/// The path of core's item at `segments` for crate `name`: from its root in
/// core itself, from the crates themselves in any other (`::core::…`).
fn core_path(name: &str, segments: &[&str]) -> Path {
    match name {
        "core" => Path {
            root: Some(Root::Crate),
            segments: segments.iter().map(|s| s.to_string()).collect(),
        },
        _ => {
            let mut all = vec!["core"];
            all.extend(segments);
            global(&all)
        }
    }
}

/// A path from the crates themselves (`::a::b`).
fn global(segments: &[&str]) -> Path {
    Path { root: Some(Root::Global), ..named(segments) }
}

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
        typed_member(&r, "/p/src/shapes.rs", "digest", 0),
        Ok(vec![at("shapes.rs", "hash", "digest")]),
        "a trait path that leads back to its own block ends, the trait not held"
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

#[test]
fn a_member_is_found_through_arguments_associated_types_and_derefs() {
    let r = project();
    let deref = "/p/src/deref.rs";
    let width = |nth| typed_member(&r, deref, "width", nth);
    let cell_width = Ok(vec![at("cell.rs", "Cell", "width")]);
    assert_eq!(width(0), cell_width, "Vec<Cell> to [Cell]; first gives Option<&T>, T as Cell");
    assert_eq!(width(1), cell_width, "an iterator's Item, `&'a T` with T as Cell");
    assert_eq!(width(2), cell_width, "through Box's Deref, Target = T");
    assert_eq!(width(3), cell_width, "a field of a Grid<Cell>: Vec<T> as Vec<Cell>");
    assert_eq!(width(4), cell_width, "Option<Self::Item>, Item = Cell");
    assert_eq!(width(5), cell_width, "an array, unsized to a slice");
    assert_eq!(width(6), cell_width, "dyn Iterator<Item = Cell>, through a Box");
    assert_eq!(width(7), cell_width, "`v.first()?`: Option's Try Output");
    assert_eq!(width(8), cell_width, "an alias's impl block's method, its H the alias's argument");
    assert_eq!(width(9), cell_width, "`Main::new()`: Self is the path's alias, with its arguments");
    assert_eq!(
        width(10),
        cell_width,
        "`Tally::default()`: Self is Tally, though `default` is Default's"
    );
    assert_eq!(
        width(11),
        cell_width,
        "Rc's Deref to RefCell, its borrow's Ref, Ref's Deref to Cell"
    );
    assert_eq!(
        width(12),
        Err(0),
        "an Rc whose T a call infers: what it dereferences to is not known"
    );
    assert_eq!(
        typed_member(&r, deref, "missing", 0),
        Err(1),
        "Box<Cell> and the Cell it dereferences to known, and neither holds it"
    );
    assert_eq!(
        typed_member(&r, deref, "first", 0),
        Ok(vec![at("lib.rs", "slice::[T]", "first")]),
        "a slice's impl block, reached from a Vec"
    );
    assert_eq!(
        typed_member(&r, deref, "len", 0),
        Ok(vec![at("lib.rs", "str::str", "len")]),
        "a string literal's str"
    );
    assert_eq!(
        typed_member(&r, deref, "len", 1),
        Ok(vec![at("lib.rs", "str::str", "len")]),
        "String's Target is the language's str, though its module imports a module `str`, \
         and Deref is core's, though geo's root declares a module `core`"
    );
    assert_eq!(
        typed_member(&r, deref, "to_string", 0),
        Ok(vec![at("lib.rs", "string::T", "to_string")]),
        "a blanket impl, of a trait in scope by the prelude"
    );
    let object = |name: &str, nth| typed_member(&r, deref, name, nth);
    assert_eq!(
        object("downcast_ref", 0),
        Ok(vec![at("lib.rs", "any::dyn Any + crate::marker::Send", "downcast_ref")]),
        "through Box, an impl block of the trait object of exactly its traits"
    );
    assert_eq!(object("is", 1), Ok(vec![at("lib.rs", "any::dyn Any", "is")]));
    assert_eq!(object("is", 0), Err(1), "`impl dyn Any`'s type is not `dyn Any + Send`");
    assert_eq!(object("downcast_ref", 1), Err(1), "nor the other way about");
    let path = named(&["Main", "new"]);
    assert!(
        matches!(
            r.resolve(std::path::Path::new(deref), &[], &path, None),
            qana_index::Resolution::Found { declarations, .. }
                if declarations.len() == 1 && declarations[0].0.ends_with("deref.rs")
        ),
        "a path through an alias names its type's member"
    );
}

/// A Rust 2015 crate: a path from `::` starts at its root, which declares a
/// module `core` and loads the crate core under another name.
const OLD: &str = "extern crate core as real;

mod core {}

use real::boxed::Box;

pub struct Cell;

impl Cell {
    pub fn width(&self) -> f32 {
        0.0
    }
}

fn f(b: Box<Cell>) {
    b.width();
}
";

#[test]
fn a_lang_trait_is_its_crates_whatever_the_root_calls_by_its_name() {
    use std::path::{Path as FilePath, PathBuf};
    use std::rc::Rc;

    use qana_index::{summarize, CrateInfo, CrateSource, Resolution, Resolver, UnitGraph};

    let krate =
        |name: &str, root: &str, edition: &str, dependencies: Vec<(String, u32)>| CrateInfo {
            name: name.into(),
            kind: "lib".into(),
            root: root.into(),
            edition: edition.into(),
            dependencies,
            loadable: Vec::new(),
            source: CrateSource::Workspace,
            preludes: Vec::new(),
            builtins: Vec::new(),
            closed_by: Vec::new(),
            inert: Vec::new(),
            imports_from_root: edition == "2015",
            unindexed: Vec::new(),
            deref_trait: Some((core_ops(name, "Deref"), "Target".to_string())),
            try_trait: Some((core_ops(name, "Try"), "Output".to_string())),
            derive_traits: Vec::new(),
            macro_use: Vec::new(),
            macro_export: Vec::new(),
            macro_prelude: Vec::new(),
            conditions: Vec::new(),
        };
    let mut graph = UnitGraph::new();
    let core = graph.add_crate(krate("core", "/core/lib.rs", "2021", Vec::new()));
    graph.add_crate(krate("old", "/old/lib.rs", "2015", vec![("core".into(), core)]));
    let summaries: Vec<(PathBuf, Rc<qana_index::Summary>)> =
        [("/core/lib.rs", CORE), ("/old/lib.rs", OLD)]
            .iter()
            .map(|(path, text)| (PathBuf::from(path), Rc::new(summarize(rust(), text))))
            .collect();
    let r = Resolver::new(Rc::new(graph), summaries);

    let all = members(rust(), OLD);
    let access = accesses(&all, "width")[0];
    let file = FilePath::new("/old/lib.rs");
    assert_eq!(
        r.resolve(file, &[], &named(&["real"]), None),
        Resolution::Found {
            declarations: Vec::new(),
            modules: vec![PathBuf::from("/core/lib.rs")]
        },
        "`extern crate core as real;` loads the crate, not the root's module `core`"
    );
    assert_eq!(
        r.resolve(file, &[], &global(&["core", "ops", "Deref"]), None),
        Resolution::Unresolved { segment: 1 },
        "written in this crate, `::core` is its root's module"
    );
    let Resolution::Found { declarations, .. } = r.member(file, access) else {
        panic!("Box<Cell> dereferences to Cell by core's Deref, not by the root's `core`");
    };
    let declared = summarize(rust(), OLD);
    let width = declared.offers().declarations().iter().position(|d| d.name == "width").unwrap();
    assert_eq!(declarations, vec![(PathBuf::from("/old/lib.rs"), width as u32)]);
}

/// Where the declaration named `name`, of `kind`, inside `container`, of
/// `file` is implemented, each said as its file's name and what it is.
fn implemented(
    r: &qana_index::Resolver,
    file: &str,
    container: &str,
    name: &str,
    kind: &str,
) -> Vec<String> {
    use qana_index::{summarize, Implementation};

    let summary_of = |path: &std::path::Path| {
        let text = FILES.iter().find(|(p, _)| std::path::Path::new(p) == path).unwrap().1;
        summarize(rust(), text)
    };
    let short = |path: &std::path::Path| path.file_name().unwrap().to_str().unwrap().to_string();
    let path = std::path::Path::new(file);
    let summary = summary_of(path);
    let index = summary
        .offers()
        .declarations()
        .iter()
        .position(|d| d.container.join("::") == container && d.name == name && d.kind == kind)
        .unwrap_or_else(|| panic!("{container}::{name}, a {kind}, in {file}"));
    r.implementations(path, index as u32)
        .into_iter()
        .map(|found| match found {
            Implementation::Block { file, block } => {
                let summary = summary_of(&file);
                let of = &summary.offers().impls()[block as usize];
                let last = |p: Option<&qana_index::Path>| {
                    p.and_then(|p| p.segments.last().cloned()).unwrap_or_default()
                };
                let ty = last(of.self_type.as_ref().and_then(qana_index::TypeRef::path));
                match &of.trait_path {
                    Some(t) => format!("{}: impl {} for {ty}", short(&file), last(Some(t))),
                    None => format!("{}: impl {ty}", short(&file)),
                }
            }
            Implementation::Item { file, declaration } => {
                let summary = summary_of(&file);
                let d = &summary.offers().declarations()[declaration as usize];
                let block = &summary.offers().impls()[d.impl_block.unwrap() as usize];
                let t = block.trait_path.as_ref().and_then(|t| t.segments.last()).unwrap();
                format!("{}: {}::{} of {t}", short(&file), d.container.join("::"), d.name)
            }
            Implementation::Derived { file, declaration, derives } => {
                let d =
                    &summary_of(&file).offers().declarations()[declaration as usize].name.clone();
                let t =
                    summary_of(&derives.0).offers().declarations()[derives.1 as usize].name.clone();
                format!("{}: {d} derives {t}", short(&file))
            }
        })
        .collect()
}

#[test]
fn a_member_access_can_name_a_field_or_a_method_and_nothing_else() {
    use qana_index::summarize;

    let r = project();
    let member = |file: &str, container: &str, name: &str, kind: &str| {
        let text = FILES.iter().find(|(p, _)| *p == file).unwrap().1;
        let summary = summarize(rust(), text);
        let index = summary
            .offers()
            .declarations()
            .iter()
            .position(|d| d.container.join("::") == container && d.name == name && d.kind == kind)
            .unwrap_or_else(|| panic!("{container}::{name}"));
        r.is_member(std::path::Path::new(file), index as u32)
    };
    let (cell, deref) = ("/p/src/cell.rs", "/p/src/deref.rs");
    assert!(member(cell, "Cell", "width", "field"), "a field");
    assert!(member(cell, "Cell", "width", "function"), "an impl block's function");
    assert!(member(cell, "Draw", "draw", "function"), "a trait's");
    assert!(!member(deref, "", "fresh", "function"), "a free function");
    assert!(!member(cell, "", "Cell", "struct"), "a type");
}

#[test]
fn implementations_of_a_trait_its_items_and_a_type() {
    use qana_index::summarize;

    let r = project();
    let (core, cell, deref) = ("/core/lib.rs", "/p/src/cell.rs", "/p/src/deref.rs");
    assert_eq!(
        implemented(&r, core, "iter", "Iterator", "class"),
        ["lib.rs: impl Iterator for Iter", "deref.rs: impl Iterator for Walk"],
        "a trait's impl blocks, in files' order"
    );
    assert_eq!(
        implemented(&r, core, "iter::Iterator", "next", "function"),
        ["lib.rs: slice::Iter<'a, T>::next of Iterator", "deref.rs: Walk::next of Iterator"],
        "a trait's method: its implementors' methods"
    );
    assert_eq!(
        implemented(&r, cell, "Draw", "draw", "function"),
        ["cell.rs: Cell::draw of Draw"],
        "not `Hidden`'s `draw`, of another trait"
    );
    assert_eq!(
        implemented(&r, cell, "Hidden", "draw", "function"),
        ["cell.rs: Cell::draw of Hidden"]
    );
    assert!(
        implemented(&r, cell, "Draw", "redraw", "function").is_empty(),
        "provided, not overridden"
    );
    assert_eq!(
        implemented(&r, core, "default", "Default", "class"),
        ["deref.rs: Tally derives Default"],
        "a derive implements a trait with no impl block written"
    );
    assert_eq!(implemented(&r, core, "clone", "Clone", "class"), ["cell.rs: Cell derives Clone"]);
    assert_eq!(
        implemented(&r, core, "fmt", "Debug", "class"),
        ["deref.rs: Wide derives Debug"],
        "a derive that names no trait: the built-in derive's, by its name"
    );
    let wide =
        summarize(rust(), DEREF).offers().declarations().iter().position(|d| d.name == "Wide");
    let debug = summarize(rust(), CORE)
        .offers()
        .declarations()
        .iter()
        .position(|d| d.name == "Debug" && d.kind == "class");
    assert_eq!(
        r.derived_traits(std::path::Path::new(deref), wide.unwrap() as u32),
        [vec![(std::path::PathBuf::from(core), debug.unwrap() as u32)]],
        "each derive's trait, in the derives' order"
    );
    assert_eq!(
        implemented(&r, core, "string", "ToString", "class"),
        ["lib.rs: impl ToString for T"],
        "a blanket impl"
    );
    assert_eq!(
        implemented(&r, cell, "", "Cell", "struct"),
        [
            "cell.rs: impl Cell",
            "cell.rs: impl Draw for Cell",
            "cell.rs: impl Hidden for Cell",
            "cell.rs: Cell derives Clone"
        ],
        "a type's impl blocks, inherent and of traits, and what it derives"
    );
    assert_eq!(
        implemented(&r, deref, "", "Split", "struct"),
        ["deref.rs: impl Split"],
        "an alias's: those of the type it stands for"
    );
    assert_eq!(
        implemented(&r, deref, "Walk", "next", "function"),
        ["deref.rs: Walk::next of Iterator"],
        "an impl block's item implementing a trait's is its own implementation"
    );
    assert!(implemented(&r, cell, "Cell", "new", "function").is_empty(), "an inherent method");

    // Whether anything is, as far as the first answer: as each list says.
    for (file, container, name, kind) in [
        (core, "iter", "Iterator", "class"),
        (core, "iter::Iterator", "next", "function"),
        (cell, "Draw", "draw", "function"),
        (cell, "Draw", "redraw", "function"),
        (core, "default", "Default", "class"),
        (core, "fmt", "Debug", "class"),
        (core, "fmt", "Display", "class"),
        (cell, "", "Cell", "struct"),
        (deref, "", "Split", "struct"),
        (deref, "Walk", "next", "function"),
        (cell, "Cell", "new", "function"),
        (cell, "", "Inner", "struct"),
    ] {
        let summary = summarize(rust(), FILES.iter().find(|(p, _)| *p == file).unwrap().1);
        let index = summary
            .offers()
            .declarations()
            .iter()
            .position(|d| d.container.join("::") == container && d.name == name && d.kind == kind)
            .unwrap() as u32;
        assert_eq!(
            r.is_implemented(std::path::Path::new(file), index),
            !implemented(&r, file, container, name, kind).is_empty(),
            "{container}::{name}"
        );
    }
}
