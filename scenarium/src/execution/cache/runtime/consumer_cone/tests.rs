use super::*;
use crate::execution::compile::Compiler;
use crate::graph::identity::{InputPort, NodeId};
use crate::graph::{Binding, Graph};
use crate::testing::{TestFuncHooks, test_func_lib};

/// A sink reading a source, plus an unwired node — the three positions a
/// seed can occupy relative to a consumer edge.
struct Fixture {
    program: CompiledGraph,
    source: NodeId,
    sink: NodeId,
    loose: NodeId,
}

fn fixture() -> Fixture {
    let library = test_func_lib(TestFuncHooks::default());
    let mut graph = Graph::default();
    let source = graph.add_func_node(library.by_name("get_a").unwrap());
    let sink = graph.add_func_node(library.by_name("Print").unwrap());
    let loose = graph.add_func_node(library.by_name("get_b").unwrap());
    graph.set_input_binding(InputPort::new(sink, 0), Binding::bind(source, 0));
    Fixture {
        program: Compiler::default().compile(&graph, &library).unwrap(),
        source,
        sink,
        loose,
    }
}

/// The cone back in the id space, so an assertion names nodes rather than
/// wherever the id sort happened to place them.
fn reached(cone: &mut ConsumerCone, program: &CompiledGraph, seeds: &[NodeId]) -> Vec<NodeId> {
    cone.of(
        program,
        seeds.iter().filter_map(|node_id| program.node(*node_id)),
    )
    .iter()
    .map(|node_idx| program.node_ids[node_idx])
    .collect()
}

/// A seed reaches everything downstream of it, reflexively — and stops at a
/// node nothing connects to.
#[test]
fn the_cone_reaches_downstream_and_stops() {
    let f = fixture();
    let cone = &mut ConsumerCone::default();
    let mut expected = vec![f.source, f.sink];
    expected.sort();
    assert_eq!(
        reached(cone, &f.program, &[f.source]),
        expected,
        "a source reaches its reader"
    );
    assert_eq!(
        reached(cone, &f.program, &[f.sink]),
        vec![f.sink],
        "a terminal node reaches only itself"
    );
    assert_eq!(
        reached(cone, &f.program, &[f.loose]),
        vec![f.loose],
        "an unwired node reaches only itself"
    );
    assert!(
        reached(cone, &f.program, &[NodeId::unique()]).is_empty(),
        "an id the program never held seeds nothing"
    );
}

/// The set is the visited mark, so a seed named twice — and a seed already
/// reachable from another — appears exactly once.
#[test]
fn repeated_and_overlapping_seeds_are_visited_once() {
    let f = fixture();
    let cone = &mut ConsumerCone::default();
    let mut expected = vec![f.source, f.sink];
    expected.sort();
    assert_eq!(reached(cone, &f.program, &[f.source, f.source]), expected);
    assert_eq!(reached(cone, &f.program, &[f.source, f.sink]), expected);
}

/// Ascending indices out, and the walk assigns them in id order — so the
/// caller reads the cone in a stable order across compiles of one graph.
#[test]
fn the_cone_walks_the_dense_space_in_order() {
    let f = fixture();
    let all = reached(
        &mut ConsumerCone::default(),
        &f.program,
        &[f.source, f.sink, f.loose],
    );
    assert_eq!(all.len(), 3);
    assert!(all.is_sorted());
}

/// Reuse is the point of holding one: every buffer is refilled per call, so
/// a second query over a *different* program cannot inherit the first's
/// edges, run lengths, or visited marks.
#[test]
fn a_reused_cone_answers_each_program_from_scratch() {
    let f = fixture();
    let cone = &mut ConsumerCone::default();

    let mut both = vec![f.source, f.sink];
    both.sort();
    assert_eq!(reached(cone, &f.program, &[f.source]), both);

    // A second, *smaller* program with no edge at all — the shrink is the
    // case leftover runs and marks would survive into. The source must now
    // reach only itself.
    let library = test_func_lib(TestFuncHooks::default());
    let mut graph = Graph::default();
    let lone = graph.add_func_node(library.by_name("get_a").unwrap());
    graph.add_func_node(library.by_name("Print").unwrap());
    let unwired = Compiler::default().compile(&graph, &library).unwrap();
    assert_eq!(reached(cone, &unwired, &[lone]), vec![lone]);

    // And back to the wired one, which must answer as it did the first time.
    assert_eq!(reached(cone, &f.program, &[f.source]), both);
}
