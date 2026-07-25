# lib.ml — exports `scale` and `base`; keeps `secret` file-private.

pub fn scale(factor: Num) -> Num {
  return factor + secret;
}

pub let base = 10;

let secret = 32;
