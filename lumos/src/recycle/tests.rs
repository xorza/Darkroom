use std::cell::Cell;

use crate::recycle;
use crate::recycle::recycled::Recycled;

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

/// A `Recycled` lends the allocation it was given back, empty, and takes it again: a `Vec` put
/// once is the `Vec` taken next, its address and capacity kept.
#[test]
fn a_recycled_vec_lends_the_allocation_it_keeps() {
    let cell = Cell::new(0);
    let mut kept: Recycled<Dropped<'static>> = Recycled::default();
    let mut vec: Vec<Dropped<'_>> = kept.take();
    assert_eq!(vec.capacity(), 0, "nothing kept yet");
    vec.extend((0..5).map(|_| Dropped(&cell)));
    let (pointer, capacity) = (vec.as_ptr().cast::<()>(), vec.capacity());
    kept.put(vec);
    assert_eq!(cell.get(), 5, "putting drops the elements");
    let again: Vec<Dropped<'_>> = kept.take();
    assert_eq!(again.len(), 0);
    assert_eq!(
        (again.as_ptr().cast::<()>(), again.capacity()),
        (pointer, capacity)
    );
}
