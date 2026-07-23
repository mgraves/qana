# ChartLang — rantlr demo extension

1. Build the server: `cargo build --release -p rantlr-lsp` (repo root).
2. `npm install` in this directory (fetches vscode-languageclient).
3. Open this directory in VS Code and press F5 (Extension Development
   Host), then open `examples/playground/` from the repo.

The playground: open `demo.cl` and `chartlang.toml` side by side. Edit
the config — add a keyword, flip an associativity — and watch the open
documents re-colorize live (the whole certified pipeline rebuilds in
milliseconds). Break the config (e.g. remove `prec.left.2 = * /`) and
the envelope refuses it: the conflict counterexample appears as an error
on chartlang.toml while the last good language stays live.
