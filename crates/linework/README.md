# linework

**An engine-neutral protocol for binding language intelligence to a code
editor.** Line-keyed, two-wave code paint plus on-demand facts, behind one
trait an editor widget can be generic over.

Zero dependencies. This crate knows nothing about any particular parser,
language runtime, or editor.

```toml
[dependencies]
linework = "0.0.1"
```

## What it is for

An editor that wants syntax colouring, go-to-definition and inline hints
normally couples itself to whatever produces them. Swap the engine and the
widget is rewritten; support a second language and the widget grows a second
integration.

`linework` is the seam. One side implements [`Limner`]; the other consumes
it. Neither needs the other's types:

```
   your editor widget  ──►  dyn Limner  ◄──  your intelligence engine
      (consumes)                                  (implements)
```

## The shape, and why

**Paint is line-keyed.** Each line is a vector of `Run`s — 4-byte cells
`{len, style, mods}` tiling that line's bytes exactly. Runs are
line-*relative*, so a line that merely shifts down re-emits nothing. Editing
line 3 of a 10,000-line file does not repaint 9,997 unchanged lines.

**Paint is two-wave.** `style` is the lexical class, available synchronously
with a keystroke from the damaged lines alone. `mods` carries semantic bits —
definition, resolved or unresolved reference, exported, foreign, typed — that
arrive later and **never change a style**. Refinement is strictly additive, so
the first frame is already correct and nothing flickers as understanding
deepens.

**A delta names only what changed.** A `PaintDelta` carries a spliced window
of lines plus semantic-only repaints by line number. That repaint set is the
edit's semantic blast radius, made explicit rather than guessed at.

**Facts are lookups, not requests.** `FactCard` answers "what *is* this name"
— definition site, problem, display type, namespace — against warm state.
`Hint`s carry inline decorations.

## The trait

```rust
pub trait Limner {
    fn open(&mut self, text: &str) -> Paint;
    fn edit(&mut self, edit: &LineEdit) -> PaintDelta;
    fn facts(&mut self, offset: u32) -> Option<FactCard>;
    fn hints(&mut self) -> Vec<Hint>;
    fn legend(&self) -> Vec<String>;
    fn text(&mut self) -> String;
}
```

`legend()` names the style classes by index, so the editor maps indices to
its own theme and the engine never speaks in colours.

## Conventions

- **Offsets and lengths are bytes** of UTF-8 text. Consumers that index by
  character (ropes, for instance) convert at the boundary.
- **The wire form is the memory form**, little-endian — see `encode_lines` /
  `decode_lines`. Decoding is bounds checks, not parsing, so the protocol
  crosses a process boundary without a serialisation format.

## Status

Pre-1.0 and evolving. The types are small and the surface is deliberately
narrow, but expect breaking changes before 1.0. Pin an exact version.

## Licence

MIT OR Apache-2.0, at your option.
