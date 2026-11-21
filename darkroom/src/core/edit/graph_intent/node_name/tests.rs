use scenarium::internals::test_graph::TestGraph;

use crate::core::edit::graph_intent::node_name;

/// A free name stays; one a node reads as its title, its own or its func's, takes the smallest
/// free number from 2, filling a gap; a numbered name counts from its base; a trailing number of
/// 0 or 1, or a name that is only a number, is part of the name.
#[test]
fn a_taken_name_takes_the_smallest_free_number() {
    let graph = |names: &[&str]| {
        let mut fixture = TestGraph::new();
        for name in names {
            fixture.add(name, |spec| spec);
        }
        fixture
    };
    let cases: [(&[&str], &str, &str); 8] = [
        (&[], "B", "B"),
        (&["B"], "B", "B 2"),
        (&["B", "B 2"], "B", "B 3"),
        (&["B", "B 3"], "B", "B 2"),
        (&["B", "B 2"], "B 2", "B 3"),
        (&["B 2"], "B 2", "B 3"),
        (&["B 1"], "B 1", "B 1 2"),
        (&["7"], "7", "7 2"),
    ];
    for (names, wanted, expected) in cases {
        let fixture = graph(names);
        assert_eq!(
            node_name::free(fixture.graph(), fixture.scope(), wanted),
            expected,
            "{wanted:?} among {names:?}"
        );
    }
}
