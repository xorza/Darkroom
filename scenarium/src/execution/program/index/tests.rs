use crate::execution::program::index::{NodeColumn, NodeIdx, NodeSet};

/// 70 nodes span two words, so this covers the low word, both sides of the
/// 64-bit boundary, and a bit inside the trailing partial word.
fn spanning_two_words() -> NodeSet {
    let mut set = NodeSet::default();
    set.reset(70);
    for i in [0, 63, 64, 69] {
        set.insert(NodeIdx(i));
    }
    set
}

#[test]
fn membership_and_iteration_cross_the_word_boundary() {
    let set = spanning_two_words();

    for i in [0, 63, 64, 69] {
        assert!(set.contains(NodeIdx(i)), "{i} was inserted");
    }
    // 62/65 neighbour the boundary bits; 1 and 68 sit inside each word's span.
    for i in [1, 62, 65, 68] {
        assert!(!set.contains(NodeIdx(i)), "{i} was never inserted");
    }

    assert_eq!(
        set.iter().collect::<Vec<_>>(),
        [NodeIdx(0), NodeIdx(63), NodeIdx(64), NodeIdx(69)],
        "iteration is ascending across words, not word-local"
    );
}

/// `reset` reuses the buffer, so a stale word from the previous program must not
/// survive into the next one — including when the node count shrinks and regrows.
#[test]
fn reset_clears_every_word_and_resizes() {
    let mut set = spanning_two_words();

    set.reset(3);
    assert_eq!(set.iter().count(), 0, "a shrunk reset drops the high word");
    assert!(!set.contains(NodeIdx(0)), "and clears the low word's bits");

    set.reset(70);
    assert_eq!(
        set.iter().count(),
        0,
        "regrowing past the old length reveals no stale bits"
    );

    set.insert(NodeIdx(64));
    assert_eq!(set.iter().collect::<Vec<_>>(), [NodeIdx(64)]);
}

/// `iter_indexed` is what lets a walk carry the index without reconstructing it
/// from a counter, so it must agree with indexing on every entry.
#[test]
fn iter_indexed_pairs_each_entry_with_the_index_that_addresses_it() {
    let mut column = NodeColumn::default();
    for value in ["a", "b", "c"] {
        column.push(value);
    }

    let pairs: Vec<_> = column.iter_indexed().collect();
    assert_eq!(
        pairs,
        [(NodeIdx(0), &"a"), (NodeIdx(1), &"b"), (NodeIdx(2), &"c")]
    );
    for (node_idx, value) in pairs {
        assert_eq!(column[node_idx], *value, "the pair addresses its own entry");
    }

    assert_eq!(column.get(NodeIdx(2)), Some(&"c"));
    assert_eq!(column.get(NodeIdx(3)), None, "one past the last entry");
    assert!(
        NodeColumn::<&str>::default()
            .iter_indexed()
            .next()
            .is_none()
    );
}

/// Word-granular indexing would silently accept an index landing in the last
/// word's padding; the length check is what keeps that a panic.
#[test]
#[should_panic(expected = "node index out of range")]
#[cfg(debug_assertions)]
fn insert_past_the_length_panics_inside_the_trailing_word() {
    let mut set = NodeSet::default();
    // 3 bits of a 64-bit word: index 40 is in-bounds for the word, not the set.
    set.reset(3);
    set.insert(NodeIdx(40));
}
