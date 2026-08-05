use crate::DataType;
use crate::graph::Graph;
use crate::graph::identity::OutputPort;
use crate::graph::output_types::OutputTypes;
use crate::testing::graph::TestGraph;

/// `src` (declared `Int`) → `p1` → `p2`, both passthroughs declaring a
/// wildcard output over an `Any` input, plus a second producer wired to
/// nothing — a fixed output lying on no wildcard chain.
fn chain() -> TestGraph {
    let mut g = TestGraph::new();
    g.add("src", |n| n.pure().output(DataType::Int));
    g.add("p1", |n| n.input(DataType::Any).wildcard(0));
    g.instance("p2", "p1");
    g.instance("isolated", "src");
    g.wire("src", 0, "p1", 0);
    g.wire("p1", 0, "p2", 0);
    g
}

/// The port `port` of the node the fixture named `name`.
fn port(g: &TestGraph, name: &str, port: usize) -> OutputPort {
    OutputPort::new(g.id(name), port)
}

/// Every declared output port is in the table, whichever kind it is: a
/// wildcard through the chain above it, a fixed output from its own
/// declaration. One lookup answers for both, which is what lets a caller
/// skip the case analysis. A port that does not exist stays out.
#[test]
fn covers_every_declared_output_port() {
    let c = chain();
    let mut types = OutputTypes::default();

    types.update(&c.graph, &c.library);

    assert_eq!(
        types.get(port(&c, "p1", 0)),
        Some(&DataType::Int),
        "the passthrough mirrors the producer it reads"
    );
    assert_eq!(
        types.get(port(&c, "p2", 0)),
        Some(&DataType::Int),
        "transitively, through the chain"
    );
    assert_eq!(
        types.get(port(&c, "src", 0)),
        Some(&DataType::Int),
        "the fixed producer answers from its own declaration"
    );
    assert_eq!(
        types.get(port(&c, "isolated", 0)),
        Some(&DataType::Int),
        "including one no wildcard chain reaches"
    );
    assert_eq!(
        types.get(port(&c, "p1", 7)),
        None,
        "a port that does not exist is not declared, so not in the table"
    );
}

/// `update` is a replacement, not an accumulation: the previous graph's
/// answers must not survive it. This is the invariant a per-frame refresh
/// relies on, and the one a stale table would break silently.
#[test]
fn update_discards_what_the_previous_graph_resolved() {
    let mut c = chain();
    let mut types = OutputTypes::default();
    types.update(&c.graph, &c.library);
    assert_eq!(types.get(port(&c, "p1", 0)), Some(&DataType::Int));

    // Same nodes, binding severed: the passthrough goes polymorphic.
    c.unbind("p1", 0);
    types.update(&c.graph, &c.library);

    assert_eq!(
        types.get(port(&c, "p1", 0)),
        Some(&DataType::Any),
        "the severed input leaves it polymorphic, not `Int` from last time"
    );
    assert_eq!(
        types.get(port(&c, "p2", 0)),
        Some(&DataType::Any),
        "and the taint reaches downstream"
    );

    // A graph with none of those nodes leaves the table empty rather than
    // answering for ids it no longer contains.
    types.update(&Graph::default(), &c.library);
    assert_eq!(types.get(port(&c, "p1", 0)), None);
}

/// A binding cycle is *present* and `Any` — distinguishable from a port the
/// table never reached, which is `None`.
#[test]
fn a_cycle_resolves_to_any_rather_than_being_absent() {
    let mut c = chain();
    // p1 ← p2 ← p1: the chain now closes on itself.
    c.wire("p2", 0, "p1", 0);
    let mut types = OutputTypes::default();

    types.update(&c.graph, &c.library);

    assert_eq!(types.get(port(&c, "p1", 0)), Some(&DataType::Any));
    assert_eq!(types.get(port(&c, "p2", 0)), Some(&DataType::Any));
}
