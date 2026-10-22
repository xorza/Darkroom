use std::error::Error as _;
use std::io;

use super::*;
use crate::async_lambda;

/// A lambda's failure with a cause of its own.
#[derive(Debug, thiserror::Error)]
#[error("could not read the frame")]
struct ReadFailed(#[source] io::Error);

#[tokio::test]
async fn node_error_propagates_to_dependents() {
    let mut g = TestGraph::sample_values(1, 42);
    g.fails("get_a", "Intentional failure in get_a");
    let mut e = TestEngine::over(g);

    let run = e.run_sinks().await;

    // The failure and the three consumers that inherit it — errors are
    // reported through the run, not the cross-run cache, which only
    // reflects which outputs survived.
    assert_eq!(run.errored(), ["Print", "get_a", "mult", "sum"]);
    assert!(matches!(
        run.error("get_a"),
        Some(RunError::Invoke(error)) if error.to_string() == "Intentional failure in get_a"
    ));
    for name in ["sum", "mult", "Print"] {
        assert!(
            matches!(run.error(name), Some(RunError::SkippedUpstream)),
            "{name} should report an upstream error",
        );
        assert!(e.outputs(name).is_empty(), "{name} should have no output");
    }

    // The one node off the failing cone keeps its value.
    assert!(e.outputs("get_a").is_empty());
    assert!(run.error("get_b").is_none());
    assert_eq!(e.output_i64("get_b", 0), Some(42));

    // The lambda's error reaches the row whole: its own message, and its own
    // cause behind it rather than folded into a string.
    e.edit(|g| {
        g.edit_func("get_a", |func| {
            func.lambda = async_lambda!(|_| {
                Err(InvokeError::external(ReadFailed(io::Error::other(
                    "disk gone",
                ))))
            });
        });
    });
    let run = e.run_sinks().await;
    let error = run.error("get_a").expect("the failing node reports");
    assert!(matches!(error, RunError::Invoke(_)));
    assert_eq!(error.to_string(), "could not read the frame");
    assert_eq!(
        error.source().map(ToString::to_string).as_deref(),
        Some("disk gone")
    );
    assert!(matches!(run.error("sum"), Some(RunError::SkippedUpstream)));
}
