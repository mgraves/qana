# app.ml — imports from lib.ml. Try `use secret;` — the diagnostic says
# it EXISTS but is not exported (different from a typo's "cannot find").

use scale;
use base as start;

let width = scale(start) + 2;

use math;
let area = math::pi + width;
