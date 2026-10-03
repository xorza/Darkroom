use std::slice;

use super::*;

use crate::graph::func::FuncInput;
use crate::graph::func::FuncOutput;
use crate::{FsPathConfig, FsPathMode};

/// The output pool is range-addressed: when a consumer precedes its producer
/// in insertion order, lowering claims the producer's *index* early while
/// output ranges are assigned in emit order — an index-order sequential fill
/// would hand the two producers each other's types.
#[test]
fn output_metadata_follows_ranges_when_consumer_precedes_producer() {
    // Declared consumer-first, then the *other* producer, then the one it
    // binds — so `make_str` claims its index before its range is assigned.
    let mut g = TestGraph::new();
    g.add("sink", |n| n.sink().input(DataType::Any));
    g.add("make_int", |n| n.returns(1i64));
    g.add("make_str", |n| n.returns("s"));
    g.wire("make_str", 0, "sink", 0);

    let compiled = g.compile();

    for (name, expected) in [("make_int", DataType::Int), ("make_str", DataType::String)] {
        assert_eq!(
            compiled.output_types(name),
            [expected],
            "{name} reads its own type, not its neighbour's"
        );
    }
}

#[test]
fn compiled_output_types_match_authoring_resolution() {
    let path_type = DataType::FsPath(Arc::new(FsPathConfig::new(FsPathMode::ExistingFile)));

    let mut g = TestGraph::new();
    g.add("fixed", |n| n.output(DataType::Int));
    // A reroute run long enough that a recursive resolver would blow the
    // stack: the walk must be iterative on both sides.
    let mut previous = "fixed".to_string();
    for hop in 0..70 {
        let name = format!("hop{hop}");
        g.add(&name, NodeSpec::passthrough);
        g.wire(&previous, 0, &name, 0);
        previous = name;
    }
    g.add("scalar_const", NodeSpec::passthrough);
    g.constant("scalar_const", 0, true);
    g.add("ambiguous_const", NodeSpec::passthrough);
    g.constant("ambiguous_const", 0, ConstValue::Enum("A".into()));
    g.add("typed_const", |n| n.input(path_type.clone()).wildcard(0));
    g.constant("typed_const", 0, ConstValue::FsPath("input.fit".into()));
    g.add("unbound", NodeSpec::passthrough);

    let cases = [
        ("fixed", DataType::Int),
        ("hop69", DataType::Int),
        ("scalar_const", DataType::Bool),
        ("ambiguous_const", DataType::Any),
        ("typed_const", path_type),
        ("unbound", DataType::Any),
    ];
    let authored: Vec<DataType> = cases
        .iter()
        .map(|(name, _)| g.output_type(name, 0))
        .collect();

    let compiled = g.compile();
    for ((name, expected), authored) in cases.iter().zip(authored) {
        assert_eq!(&authored, expected, "authoring resolution for {name}");
        assert_eq!(
            compiled.output_types(name),
            slice::from_ref(expected),
            "compiled resolution for {name}"
        );
    }
}

#[test]
fn authoring_and_compiled_output_resolution_break_cycles_as_any() {
    let mut g = TestGraph::new();
    g.add("passthrough", NodeSpec::passthrough);
    g.wire("passthrough", 0, "passthrough", 0);
    assert_eq!(g.output_type("passthrough", 0), DataType::Any);

    // The same wire, compiled: the walk resolves the wildcard through the
    // binding it just interned, and the cycle closes on `Any` there too.
    assert_eq!(g.compile().output_types("passthrough"), [DataType::Any]);
}

/// An install may carry an evolved library: changed inputs and lambdas must
/// replace their prior compiled forms under the reused lowered node.
#[tokio::test]
async fn update_with_evolved_func_recompiles_and_runs_new_lambda() {
    use crate::async_lambda;

    let mut g = TestGraph::new();
    g.add("generate", |n| n.returns(1i64));
    g.add("print", NodeSpec::records);
    g.wire("generate", 0, "print", 0);

    let mut e = TestEngine::over(g);
    let run = e.run_sinks().await;
    assert_eq!(run.logs(), ["1"], "v1 lambda ran");

    // v2: the same declaration gains an input and a different body.
    e.edit(|g| {
        g.evolve_func("generate", |func| {
            func.inputs
                .push(FuncInput::optional("Extra", DataType::Int));
            func.lambda = async_lambda!(move |Invocation { outputs, .. }| {
                outputs[0] = ConstValue::Int(2).into();
                Ok(())
            });
        });
    });

    assert_eq!(
        e.engine.node_inputs(e.id("generate")).len(),
        1,
        "the reused lowered node picked up the grown input list"
    );
    let run = e.run_sinks().await;
    assert_eq!(
        run.logs(),
        ["2"],
        "the input-shape change re-keyed the digest and the new lambda ran"
    );
}

/// A func that grows an **output** must not leave its previous, shorter
/// snapshot resident.
///
/// The grown-input case above re-keys the digest, which is what retires the
/// old value. Growing an output need not: the id is unchanged, so `reown`
/// sees no owner change, and the stale `produced_under` still equals the
/// stale `current_digest`, so the RAM-retention check keeps a snapshot that
/// would be one value short of the port list. The install retires it, so the
/// next run recomputes both outputs.
#[tokio::test]
async fn update_with_a_grown_output_list_retires_the_shorter_snapshot() {
    use crate::async_lambda;

    let body = || {
        async_lambda!(move |Invocation { outputs, .. }| {
            outputs[0] = ConstValue::Int(1).into();
            if outputs.len() > 1 {
                outputs[1] = ConstValue::Int(2).into();
            }
            Ok(())
        })
    };

    let mut g = TestGraph::new();
    // RAM-cached, so the snapshot is *meant* to survive an install — which
    // is what makes the stale one survive too.
    g.add("generate", |n| {
        n.pure()
            .cache(CacheMode::Ram)
            .output(DataType::Int)
            .lambda(body())
    });
    g.add("print", NodeSpec::records);
    g.wire("generate", 0, "print", 0);

    let mut e = TestEngine::over(g);
    e.run_sinks().await;

    // The same declaration gains an output. Installing that is where the
    // retained snapshot had to be retired.
    e.edit(|g| {
        g.evolve_func("generate", |func| {
            func.outputs.push(FuncOutput::new("W", DataType::Int));
        });
    });
    let run = e.run_sinks().await;
    assert_eq!(
        run.ran(),
        ["generate", "print"],
        "the retired value recomputes"
    );
    assert_eq!(e.outputs("generate").len(), 2);
    assert_eq!(e.output_i64("generate", 1), Some(2));
}

/// Library drift: wiring that references ports/events the library no
/// longer declares must still compile — the dangling binding degrades
/// to unbound (a required input reports missing), and a dangling
/// subscription wires nothing.
#[tokio::test]
async fn dangling_wiring_compiles_and_reports_missing_input() {
    let mut e = TestEngine::over(TestGraph::sample());
    // sum's required input 0 bound to an output `get_a` doesn't have, plus a
    // subscription to an event it doesn't emit — the drift a changed library
    // leaves behind. Neither may fail the compile.
    e.edit(|g| {
        g.wire("get_a", 9, "sum", 0);
        g.subscribe("get_a", 9, "sum");
    });

    assert!(
        e.engine.compiled().subscribers.is_empty(),
        "the dangling subscription wires nothing"
    );
    let run = e.run_sinks().await;

    assert_eq!(
        run.missing_ports("sum"),
        [0],
        "the dangling binding degrades to a missing input on that exact port"
    );
}
