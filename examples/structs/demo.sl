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
