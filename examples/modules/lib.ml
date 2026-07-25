# lib.ml — exports `scale` and `base`; keeps `secret` file-private.

pub fn scale(factor: Num) -> Num {
  return factor + secret;
}

pub let base = 10;

let secret = 32;

pub mod math {
  pub let pi = 3;
  let tau_hidden = 6;
}
