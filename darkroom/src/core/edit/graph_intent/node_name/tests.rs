use scenarium::internals::test_graph::TestGraph;

use crate::core::edit::graph_intent::node_name;

/// A free name stays; a taken one takes the smallest free number from 2, filling a gap; a
/// numbered name counts from its base; a trailing number of 0 or 1, or a name that is only a
/// number, is part of the name; an empty name stays empty, as a node with none shows what it
/// places.
#[test]
fn a_taken_name_takes_the_smallest_free_number() {
    let graph = |names: &[&str]| {
        let mut fixture = TestGraph::new();
        for name in names {
            fixture.add(name, |spec| spec);
        }
        fixture.graph().clone()
    };
    let cases: [(&[&str], &str, &str); 9] = [
        (&[], "B", "B"),
        (&["B"], "B", "B 2"),
        (&["B", "B 2"], "B", "B 3"),
        (&["B", "B 3"], "B", "B 2"),
        (&["B", "B 2"], "B 2", "B 3"),
        (&["B 2"], "B 2", "B 3"),
        (&["B 1"], "B 1", "B 1 2"),
        (&["7"], "7", "7 2"),
        (&[""], "", ""),
    ];
    for (names, wanted, expected) in cases {
        assert_eq!(
            node_name::free(&graph(names), wanted),
            expected,
            "{wanted:?} among {names:?}"
        );
    }
}
