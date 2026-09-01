use super::*;

use std::any::Any;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use crate::async_lambda;
use crate::library::TypeEntry;
use crate::{CustomValue, TypeId};

const TRACKED_TYPE: &str = "7266406a-8083-4e46-b661-de4308bcec96";

/// Live/peak count of [`Tracked`] values resident at once during a run.
#[derive(Debug, Default)]
struct LiveTracker {
    current: usize,
    peak: usize,
}

/// A custom value that registers as live on creation and deregisters on `Drop`, so the
/// shared [`LiveTracker`] captures the peak number resident simultaneously. Cloning a
/// `DynamicValue::Custom` clones the `Arc`, not the `Tracked`, so a value stays live until
/// its last reference (cache slot or invoke buffer) drops — exactly what peak RAM tracks.
#[derive(Debug)]
struct Tracked {
    tracker: Arc<StdMutex<LiveTracker>>,
}

impl Tracked {
    fn new(tracker: Arc<StdMutex<LiveTracker>>) -> Self {
        {
            let mut t = tracker.lock().unwrap();
            t.current += 1;
            t.peak = t.peak.max(t.current);
        }
        Tracked { tracker }
    }
}

impl Drop for Tracked {
    fn drop(&mut self) {
        self.tracker.lock().unwrap().current -= 1;
    }
}

impl std::fmt::Display for Tracked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Tracked")
    }
}

impl CustomValue for Tracked {
    fn type_id(&self) -> TypeId {
        TRACKED_TYPE.into()
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn into_any(self: Arc<Self>) -> Arc<dyn Any + Send + Sync> {
        self
    }
}

fn tracked() -> DataType {
    DataType::Custom(TRACKED_TYPE.into())
}

/// A pure custom→custom node emitting a fresh [`Tracked`] on every call.
fn relay(
    tracker: Arc<StdMutex<LiveTracker>>,
    mode: CacheMode,
) -> impl FnOnce(NodeSpec) -> NodeSpec {
    move |n: NodeSpec| {
        n.pure()
            .cache(mode)
            .optional(tracked())
            .output(tracked())
            .lambda(async_lambda!(
                move |Invocation { outputs, .. }| { tracker = Arc::clone(&tracker) } => {
                    outputs[0] = DynamicValue::Custom(Arc::new(Tracked::new(Arc::clone(&tracker))));
                    Ok(())
                }
            ))
    }
}

/// A graph with `TRACKED_TYPE` registered, ready for the nodes above.
fn tracked_graph() -> TestGraph {
    let mut g = TestGraph::new();
    g.library
        .register_type(TRACKED_TYPE, TypeEntry::custom("Tracked"));
    g
}

/// Run a 4-stage relay chain into a sink with every relay set to
/// `relay_mode`, and return the peak number of tracked outputs resident at
/// once.
async fn chain_peak(relay_mode: CacheMode) -> usize {
    let tracker = Arc::new(StdMutex::new(LiveTracker::default()));
    let mut g = tracked_graph();
    for stage in 0..4 {
        g.add(
            &format!("relay{stage}"),
            relay(Arc::clone(&tracker), relay_mode),
        );
    }
    g.add("sink", |n| n.sink().input(tracked()));
    for stage in 1..4 {
        g.wire(
            &format!("relay{}", stage - 1),
            0,
            &format!("relay{stage}"),
            0,
        );
    }
    g.wire("relay3", 0, "sink", 0);

    let mut e = TestEngine::over(g);
    e.run_sinks().await;
    tracker.lock().unwrap().peak
}

/// The cache mode drives peak residency. With `None`, each stage's output is freed the
/// moment the next stage reads it, so only a producer/consumer pair is ever resident →
/// peak 2, whatever the chain length. With `Ram`, every stage is retained for cross-run
/// reuse, so all four accumulate → peak 4. That the two differ is the whole feature.
#[tokio::test]
async fn none_cache_bounds_peak_residency_but_ram_accumulates() {
    assert_eq!(
        chain_peak(CacheMode::None).await,
        2,
        "None frees each stage the instant it is drained"
    );
    assert_eq!(
        chain_peak(CacheMode::Ram).await,
        4,
        "Ram retains every stage for the whole run"
    );
}

/// Each probe's ownership observation, in invocation order, plus what stayed live.
#[derive(Debug)]
struct ProbeRun {
    unique_reads: Vec<bool>,
    live_after: usize,
}

/// Run `relay → n_probes × probe` with the relay in `relay_mode`.
///
/// A probe takes its input value out of the invoke buffer and records
/// whether it was uniquely owned (`into_custom` succeeded) — the observable
/// contract of the executor's move-on-last-use.
async fn probe_run(relay_mode: CacheMode, probes: usize) -> ProbeRun {
    let tracker = Arc::new(StdMutex::new(LiveTracker::default()));
    let reads = Arc::new(StdMutex::new(Vec::new()));

    let mut g = tracked_graph();
    g.add("relay", relay(Arc::clone(&tracker), relay_mode));
    for probe in 0..probes {
        let reads = Arc::clone(&reads);
        g.add(&format!("probe{probe}"), move |n: NodeSpec| {
            n.sink().input(tracked()).lambda(async_lambda!(
                move |Invocation { inputs, .. }| { reads = Arc::clone(&reads) } => {
                    let value = std::mem::take(&mut inputs[0]);
                    reads.lock().unwrap().push(value.into_custom::<Tracked>().is_ok());
                    Ok(())
                }
            ))
        });
        g.wire("relay", 0, &format!("probe{probe}"), 0);
    }

    // The engine stays bound: dropping it drops its cache, which would
    // release exactly the retained value `live_after` is here to observe.
    let mut e = TestEngine::over(g);
    e.run_sinks().await;
    let unique_reads = reads.lock().unwrap().clone();
    let live_after = tracker.lock().unwrap().current;
    ProbeRun {
        unique_reads,
        live_after,
    }
}

/// Move-on-last-use: the last read of a non-RAM output hands the consumer the slot's
/// own value — uniquely held, so an owning `into_custom` succeeds without a copy — and
/// nothing stays live after the run. A RAM-cached producer keeps its slot copy, so the
/// same probe observes a shared value; with fan-out only the final read is the move.
#[tokio::test]
async fn last_read_of_non_ram_output_is_uniquely_owned() {
    let run = probe_run(CacheMode::None, 1).await;
    assert_eq!(
        run.unique_reads,
        [true],
        "sole consumer of a None producer owns the value"
    );
    assert_eq!(run.live_after, 0, "moved value dropped with the probe");

    let run = probe_run(CacheMode::Ram, 1).await;
    assert_eq!(
        run.unique_reads,
        [false],
        "the RAM slot keeps a second Arc holder"
    );
    assert_eq!(run.live_after, 1, "the RAM slot retains the value");

    let run = probe_run(CacheMode::None, 2).await;
    assert_eq!(
        run.unique_reads,
        [false, true],
        "with fan-out only the last read is the move"
    );
    assert_eq!(run.live_after, 0, "both probe copies dropped by run end");
}
