use std::panic;
use std::panic::AssertUnwindSafe;

use common::Buffer2;

use crate::internals;
use crate::scratch_pool::{Scratch, ScratchPool, Shapeless};
use common::USize2;

const SMALL: USize2 = USize2 {
    width: 8,
    height: 4,
};
const LARGE: USize2 = USize2 {
    width: 16,
    height: 4,
};

/// A vector whose shape is its length, built and fitted by resizing: what makes a reused value
/// distinguishable from a fresh one is the fill it kept.
#[derive(Debug)]
struct Row(Vec<u8>);

impl Scratch for Row {
    type Shape = usize;

    fn build(shape: usize) -> Self {
        Self(vec![0; shape])
    }

    fn fit(&mut self, shape: usize) {
        self.0.resize(shape, 0);
    }

    fn has_shape(&self, shape: usize) -> bool {
        self.0.len() == shape
    }
}

/// An acquire from an empty pool builds; a dropped lease is shelved and the next acquire hands
/// back that same allocation, contents and all; two leases at once are two allocations.
#[test]
fn a_dropped_lease_is_the_next_acquire() {
    let pool: ScratchPool<Buffer2<f32>> = ScratchPool::new(SMALL);
    assert_eq!(pool.pooled(), 0, "nothing is built up front");

    let mut first = pool.acquire();
    assert_eq!((first.width(), first.height()), (8, 4));
    first.pixels_mut()[0] = 7.0;
    let address = first.as_ptr();
    drop(first);
    assert_eq!(pool.pooled(), 1);

    let again = pool.acquire();
    assert_eq!(again.as_ptr(), address);
    assert_eq!(
        again.pixels()[0],
        7.0,
        "a reused value keeps what its holder left"
    );
    let fresh = pool.acquire();
    assert_ne!(fresh.as_ptr(), address);
    assert_eq!(pool.pooled(), 0);
    drop((again, fresh));
    assert_eq!(pool.pooled(), 2);
}

/// A reused value is fitted to the pool's shape, a fresh one built at it: `Row`'s fit keeps
/// the first `min(old, new)` bytes, so the kept fill tells the two apart.
#[test]
fn acquire_builds_and_fits_at_the_pool_shape() {
    let pool: ScratchPool<Row> = ScratchPool::new(3);
    let mut row = pool.acquire();
    assert_eq!(row.0, [0, 0, 0]);
    row.0.fill(9);
    drop(row);
    assert_eq!(pool.acquire().0, [9, 9, 9]);
}

/// A change of shape empties the shelf, and a lease taken before it is dropped on return
/// rather than shelved at the new shape; the same shape again changes nothing.
#[test]
fn reshape_drops_free_and_outstanding_values() {
    let mut pool: ScratchPool<Buffer2<f32>> = ScratchPool::new(SMALL);
    drop(pool.acquire());
    let outstanding = pool.acquire();
    drop(pool.acquire());
    assert_eq!(pool.pooled(), 1);

    pool.reshape(SMALL);
    assert_eq!(pool.pooled(), 1, "the same shape keeps the shelf");

    pool.reshape(LARGE);
    assert_eq!(pool.shape(), LARGE);
    assert_eq!(pool.pooled(), 0);
    drop(outstanding);
    assert_eq!(pool.pooled(), 0, "a value of the old shape is not shelved");

    let large = pool.acquire();
    assert_eq!((large.width(), large.height()), (16, 4));
}

/// A holder that swapped in a value of another shape broke the lease: caught on return, and
/// never shelved, also when the lease drops in an unwind that is already under way.
#[test]
fn a_value_of_another_shape_is_never_shelved() {
    let pool: ScratchPool<Buffer2<f32>> = ScratchPool::new(SMALL);
    let mut lease = pool.acquire();
    *lease = Buffer2::new_default(LARGE);
    let payload = panic::catch_unwind(AssertUnwindSafe(|| drop(lease))).unwrap_err();
    assert_eq!(
        internals::panic_message(&*payload),
        "pooled value returned at a shape other than the one it was leased at"
    );
    assert_eq!(pool.pooled(), 0);

    let payload = panic::catch_unwind(AssertUnwindSafe(|| {
        let mut lease = pool.acquire();
        *lease = Buffer2::new_default(LARGE);
        panic!("the holder's own failure");
    }))
    .unwrap_err();
    assert_eq!(
        internals::panic_message(&*payload),
        "the holder's own failure"
    );
    assert_eq!(pool.pooled(), 0);
}

/// A lease holds its share of the pool, so it outlives the pool handle it came from.
#[test]
fn a_lease_outlives_its_pool_handle() {
    let pool: ScratchPool<Row> = ScratchPool::default();
    let lease = pool.acquire();
    drop(pool);
    assert!(lease.0.is_empty());
}

/// `clear` drops the free values, so the next acquire builds afresh.
#[test]
fn clear_takes_values_out_of_the_pool() {
    let mut pool: ScratchPool<Row> = ScratchPool::new(2);
    let mut returned = pool.acquire();
    returned.0.fill(6);
    drop(returned);
    assert_eq!(pool.pooled(), 1);
    pool.clear();
    assert_eq!(pool.pooled(), 0);
    assert_eq!(pool.acquire().0, [0, 0], "built afresh after the clear");
}

/// A `Shapeless` type pools at the unit shape: built by `Default`, reused as it was left.
#[test]
fn shapeless_scratch_is_reused_as_it_was_left() {
    #[derive(Debug, Default)]
    struct Notes(Vec<u8>);
    impl Shapeless for Notes {}

    let pool: ScratchPool<Notes> = ScratchPool::default();
    let mut notes = pool.acquire();
    assert!(notes.0.is_empty());
    notes.0.extend([1, 2, 3]);
    drop(notes);
    assert_eq!(pool.acquire().0, [1, 2, 3]);
}
