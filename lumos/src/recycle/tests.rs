use std::cell::Cell;

use crate::recycle;

/// Counts its drops into the cell it borrows.
#[derive(Debug)]
struct Dropped<'a>(&'a Cell<usize>);

impl Drop for Dropped<'_> {
    fn drop(&mut self) {
        self.0.set(self.0.get() + 1);
    }
}

/// The allocation moves to the new lifetime whole: its address and capacity stay, nothing is
/// left in it, each old element drops exactly once, and the recycled `Vec` takes new elements
/// into the same allocation.
#[test]
fn emptied_keeps_the_allocation_and_drops_each_element_once() {
    let first = Cell::new(0);
    let mut old: Vec<Dropped<'_>> = Vec::with_capacity(8);
    old.extend((0..3).map(|_| Dropped(&first)));
    let (pointer, capacity) = (old.as_ptr().cast::<()>(), old.capacity());

    let second = Cell::new(0);
    let mut recycled: Vec<Dropped<'_>> = recycle::emptied(old);
    assert_eq!(first.get(), 3, "each old element dropped once");
    assert_eq!(recycled.len(), 0);
    assert_eq!(recycled.capacity(), capacity);
    assert_eq!(recycled.as_ptr().cast::<()>(), pointer);

    recycled.extend((0..8).map(|_| Dropped(&second)));
    assert_eq!(recycled.as_ptr().cast::<()>(), pointer, "refilled in place");
    drop(recycled);
    assert_eq!((first.get(), second.get()), (3, 8));
}
