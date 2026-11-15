//! A sample of Rust chosen for its lexing, not its meaning: every token
//! of `rust_lex.qana`, and the places where one token is easily taken for
//! another. `sample.classes` records how rustc_lexer reads this file, and
//! `crates/qana-lang/tests/rust_lex.rs` holds qana to that record.
//!
//! Nothing compiles this file. After changing it, write the record again:
//!
//!     cargo run --manifest-path tools/rust-fit/Cargo.toml -- record

#![allow(dead_code, unused)]

use std::collections::{BTreeMap, HashMap};

/// Numbers: where a `.` belongs to the number and where it does not.
fn numbers() {
    let range = 0..10;
    let inclusive = 1..=2;
    let open = 3..;
    let method = 1.max(2);
    let float = 1.0;
    let bare = 2.;
    let sum = 2. + 3.;
    let call = f(1., 2.);
    let exponent = 1e10 + 1E-3 + 2.5e+7;
    let suffixed = 1u8 + 0xFF_u32 + 0b1010_1010 + 0o777 + 1_000.5e-3f64;
    let hex = 0xDEAD_BEEFusize;
    let tuple = pair.0.1;
    let spaced = pair.0 .1;
    let field = 1.e;
    let under = 1._x;
    let long = 123_456_789i64 - 1f32 as i64;
    let empty_exponent = 1e;
    let empty_digits = 0x;
    for i in 0..n.len() {}
    let slice = &v[1..];
    let whole = &v[..];
}

/// A quote starts a char literal or a lifetime.
fn quotes<'a, 'b: 'a, T: 'static>(x: &'a str, y: &'b T) -> &'a str {
    let c = 'c';
    let escaped = '\n';
    let quote = '\'';
    let backslash = '\\';
    let unicode = '\u{1F980}';
    let crab = '🦀';
    let byte = b'x';
    let byte_escape = b'\\';
    let byte_quote = b'\'';
    let digit = '9';
    let space = ' ';
    'outer: loop {
        break 'outer;
    }
    let r#type = 'r';
    let raw_lifetime: &'r#try str = x;
    let anonymous: &'_ str = x;
    let suffixed = 'a'u8;
    x
}

/// Strings, and what their prefixes are when no quote follows.
fn strings() {
    let plain = "a string with \"quotes\" and a \\ backslash";
    let bytes = b"bytes \x00\xff";
    let c_string = c"for C";
    let raw = r"raw \ no escapes";
    let raw_one = r#"one hash, so a "quote" is text"#;
    let raw_two = r##"two: "# does not close it"##;
    let raw_three = br###"three: "## still open"###;
    let raw_c = cr#"raw C"#;
    let suffixed = "text"suffix;
    let b = 1;
    let r = 2;
    let br = b + r;
    let c = r#match;
    let continued = "a string that goes on \
        to the next line, and has // no comment in it \
        and ends here";
    let across = "this one
    holds a line break, /* not a comment */
    and 'x' is not a char";
    let raw_across = r#"
        A raw string is for text like this: "quoted", \n unescaped,
        with a lone " in it. A quote and a hash close it: "#;
    let after = 1..2;
    let json = r##"
        { "key": "value #1", "n": 1.5 }
        "# is text here, and so is 'a
    "##;
    let empty = "";
    let empty_raw = r#""#;
    let unicode = "größe 名前 🦀";
}

/* A block comment. */
/* One that /* nests /* twice */ and */ comes back. */
/*
 * Across lines: "not a string", 'a not a lifetime, 1..2 not a range.
 * /* nested
 *    across lines */
 */
fn comments() -> u8 /* between tokens */ {
    // A line comment with /* no block in it.
    let x = 1; // trailing
    let y = 2 /* inline */ + 3;
    /** A doc block. */
    /*! An inner doc block. */
    /**/
    /***/
    x / y
}

/// Punctuation comes one character at a time; the parser joins it.
fn punctuation<T>(v: Vec<Vec<T>>, m: HashMap<u8, Vec<Option<T>>>) -> Option<T>
where
    T: Clone + ?Sized,
{
    let mut a = 1 << 2 >> 1;
    a <<= 1;
    a >>= 1;
    a += 1; a -= 1; a *= 2; a /= 2; a %= 3; a ^= 1; a |= 2; a &= 3;
    let b = a == 1 || a != 2 && !(a <= 3) || a >= 4;
    let closure = |x: u8| -> u8 { x };
    let path = ::std::mem::size_of::<T>();
    let turbofish = "1".parse::<u8>()?;
    let reference = &&v;
    let pointer = &raw const a;
    let unit = ();
    let array = [0u8; 4];
    #[cfg(test)]
    let attribute = 1;
    match a {
        0 | 1 => {}
        2..=9 => {}
        n @ 10.. => {}
        _ => {}
    }
    None
}

macro_rules! each {
    ($($name:ident => $value:expr),* $(,)?) => {
        $( let $name = $value; )*
    };
}

fn größe(名前: u8) -> u8 {
    let übung = 名前 + 1;
    übung
}

struct Point { x: f32, y: f32 }

impl Point {
    const ORIGIN: Point = Point { x: 0., y: 0. };
    fn scale(&self, by: f32) -> Point {
        Point { x: self.x * by, y: self.y * 2.0e0 }
    }
}

async fn keywords() -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
    let r#async = r#await;
    let value = unsafe_free().await?;
    Ok(())
}
