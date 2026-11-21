use crate::containers::col::{Idx, idx_type};
use crate::containers::idx_set::IdxSet;

idx_type! {
    /// The tests' own index space, so the leaf's tests reach nothing above it.
    struct TestIdx, "a test index"
}

/// 70 nodes span two words, so this covers the low word, both sides of the
/// 64-bit boundary, and a bit inside the trailing partial word.
fn spanning_two_words() -> IdxSet<TestIdx> {
    let mut set = IdxSet::default();
    set.reset(70);
    for i in [0, 63, 64, 69] {
        set.insert(TestIdx::from_idx(i));
    }
    set
}

#[test]
fn membership_and_iteration_cross_the_word_boundary() {
    let set = spanning_two_words();

    for i in [0, 63, 64, 69] {
        assert!(set.contains(TestIdx::from_idx(i)), "{i} was inserted");
    }
    // 62/65 neighbour the boundary bits; 1 and 68 sit inside each word's span.
    for i in [1, 62, 65, 68] {
        assert!(
            !set.contains(TestIdx::from_idx(i)),
            "{i} was never inserted"
        );
    }

    assert_eq!(
        set.iter().collect::<Vec<_>>(),
        [
            TestIdx::from_idx(0),
            TestIdx::from_idx(63),
            TestIdx::from_idx(64),
            TestIdx::from_idx(69)
        ],
        "iteration is ascending across words, not word-local"
    );
    assert!((&set).into_iter().eq(set.iter()));

    // A full word yields every bit once, the top one included.
    let mut full = IdxSet::default();
    full.reset(64);
    for i in 0..64 {
        full.insert(TestIdx::from_idx(i));
    }
    assert!(full.iter().eq((0..64).map(TestIdx::from_idx)));
}

/// `reset` reuses the buffer, so a stale word from the previous program must not
/// survive into the next one — including when the node count shrinks and regrows.
#[test]
fn reset_clears_every_word_and_resizes() {
    let mut set = spanning_two_words();

    set.reset(3);
    assert_eq!(set.iter().count(), 0, "a shrunk reset drops the high word");
    assert!(
        !set.contains(TestIdx::from_idx(0)),
        "and clears the low word's bits"
    );

    set.reset(70);
    assert_eq!(
        set.iter().count(),
        0,
        "regrowing past the old length reveals no stale bits"
    );

    set.insert(TestIdx::from_idx(64));
    assert_eq!(set.iter().collect::<Vec<_>>(), [TestIdx::from_idx(64)]);

    // A set of no indices holds no word, and iterates nothing.
    set.reset(0);
    assert_eq!(set.iter().count(), 0);
    assert!(IdxSet::<TestIdx>::default().iter().next().is_none());
}

/// Word-granular indexing would silently accept an index landing in the last
/// word's padding; the length check is what keeps that a panic.
#[test]
#[should_panic(expected = "index out of range")]
#[cfg(debug_assertions)]
fn insert_past_the_length_panics_inside_the_trailing_word() {
    let mut set = IdxSet::default();
    // 3 bits of a 64-bit word: index 40 is in-bounds for the word, not the set.
    set.reset(3);
    set.insert(TestIdx::from_idx(40));
}
