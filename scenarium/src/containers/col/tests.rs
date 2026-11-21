use crate::containers::col::{Col, Idx, Span};

idx_type! {
    /// The tests' own index space, so the leaf's tests reach nothing above it.
    struct TestIdx, "a test index"
}

/// `iter_indexed` is what lets a walk carry the index without reconstructing it
/// from a counter, so it must agree with indexing on every entry.
#[test]
fn iter_indexed_pairs_each_entry_with_the_index_that_addresses_it() {
    let mut col = Col::<TestIdx, _>::default();
    for value in ["a", "b", "c"] {
        col.push(value);
    }

    let pairs: Vec<_> = col.iter_indexed().collect();
    assert_eq!(
        pairs,
        [
            (TestIdx::from_idx(0), &"a"),
            (TestIdx::from_idx(1), &"b"),
            (TestIdx::from_idx(2), &"c")
        ]
    );
    for (node_idx, value) in pairs {
        assert_eq!(col[node_idx], *value, "the pair addresses its own entry");
    }

    assert_eq!(col.get(TestIdx::from_idx(2)), Some(&"c"));
    assert_eq!(
        col.get(TestIdx::from_idx(3)),
        None,
        "one past the last entry"
    );
    assert!(
        Col::<TestIdx, &str>::default()
            .iter_indexed()
            .next()
            .is_none()
    );
}

/// The second way a column is addressed: an owner keeps the run `append` hands
/// back, and every way of reaching an entry through it — the whole slice, one
/// index inside it, the checked form — must name the same entries.
#[test]
fn append_hands_back_the_run_it_packed_and_every_address_into_it_agrees() {
    let mut col = Col::<TestIdx, _>::default();

    let first = col.append([10, 20]);
    let empty = col.append([]);
    let second = col.append([30]);

    assert_eq!((first.start, first.len), (0, 2));
    assert_eq!(
        (empty.start, empty.len),
        (2, 0),
        "an empty run still places"
    );
    assert_eq!((second.start, second.len), (2, 1));
    assert_eq!(first.range(), 0..2);
    assert_eq!(col[first], [10, 20]);
    assert!(col[empty].is_empty());
    assert_eq!(col[second], [30]);

    // The run's own offsets, and the scalar index each names.
    assert_eq!(first.nth(0), TestIdx::from_idx(0));
    assert_eq!(first.nth(1), TestIdx::from_idx(1));
    assert_eq!(second.nth(0), TestIdx::from_idx(2));
    assert_eq!(col[first.nth(1)], 20, "an offset addresses its entry");

    assert_eq!(col.get_span(second), Some(&[30][..]));
    assert_eq!(
        col.get_span(Span::<TestIdx>::default()),
        Some(&[][..]),
        "an empty run is in range even in an empty column"
    );

    col[first][1] = 25;
    assert_eq!(col[first], [10, 25]);
}

/// A per-run column is refilled in place: `clone_from` copies a shorter column into one with room
/// and keeps the buffer (capacity 8 stays 8, where a fresh clone holds exactly the 3 copied), and
/// `reset_with` refills every entry from `fill` without growing.
#[test]
fn a_column_refilled_from_another_keeps_its_buffer() {
    let source: Col<TestIdx, u32> = [1, 2, 3].into();
    let mut dest = Col::<TestIdx, u32>::with_capacity(8);
    dest.extend([9, 9, 9, 9, 9]);

    dest.clone_from(&source);
    assert_eq!(dest.iter().copied().collect::<Vec<_>>(), [1, 2, 3]);
    assert_eq!(dest.capacity(), 8);
    assert_eq!(source.clone().capacity(), 3);

    let mut next = 0;
    dest.reset_with(4, || {
        next += 10;
        next
    });
    assert_eq!(dest.iter().copied().collect::<Vec<_>>(), [10, 20, 30, 40]);
    assert_eq!(dest.capacity(), 8);
}

/// A span from a longer column reaches past a shorter one — the case
/// `get_span` exists for, and the one a validator meets when a column and the
/// program its spans came from disagree.
#[test]
fn a_span_past_the_end_is_none_rather_than_a_panic() {
    let mut long = Col::<TestIdx, _>::default();
    let run = long.append([1, 2, 3]);
    let mut short = Col::<TestIdx, _>::default();
    short.append([1, 2]);

    assert_eq!(long.get_span(run), Some(&[1, 2, 3][..]));
    assert_eq!(short.get_span(run), None);
}

/// A strictly ascending column finds each of its entries by binary search and nothing else, and a
/// reset refills it at the length asked, whatever it held.
#[test]
fn a_sorted_column_finds_its_entries_and_a_reset_refills_it() {
    let mut col = Col::<TestIdx, i32>::from([2, 5, 9]);
    assert!(col.is_strictly_ascending());
    assert_eq!(col.search_sorted(&2), Some(TestIdx::from_idx(0)));
    assert_eq!(col.search_sorted(&9), Some(TestIdx::from_idx(2)));
    assert_eq!(col.search_sorted(&4), None);
    assert!(Col::<TestIdx, i32>::default().is_strictly_ascending());
    assert!(
        !Col::<TestIdx, i32>::from([2, 2]).is_strictly_ascending(),
        "a repeat"
    );
    assert!(!Col::<TestIdx, i32>::from([3, 2]).is_strictly_ascending());

    col.reset(2, 7);
    assert_eq!(col.iter().copied().collect::<Vec<_>>(), [7, 7]);
    col.reset(0, 1);
    assert!(col.is_empty());

    let span = Span::<TestIdx>::new(4, 3);
    assert_eq!((span.len(), span.offsets()), (3, 0..3));
    assert_eq!(span.nth(2), TestIdx::from_idx(6));
}
