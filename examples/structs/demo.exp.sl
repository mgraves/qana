# Structlang: the grammar next to this file declares Num and Str; the
# `struct` declarations BELOW extend the type vocabulary at document
# level. Try: change `origin` to `new Point` on the last line, or give
# `scale` a Str argument.

struct Point {
  x: Num,
  y: Num
}

struct Label {
  text: Str
}

fn scale(factor: Num) -> Num {
  return factor + 1;
}

let origin: Point = new Point;
let tag: Label = new Label;
let width: Num = scale(2) + 3;
let copy: Point = origin;
let ox: Num = origin.x;
let lbl: Str = tag.text;

# REFLECTION: the macro's body repeats once per declared member of the
# reflected struct — `f` is each member's NAME (a member position),
# `t` its declared TYPE (an ordinary ref position) — joined by the
# grammar-declared " + ". Iterate a struct's fields by writing the
# per-field expression once.
macro coords(f, t) => { origin.f }
let span: Num = origin.x + origin.y;

struct Wrap {
  p: Point,
  q: Point
}

fn probe(pt: Point) -> Num {
  return 1;
}

macro spawns(f, t) => { probe(new t) }
let grown: Num = probe(new Point) + probe(new Point);

# A richer FACET list: `m!!{T}` binds each member's name, type, and
# INDEX, so the derive can number what it generates.
macro tagged(f, t, i) => { origin.f + i }
let tags: Num = origin.x + 0 + (origin.y + 1);
