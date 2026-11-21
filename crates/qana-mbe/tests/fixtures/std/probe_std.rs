use std::cell::{Cell, RefCell};

thread_local! {
    /// The count.
    static COUNT: Cell<u32> = Cell::new(0);
    #[allow(unused)]
    pub static NAME: RefCell<String> = const { RefCell::new(String::new()) };
    pub(crate) static LIST: RefCell<Vec<u8>> = RefCell::new(vec![1, 2]);
}

fn main() {
    let v = vec![1, 2, 3];
    let w = vec![0u8; 4];
    let e: Vec<u8> = vec![];
    let t = vec![(1, "a"), (2, "b"),];
    assert_eq!(v.len(), 3);
    assert_eq!(w.len(), 4, "w has {} items", w.len());
    assert_eq!(e.len() + 1, 1,);
    assert_eq!(t[0].0, 1, "{:?}", t);
    COUNT.with(|c| c.set(1));
}
