use super::*;
use crate::execution::compile::Compiler;
use crate::graph::func::FuncBehavior;

/// The builder states a whole graph — declarations, wiring and bodies — and
/// what it produces compiles.
#[test]
fn a_named_graph_compiles_to_one_node_per_name() {
    let mut g = TestGraph::new();
    g.add("src", |n| n.returns(7i64));
    g.add("double", |n| {
        n.pure()
            .input(DataType::Int)
            .output(DataType::Int)
            .compute(|inputs| (inputs[0].as_i64().unwrap() * 2).into())
    });
    g.add("print", |n| n.records());
    g.wire("src", 0, "double", 0);
    g.wire("double", 0, "print", 0);

    // Every name resolves to a distinct node, and the wiring is real.
    assert_eq!(g.graph.len(), 3);
    assert_eq!(
        g.graph.bindings[&InputPort::new(g.id("double"), 0)],
        Binding::bind(g.id("src"), 0)
    );
    // `returns` declared the output its literal implies.
    let compiled = Compiler::default()
        .compile(&g.graph, &g.library)
        .expect("the fixture compiles");
    assert!(compiled.contains(g.id("src")));
    assert_eq!(compiled.node_ids.len(), 3);
}

/// One func per node by default: editing one node's declaration leaves
/// every other node's alone. `instance` is the only way to opt into
/// sharing, and then the edit reaches both.
#[test]
fn declarations_are_per_node_unless_a_test_shares_one() {
    let mut g = TestGraph::new();
    g.add("a", |n| n.pure().input(DataType::Int).output(DataType::Int));
    g.add("b", |n| n.pure().input(DataType::Int).output(DataType::Int));
    g.instance("a2", "a");

    g.edit_func("a", |func| func.inputs[0].required = false);

    let required = |g: &TestGraph, name: &str| {
        let node = g.graph.find(g.id(name)).unwrap();
        g.graph.node_func(node, &g.library).unwrap().inputs[0].required
    };
    assert!(!required(&g, "a"), "the edited declaration went optional");
    assert!(!required(&g, "a2"), "and its other instance shares it");
    assert!(required(&g, "b"), "an unrelated node is untouched");
}

/// `defaulted` seeds the const binding a fresh node starts with, and
/// `constant`/`unbind` overwrite it — the three ways a literal reaches a
/// port.
#[test]
fn declared_defaults_seed_bindings_that_later_calls_replace() {
    let mut g = TestGraph::new();
    g.add("n", |n| {
        n.pure()
            .defaulted(DataType::Int, 3i64)
            .output(DataType::Int)
    });
    let port = InputPort::new(g.id("n"), 0);

    assert_eq!(
        g.graph.bindings[&port],
        Binding::Const(ConstValue::Int(3)),
        "the declaration's default is bound on instantiation"
    );

    g.constant("n", 0, 9i64);
    assert_eq!(g.graph.bindings[&port], Binding::Const(ConstValue::Int(9)));

    g.unbind("n", 0);
    assert!(!g.graph.bindings.contains_key(&port));
}

/// The sample fixture wires what its doc claims, and its nodes carry ids
/// ascending with declaration order — which is what makes the schedule
/// order every engine test names reproducible.
#[test]
fn sample_wires_five_nodes_in_declaration_order() {
    let g = TestGraph::sample();
    assert_eq!(g.graph.len(), 5);

    for (consumer, input, producer) in [
        ("sum", 0, "get_a"),
        ("sum", 1, "get_b"),
        ("mult", 0, "sum"),
        ("mult", 1, "get_b"),
        ("Print", 0, "mult"),
    ] {
        assert_eq!(
            g.graph.bindings[&InputPort::new(g.id(consumer), input)],
            Binding::bind(g.id(producer), 0),
            "{producer} feeds {consumer}[{input}]"
        );
    }

    let ids: Vec<NodeId> = ["get_a", "get_b", "sum", "mult", "Print"]
        .into_iter()
        .map(|name| g.id(name))
        .collect();
    let mut sorted = ids.clone();
    sorted.sort();
    assert_eq!(ids, sorted, "ids ascend with declaration order");
}

/// `sample_values` moves what the sources emit, all the way to the sink:
/// the defaults compute `(1 + 11) * 11 = 132`, and 2/5 computes
/// `(2 + 5) * 5 = 35`.
#[tokio::test]
async fn sample_values_reach_the_sink() {
    use crate::testing::engine::TestEngine;

    let mut default = TestEngine::over(TestGraph::sample());
    assert_eq!(default.run_sinks().await.logs(), ["132"]);

    let mut moved = TestEngine::over(TestGraph::sample_values(2, 5));
    assert_eq!(moved.run_sinks().await.logs(), ["35"]);
}

/// `never` makes "this must not run" something the run has to honour: the
/// node it names panics if reached, while the rest of the fixture is
/// untouched.
#[tokio::test]
#[should_panic(expected = "get_a must not run in this fixture")]
async fn never_panics_when_the_run_reaches_the_node() {
    use crate::testing::engine::TestEngine;

    let mut g = TestGraph::sample();
    g.never("get_a");
    TestEngine::over(g).run_sinks().await;
}

/// The spec's flags land on the declaration rather than being remembered
/// by the builder — distinct inputs produce distinct funcs.
#[test]
fn spec_flags_reach_the_declaration() {
    let mut g = TestGraph::new();
    g.add("plain", |n| n.output(DataType::Int));
    g.add("flagged", |n| {
        n.pure()
            .sink()
            .uncacheable()
            .cache(CacheMode::Both)
            .output(DataType::Int)
    });

    let func = |g: &TestGraph, name: &str| {
        let node = g.graph.find(g.id(name)).unwrap();
        g.graph.node_func(node, &g.library).unwrap().clone()
    };
    let plain = func(&g, "plain");
    assert_eq!(plain.behavior, FuncBehavior::Impure);
    assert!(!plain.sink && !plain.uncacheable);
    assert_eq!(plain.default_cache_mode, CacheMode::None);

    let flagged = func(&g, "flagged");
    assert_eq!(flagged.behavior, FuncBehavior::Pure);
    assert!(flagged.sink && flagged.uncacheable);
    assert_eq!(flagged.default_cache_mode, CacheMode::Both);
}
