# app.ml — imports from lib.ml. Try `use secret;` — the diagnostic says
# it EXISTS but is not exported (different from a typo's "cannot find").

use secret;
use base as start;

let width = scale(start) + 2;
