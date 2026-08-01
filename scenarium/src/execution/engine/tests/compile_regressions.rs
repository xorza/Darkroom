use super::*;

use crate::graph::output_types::OutputTypes;
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

    let e = TestEngine::over(g);
    let program = e.engine.compiled();

    for (name, expected) in [("make_int", DataType::Int), ("make_str", DataType::String)] {
        assert_eq!(
            program.outputs[program.by_id(e.id(name)).outputs][0],
            expected,
            "{name} reads its own type, not its neighbour's"
        );
    }
}

/// The authoring-side type at one output port, for the tests that compare
/// what the editor would paint against what the compiled program carries.
///
/// A miss is the fixture naming a port that is not declared — never something
/// to read as `Any`, which is what an unresolvable *chain* resolves to. Since
/// `DataType::default()` is itself `Any`, defaulting here would let a resolver
/// that recorded nothing pass every `Any` case below vacuously.
fn authoring_output_type(g: &TestGraph, name: &str) -> DataType {
    let mut types = OutputTypes::default();
    types.update(&g.graph, &g.library);
    types
        .get(OutputPort::new(g.id(name), 0))
        .expect("the fixture names a declared output port")
        .clone()
}

#[test]
fn compiled_output_types_match_authoring_resolution() {
    let path_type = DataType::FsPath(Arc::new(FsPathConfig::new(FsPathMode::ExistingFile)));
    let passthrough = |n: NodeSpec| n.input(DataType::Any).wildcard(0);

    let mut g = TestGraph::new();
    g.add("fixed", |n| n.output(DataType::Int));
    // A reroute run long enough that a recursive resolver would blow the
    // stack: the walk must be iterative on both sides.
    let mut previous = "fixed".to_string();
    for hop in 0..70 {
        let name = format!("hop{hop}");
        g.add(&name, passthrough);
        g.wire(&previous, 0, &name, 0);
        previous = name;
    }
    g.add("scalar_const", passthrough);
    g.constant("scalar_const", 0, true);
    g.add("ambiguous_const", passthrough);
    g.constant("ambiguous_const", 0, ConstValue::Enum("A".into()));
    g.add("typed_const", |n| n.input(path_type.clone()).wildcard(0));
    g.constant("typed_const", 0, ConstValue::FsPath("input.fit".into()));
    g.add("unbound", passthrough);

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
        .map(|(name, _)| authoring_output_type(&g, name))
        .collect();

    let e = TestEngine::over(g);
    let program = e.engine.compiled();
    for ((name, expected), authored) in cases.iter().zip(authored) {
        assert_eq!(&authored, expected, "authoring resolution for {name}");
        assert_eq!(
            &program.outputs[program.by_id(e.id(name)).outputs][0],
            expected,
            "compiled resolution for {name}"
        );
    }
}

#[test]
fn authoring_and_compiled_output_resolution_break_cycles_as_any() {
    let mut g = TestGraph::new();
    g.add("passthrough", |n| n.input(DataType::Any).wildcard(0));
    g.wire("passthrough", 0, "passthrough", 0);
    assert_eq!(authoring_output_type(&g, "passthrough"), DataType::Any);

    // The same wire, compiled: the walk resolves the wildcard through the
    // binding it just interned, and the cycle closes on `Any` there too.
    let e = TestEngine::over(g);
    let program = e.engine.compiled();
    assert_eq!(
        program.outputs[program.by_id(e.id("passthrough")).outputs][0],
        DataType::Any
    );
}

/// An install may carry an evolved library: changed inputs and lambdas must
/// replace their prior compiled forms under the reused lowered node.
#[tokio::test]
async fn update_with_evolved_func_recompiles_and_runs_new_lambda() {
    use crate::async_lambda;

    let mut g = TestGraph::new();
    g.add("generate", |n| n.pure().output(DataType::Int).returns(1i64));
    g.add("print", |n| n.records());
    g.wire("generate", 0, "print", 0);

    let mut e = TestEngine::over(g);
    let run = e.run_sinks().await;
    assert_eq!(run.logs(), ["1"], "v1 lambda ran");

    // v2: the same declaration gains an input and a different body.
    e.edit(|g| {
        g.edit_func("generate", |func| {
            func.inputs.push(crate::graph::func::FuncInput::optional(
                "Extra",
                DataType::Int,
            ));
            func.lambda = async_lambda!(move |Invocation { outputs, .. }| {
                outputs[0] = ConstValue::Int(2).into();
                Ok(())
            });
        })
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
/// is now one value short of the port list. Debug builds caught it at
/// install as an `OutputArity` invariant violation; release builds carried
/// the mismatched snapshot into the run.
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
    g.add("print", |n| n.records());
    g.wire("generate", 0, "print", 0);

    let mut e = TestEngine::over(g);
    e.run_sinks().await;

    // The same declaration gains an output. Installing that is where the
    // retained snapshot had to be retired.
    e.edit(|g| {
        g.edit_func("generate", |func| {
            func.outputs
                .push(crate::graph::func::FuncOutput::new("W", DataType::Int));
        })
    });
    e.run_sinks().await;
}
