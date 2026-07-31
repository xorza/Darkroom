use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn node_error_propagates_to_dependents() -> TestResult {
    let mut e = TestEngine::over(TestGraph::sample_with(TestFuncHooks {
        get_a: Arc::new(|| Err(internals::failure("Intentional failure in get_a"))),
        get_b: Arc::new(|| 42),
        print: Arc::new(|_| {}),
    }));

    let run = e.run_sinks().await;

    // The failure and the three consumers that inherit it — errors are
    // reported through the run, not the cross-run cache, which only
    // reflects which outputs survived.
    assert_eq!(run.errored(), ["Print", "get_a", "mult", "sum"]);
    assert!(
        run.error("get_a")
            .expect("the failing node reports its own error")
            .to_string()
            .contains("Intentional failure")
    );
    for name in ["sum", "mult", "Print"] {
        assert!(
            run.error(name)
                .unwrap_or_else(|| panic!("{name} should carry an upstream error"))
                .to_string()
                .contains("upstream"),
            "{name} should report an upstream error",
        );
        assert!(e.outputs(name).is_empty(), "{name} should have no output");
    }

    // The one node off the failing cone keeps its value.
    assert!(e.outputs("get_a").is_empty());
    assert!(run.error("get_b").is_none());
    assert_eq!(e.output_i64("get_b", 0), Some(42));
    Ok(())
}
