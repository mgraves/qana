fn f() {
    {
        macro_rules! a { () => { 1 } }
        let _ = a!();
    }
    let _ = a!();
}
mod m { macro_rules! b { () => { 2 } } }
#[macro_use]
mod n { macro_rules! c { () => { 3 } } }
fn g() { let _ = b!(); let _ = c!(); }
fn main() {}
