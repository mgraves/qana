// ChartLang playground — served by rantlr-lsp.
// Everything you see derives from one grammar value:
// highlighting, folding, outline, completion, diagnostics.

let alpha = 1 + 2 * 3;
let beta = (alpha - 1) / 2;

if (alpha) {
  emit("nested", [1, 2.5]);
  let inner = beta;
} else {
  skip();
}

/* multi-line comments
   fold and survive edits
   without breaking anything below */

let tail = done(alpha, beta);
