# ChartLang — rantlr demo extension

1. Build the server: `cargo build --release -p rantlr-lsp` (repo root).
2. `npm install` in this directory (fetches vscode-languageclient).
3. Open this directory in VS Code and press F5 (Extension Development
   Host), then open `examples/playground/` from the repo.

The playground: open `demo.cl` and `chartlang.rg` side by side. The
`.rg` file is the ENTIRE language definition — tokens, keywords, modes,
precedence, productions, binding and style annotations — and it is
itself served by rantlr (highlighting, outline of rules and tokens,
go-to-definition on rule names including forward references, live
envelope diagnostics as you type). Edit it — add a keyword, a token, a
whole production — and on save the certified pipeline rebuilds in
milliseconds and open documents re-colorize. Break it (e.g. delete the
`prec` lines) and the envelope refuses it: the conflict counterexample
appears as an error ON THE OFFENDING PRODUCTION while the last good
language stays live.

Legacy `chartlang.toml` configs (keywords + precedence only) are still
supported when no `chartlang.rg` is present.
