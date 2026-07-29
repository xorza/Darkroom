use crate::common::column::Column;
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
