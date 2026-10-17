//! [`emptied`]: one `Vec`'s allocation for elements that borrow something else.

use std::mem::ManuallyDrop;

/// `vec`'s allocation with no elements, as a `Vec<U>`: how a buffer of borrows outlives what it
/// borrowed, so a loop over chunks allocates it once rather than once a chunk.
///
/// At each use `T` and `U` are one type but for a lifetime; a compile error holds them to one size,
/// not zero, and one alignment. That is the layout the allocation was made with, so it is the
/// layout `U`'s `Vec` frees it with, and with no element in it nothing of `U` is read before it is
/// written. The elements are dropped first, and the old `Vec` is kept from freeing the allocation.
pub(crate) fn emptied<T, U>(vec: Vec<T>) -> Vec<U> {
    const {
        assert!(
            size_of::<T>() == size_of::<U>()
                && size_of::<T>() != 0
                && align_of::<T>() == align_of::<U>(),
            "a recycled Vec keeps its element layout",
        );
    }
    let mut vec = ManuallyDrop::new(vec);
    vec.clear();
    let (pointer, capacity) = (vec.as_mut_ptr(), vec.capacity());
    unsafe { Vec::from_raw_parts(pointer.cast::<U>(), 0, capacity) }
}

#[cfg(test)]
mod tests;
