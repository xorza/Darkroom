use crate::containers::column::{Column, Span};
use crate::execution::identity::NodeIdx;

/// `iter_indexed` is what lets a walk carry the index without reconstructing it
/// from a counter, so it must agree with indexing on every entry.
#[test]
fn iter_indexed_pairs_each_entry_with_the_index_that_addresses_it() {
    let mut column = Column::<NodeIdx, _>::default();
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
        Column::<NodeIdx, &str>::default()
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
    let mut column = Column::<NodeIdx, _>::default();

    let first = column.append([10, 20]);
    let empty = column.append([]);
    let second = column.append([30]);

    assert_eq!((first.start, first.len), (0, 2));
    assert_eq!(
        (empty.start, empty.len),
        (2, 0),
        "an empty run still places"
    );
    assert_eq!((second.start, second.len), (2, 1));
    assert_eq!(first.range(), 0..2);
    assert_eq!(column[first], [10, 20]);
    assert!(column[empty].is_empty());
    assert_eq!(column[second], [30]);

    // The run's own offsets, and the scalar index each names.
    assert_eq!(first.nth(0), NodeIdx(0));
    assert_eq!(first.nth(1), NodeIdx(1));
    assert_eq!(second.nth(0), NodeIdx(2));
    assert_eq!(column[first.nth(1)], 20, "an offset addresses its entry");

    assert_eq!(column.get_span(second), Some(&[30][..]));
    assert_eq!(
        column.get_span(Span::<NodeIdx>::default()),
        Some(&[][..]),
        "an empty run is in range even in an empty column"
    );

    column[first][1] = 25;
    assert_eq!(column[first], [10, 25]);
}

/// A span from a longer column reaches past a shorter one — the case
/// `get_span` exists for, and the one a validator meets when a column and the
/// program its spans came from disagree.
#[test]
fn a_span_past_the_end_is_none_rather_than_a_panic() {
    let mut long = Column::<NodeIdx, _>::default();
    let run = long.append([1, 2, 3]);
    let mut short = Column::<NodeIdx, _>::default();
    short.append([1, 2]);

    assert_eq!(long.get_span(run), Some(&[1, 2, 3][..]));
    assert_eq!(short.get_span(run), None);
}
