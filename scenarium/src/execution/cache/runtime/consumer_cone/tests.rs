use super::*;
use crate::DataType;
use crate::graph::identity::NodeId;
use crate::testing::graph::TestGraph;
use crate::testing::graph::compiled::Compiled;

/// The shared source → sink + unwired `loose` graph, compiled — the three
/// positions a seed can occupy relative to a consumer edge.
fn fixture() -> Compiled {
    TestGraph::source_sink_loose().compile()
}

/// The cone back in the fixture's own names, so an assertion says which nodes
/// were reached rather than wherever the id sort happened to place them.
fn reached<'n>(cone: &mut ConsumerCone, f: &'n Compiled, seeds: &[&str]) -> Vec<&'n str> {
    seeded(cone, f, seeds.iter().map(|name| f.id(name)).collect())
}

/// [`reached`] for the seeds a fixture spells as raw ids — the one case a name
/// cannot express, since the point is an id the program never held.
fn seeded<'n>(cone: &mut ConsumerCone, f: &'n Compiled, seeds: Vec<NodeId>) -> Vec<&'n str> {
    cone.of(
        &f.program,
        seeds.iter().filter_map(|node_id| f.program.node(*node_id)),
    )
    .iter()
    .map(|node_idx| f.name(node_idx))
    .collect()
}

/// A seed reaches everything downstream of it, reflexively — and stops at a
/// node nothing connects to.
#[test]
fn the_cone_reaches_downstream_and_stops() {
    let f = fixture();
    let cone = &mut ConsumerCone::default();
    assert_eq!(
        reached(cone, &f, &["source"]),
        ["source", "sink"],
        "a source reaches its reader"
    );
    assert_eq!(
        reached(cone, &f, &["sink"]),
        ["sink"],
        "a terminal node reaches only itself"
    );
    assert_eq!(
        reached(cone, &f, &["loose"]),
        ["loose"],
        "an unwired node reaches only itself"
    );
    assert!(
        seeded(cone, &f, vec![NodeId::unique()]).is_empty(),
        "an id the program never held seeds nothing"
    );
}

/// The set is the visited mark, so a seed named twice — and a seed already
/// reachable from another — appears exactly once.
#[test]
fn repeated_and_overlapping_seeds_are_visited_once() {
    let f = fixture();
    let cone = &mut ConsumerCone::default();
    assert_eq!(reached(cone, &f, &["source", "source"]), ["source", "sink"]);
    assert_eq!(reached(cone, &f, &["source", "sink"]), ["source", "sink"]);
}

/// Ascending indices out, and the walk assigns them in id order — so the
/// caller reads the cone in a stable order across compiles of one graph.
#[test]
fn the_cone_walks_the_dense_space_in_order() {
    let f = fixture();
    let cone = &mut ConsumerCone::default();
    let all = cone.of(
        &f.program,
        ["source", "sink", "loose"].map(|name| f.idx(name)),
    );
    assert_eq!(all.iter().count(), 3);
    assert!(all.iter().is_sorted());
}

/// Reuse is the point of holding one: every buffer is refilled per call, so
/// a second query over a *different* program cannot inherit the first's
/// edges, run lengths, or visited marks.
#[test]
fn a_reused_cone_answers_each_program_from_scratch() {
    let f = fixture();
    let cone = &mut ConsumerCone::default();

    assert_eq!(reached(cone, &f, &["source"]), ["source", "sink"]);

    // A second, *smaller* program with no edge at all — the shrink is the
    // case leftover runs and marks would survive into. The source must now
    // reach only itself.
    let mut unwired = TestGraph::new();
    unwired.add("source", |n| n.pure().output(DataType::Int));
    unwired.add("sink", |n| n.records());
    let unwired = unwired.compile();
    assert_eq!(reached(cone, &unwired, &["source"]), ["source"]);

    // And back to the wired one, which must answer as it did the first time.
    assert_eq!(reached(cone, &f, &["source"]), ["source", "sink"]);
}
