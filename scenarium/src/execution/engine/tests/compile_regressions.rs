use super::*;

use crate::execution::compile::compiled_graph::ExecutionBinding;
use crate::graph::func::func_input::FuncInput;
use crate::graph::func::func_output::FuncOutput;

/// The output pool is range-addressed: when a consumer precedes its producer
/// in insertion order, lowering claims the producer's *index* early while
/// output ranges are assigned in emit order — an index-order sequential fill
/// would hand the two producers each other's slots.
#[test]
fn a_wire_addresses_its_producer_range_when_the_consumer_precedes_it() {
    // Declared consumer-first, then the *other* producer, then the one it
    // binds — so `make_str` claims its index before its range is assigned.
    let mut g = TestGraph::new();
    g.add("sink", |n| n.sink().input(DataType::Any));
    g.add("make_int", |n| n.returns(1i64));
    g.add("make_str", |n| n.returns("s"));
    g.wire("make_str", "sink");

    let compiled = g.compile();
    let &ExecutionBinding::Bind(address) = compiled.binding("sink", 0) else {
        panic!("the sink is bound to make_str");
    };
    let slot = compiled.program.output_idx(address).0 as usize;
    assert!(compiled.node("make_str").outputs.range().contains(&slot));
    assert!(!compiled.node("make_int").outputs.range().contains(&slot));
}

/// An install may carry an evolved library: changed inputs and lambdas must
/// replace their prior compiled forms under the reused lowered node.
#[tokio::test]
async fn update_with_evolved_func_recompiles_and_runs_new_lambda() {
    use crate::async_lambda;

    let mut g = TestGraph::new();
    g.add("generate", |n| n.returns(1i64));
    g.add("print", NodeSpec::records);
    g.wire("generate", "print");

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
/// old value. Growing an output need not: the id is unchanged, so the stale
/// `produced_under` still equals the stale `current_digest`, so the RAM-retention
/// check keeps a snapshot that would be one value short of the port list. The
/// install retires it, so the next run recomputes both outputs.
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
    g.wire("generate", "print");

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
        g.wire(("get_a", 9), "sum");
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
