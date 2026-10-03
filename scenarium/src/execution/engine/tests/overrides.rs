use super::*;
use crate::graph::identity::InputPort;

/// While the input declared to override another delivers, the target is set
/// aside: the lambda reads it unbound, the digest keys it unbound, and it is
/// not required. A `Null` constant or a disabled producer delivers nothing,
/// and the target is read again. The editor's reading from the graph alone
/// agrees at every step.
///
/// `pick` reports `100·preset + config`, an unbound port counting 0, so its
/// output names which inputs reached the lambda.
#[tokio::test]
async fn an_override_sets_its_target_aside_while_it_delivers() {
    let mut g = TestGraph::new();
    g.add("config", |n| n.returns(7i64).cache(CacheMode::Ram));
    g.add("pick", |n| {
        n.pure()
            .input(DataType::Int)
            .const_only()
            .optional(DataType::Int)
            .overrides(0)
            .output(DataType::Int)
            .cache(CacheMode::Ram)
            .compute(|inputs| {
                (inputs[0].as_i64().unwrap_or(0) * 100 + inputs[1].as_i64().unwrap_or(0)).into()
            })
    });
    g.add("Print", |n| n.sink().input(DataType::Int).observes(|_| ()));
    g.wire("pick", 0, "Print", 0);
    g.constant("pick", 0, 3i64);
    let mut e = TestEngine::over(g);
    let overridden = |e: &TestEngine| {
        let graph = &e.graph.graph;
        let node = graph.find(e.id("pick")).unwrap();
        let func = node.func(&e.graph.library).unwrap();
        graph.overridden(InputPort::new(e.id("pick"), 0), func)
    };

    assert!(!overridden(&e));
    assert_eq!(e.run_sinks().await.ran(), ["pick", "Print"]);
    assert_eq!(e.output_i64("pick", 0), Some(300), "100·3 + 0");

    e.edit(|g| g.wire("config", 0, "pick", 1));
    assert!(overridden(&e));
    assert_eq!(e.run_sinks().await.ran(), ["config", "pick", "Print"]);
    assert_eq!(e.output_i64("pick", 0), Some(7), "100·0 + 7");

    // Neither a new preset nor no preset at all re-keys `pick`: both fold as
    // the unbound the set-aside port already was, and the unbound required
    // port is not missing.
    e.edit(|g| g.constant("pick", 0, 4i64));
    assert_eq!(e.run_sinks().await.ran(), ["Print"]);
    e.edit(|g| g.unbind("pick", 0));
    assert_eq!(e.run_sinks().await.ran(), ["Print"]);

    e.edit(|g| g.disable("config"));
    assert!(!overridden(&e));
    let run = e.run_sinks().await;
    assert_eq!(run.missing_inputs(), ["Print", "pick"]);
    assert_eq!(run.missing_ports("pick"), [0]);

    e.edit(|g| g.constant("pick", 0, 4i64));
    assert_eq!(e.run_sinks().await.ran(), ["pick", "Print"]);
    assert_eq!(e.output_i64("pick", 0), Some(400), "100·4 + unbound 0");

    e.edit(|g| {
        g.enable("config");
        g.constant("pick", 1, ConstValue::Null);
    });
    assert!(!overridden(&e));
    let run = e.run_sinks().await;
    assert_eq!(run.ran(), ["pick", "Print"], "Null re-keys the config port");
    assert_eq!(e.output_i64("pick", 0), Some(400), "100·4 + Null as 0");

    e.edit(|g| g.constant("pick", 1, 9i64));
    assert!(overridden(&e));
    assert_eq!(e.run_sinks().await.ran(), ["pick", "Print"]);
    assert_eq!(e.output_i64("pick", 0), Some(9), "100·0 + 9");
}
