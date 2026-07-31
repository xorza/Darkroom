//! [`TestEngine`]: a [`TestGraph`] compiled, installed, and run — with every
//! answer coming back **by name**.
//!
//! The engine speaks `NodeId` and, inside, `NodeIdx`; a test speaks "sum" and
//! "print". Bridging that per assertion is what produced
//! `states[compiled().node(id).unwrap()]` and a trio of name↔id helpers at the
//! top of the engine's test file. Here the bridge is crossed once, when a run
//! finishes, and what a test reads is [`RunOutcome`] — names in, names out.
//!
//! **The outcome is a snapshot, not a borrow.** Every list is materialised when
//! the run ends, so a test can hold two runs side by side and compare them, and
//! nothing it asserts is invalidated by the next `run_*` call.

use std::path::PathBuf;

use hashbrown::HashMap;

use ::common::CancelToken;

use crate::execution::cache::disk_store::DiskStore;
use crate::execution::cache::runtime::error::CacheEvictionFailure;
use crate::execution::compile::error::CompileError;
use crate::execution::engine::ExecutionEngine;
use crate::execution::error::{Result, RunError};
use crate::execution::report::RunPhase;
use crate::execution::report::internals::{CollectingReporter, DiscardedReports};
use crate::execution::report::{ExecutionOutcome, NodeExecutionStatus, NodeStatus};
use crate::execution::schedule::NodeState;
use crate::execution::seeds::RunSeeds;
use crate::graph::func::lambda::OutputDemand;
use crate::graph::identity::{EventPort, NodeId};
use crate::testing::graph::TestGraph;
use crate::worker::status::{WorkerStatus, WorkerStatusKind};
use crate::{DynamicValue, RamUsage};

/// A [`TestGraph`] with a live engine over it.
///
/// The graph stays reachable, so an edit is stated the way the fixture stated
/// the graph — by name — and [`edit`](Self::edit) recompiles behind it.
#[derive(Debug)]
pub(crate) struct TestEngine {
    pub(crate) graph: TestGraph,
    pub(crate) engine: ExecutionEngine,
    outcome: ExecutionOutcome,
}

impl TestEngine {
    /// Compile `graph` and install it. Panics on a compile error — a fixture
    /// that does not compile is a broken fixture; [`try_over`](Self::try_over)
    /// is for the tests where the refusal *is* the subject.
    pub(crate) fn over(graph: TestGraph) -> Self {
        let mut engine = Self {
            graph,
            engine: ExecutionEngine::default(),
            outcome: ExecutionOutcome::default(),
        };
        engine.reinstall();
        engine
    }

    /// Edit the graph and reinstall what it became — the ordinary shape of a
    /// test that changes one thing and re-runs.
    pub(crate) fn edit(&mut self, edit: impl FnOnce(&mut TestGraph)) {
        edit(&mut self.graph);
        self.reinstall();
    }

    pub(crate) fn reinstall(&mut self) {
        self.try_reinstall().expect("the fixture graph compiles");
    }

    pub(crate) fn try_reinstall(&mut self) -> std::result::Result<(), CompileError> {
        self.engine.update(&self.graph.graph, &self.graph.library)
    }

    /// Point the cache's disk tier at `root`, using this graph's own codecs.
    ///
    /// Between runs, like a document gaining a cache directory — every reuse
    /// verdict already given was answered from the previous store.
    pub(crate) fn attach_disk_store(&mut self, root: impl Into<PathBuf>) {
        let store = DiskStore::new(&self.graph.library, Some(root.into()));
        self.engine.set_disk_store(store);
    }

    pub(crate) fn id(&self, name: &str) -> NodeId {
        self.graph.id(name)
    }

    /// Run to these exact nodes — the "run to this node" / preview trigger.
    pub(crate) async fn run_nodes<'n>(
        &mut self,
        names: impl IntoIterator<Item = &'n str>,
    ) -> RunOutcome {
        let node_ids = names.into_iter().map(|name| self.id(name)).collect();
        self.run(RunSeeds::nodes(node_ids))
            .await
            .expect("the run completes")
    }

    /// Fire these events, running whatever subscribes to them.
    pub(crate) async fn run_events(
        &mut self,
        events: impl IntoIterator<Item = EventPort>,
    ) -> RunOutcome {
        self.run(RunSeeds::events(events.into_iter().collect()))
            .await
            .expect("the run completes")
    }

    /// Initialize every node owning a subscribed event — the event loop's
    /// bootstrap run.
    pub(crate) async fn run_event_sources(&mut self) -> RunOutcome {
        self.run(RunSeeds {
            event_sources: true,
            ..Default::default()
        })
        .await
        .expect("the run completes")
    }

    /// An event port on a named node.
    pub(crate) fn event(&self, name: &str, event_idx: usize) -> EventPort {
        EventPort {
            node_id: self.id(name),
            event_idx,
        }
    }

    pub(crate) async fn run_sinks(&mut self) -> RunOutcome {
        self.run(RunSeeds::sinks())
            .await
            .expect("the run completes")
    }

    /// The general form, surfacing the operation-level refusal a bad seed
    /// raises.
    pub(crate) async fn run(&mut self, seeds: RunSeeds) -> Result<RunOutcome> {
        self.run_cancellable(seeds, CancelToken::never()).await
    }

    /// Run every sink, also handing back the live progress the run published
    /// — each event named, in the order the executor emitted it.
    pub(crate) async fn run_sinks_reporting(&mut self) -> (RunOutcome, Vec<(String, RunPhase)>) {
        let TestEngine {
            graph,
            engine,
            outcome,
        } = self;
        let mut reporter = CollectingReporter::default();
        engine
            .execute(
                RunSeeds::sinks(),
                &mut reporter,
                CancelToken::never(),
                outcome,
            )
            .await
            .expect("the run completes");
        let names = NameMap::of(graph);
        let progress = reporter
            .progress
            .iter()
            .map(|event| (names.name(event.node_id), event.phase))
            .collect();
        (RunOutcome::snapshot(graph, engine, outcome), progress)
    }

    /// Seed a node's cached output, as a prior run would have left it.
    pub(crate) fn set_output(&mut self, name: &str, values: Vec<DynamicValue>) {
        let id = self.id(name);
        self.engine.set_output_values(id, values);
    }

    /// Drop these nodes and everything downstream from RAM and disk, handing
    /// back whatever could not be removed.
    pub(crate) async fn evict<'n>(
        &mut self,
        names: impl IntoIterator<Item = &'n str>,
    ) -> Vec<CacheEvictionFailure> {
        let node_ids: Vec<NodeId> = names.into_iter().map(|name| self.id(name)).collect();
        self.engine.evict_cache(&node_ids).await
    }

    /// What the last plan settled for this node.
    pub(crate) fn state(&self, name: &str) -> NodeState {
        self.engine.node_state(self.id(name))
    }

    /// Whether this node currently holds a resident value.
    pub(crate) fn holds_output(&self, name: &str) -> bool {
        self.engine.slot(self.id(name)).output_values().is_some()
    }

    pub(crate) async fn run_cancellable(
        &mut self,
        seeds: RunSeeds,
        cancel: CancelToken,
    ) -> Result<RunOutcome> {
        let TestEngine {
            graph,
            engine,
            outcome,
        } = self;
        engine
            .execute(seeds, &mut DiscardedReports, cancel, outcome)
            .await?;
        Ok(RunOutcome::snapshot(graph, engine, outcome))
    }

    /// Plan and resolve without invoking any lambda — what the schedule looks
    /// like before anything runs.
    pub(crate) async fn plan_sinks(&mut self) -> Result<PlanOutcome> {
        self.plan(RunSeeds::sinks()).await
    }

    pub(crate) async fn plan(&mut self, seeds: RunSeeds) -> Result<PlanOutcome> {
        self.engine
            .prepare_execution(seeds.sinks, seeds.event_sources, &seeds.events)
            .await?;
        Ok(PlanOutcome::snapshot(&self.graph, &self.engine))
    }

    /// What the last plan or run demands of this node's outputs, and how many
    /// live readers each has.
    ///
    /// Engine state rather than run output: both are settled by the sweep, and
    /// they read the same after `plan_*` as after `run_*`.
    pub(crate) fn demand(&self, name: &str) -> &[OutputDemand] {
        self.engine.node_output_demand(self.id(name))
    }

    pub(crate) fn readers(&self, name: &str) -> &[u32] {
        self.engine.node_output_readers(self.id(name))
    }

    /// This node's inputs as the run delivered them — `None` for a port
    /// nothing fed.
    pub(crate) fn inputs(&self, name: &str) -> Vec<Option<DynamicValue>> {
        let id = self.id(name);
        self.engine
            .get_argument_values(&id)
            .expect("the named node is installed")
            .inputs
    }

    /// This node's produced outputs, as the cache currently holds them —
    /// empty when the run released or never wrote them.
    pub(crate) fn outputs(&self, name: &str) -> Vec<DynamicValue> {
        let id = self.id(name);
        self.engine
            .get_argument_values(&id)
            .expect("the named node is installed")
            .outputs
    }

    pub(crate) fn output(&self, name: &str, port: usize) -> Option<DynamicValue> {
        self.outputs(name).get(port).cloned()
    }

    /// This node's input `port` read as an integer — `None` for a port
    /// nothing fed, and for a value no scalar coercion reaches.
    pub(crate) fn input_i64(&self, name: &str, port: usize) -> Option<i64> {
        self.inputs(name)
            .get(port)
            .cloned()
            .flatten()
            .and_then(|value| value.as_i64())
    }

    /// This node's output `port` read as an integer.
    ///
    /// `DynamicValue` is not `PartialEq` — it may hold an opaque
    /// `Arc<dyn CustomValue>` — so a scalar assertion goes through the same
    /// coercion a consuming lambda would.
    pub(crate) fn output_i64(&self, name: &str, port: usize) -> Option<i64> {
        self.output(name, port).and_then(|value| value.as_i64())
    }
}

/// The per-node facts of one finished run, keyed by name.
#[derive(Debug)]
pub(crate) struct RunOutcome {
    /// Names that invoked their lambda, in the order the schedule reached them
    /// — deps before consumers, which is what makes this readable as the run.
    ran: Vec<String>,
    /// Every row the run reported, in the program's node order.
    rows: Vec<(String, NodeStatus)>,
    logs: Vec<String>,
    pub(crate) ran_node_count: usize,
    pub(crate) cancelled: bool,
    /// The events this run was seeded with, echoed back.
    pub(crate) triggered_events: Vec<EventPort>,
    /// The events a successful event source armed for the loop to spawn.
    pub(crate) armed_events: Vec<EventPort>,
    pub(crate) cache_ram: RamUsage,
}

impl RunOutcome {
    fn snapshot(graph: &TestGraph, engine: &ExecutionEngine, outcome: &ExecutionOutcome) -> Self {
        let names = NameMap::of(graph);
        Self {
            ran: engine
                .ran_in_schedule_order()
                .into_iter()
                .map(|node_id| names.name(node_id))
                .collect(),
            rows: outcome
                .nodes
                .iter()
                .map(|row| (names.name(row.node_id), row.clone()))
                .collect(),
            logs: outcome
                .logs
                .iter()
                .map(|entry| entry.message.clone())
                .collect(),
            ran_node_count: outcome.ran_node_count,
            cancelled: outcome.cancelled,
            triggered_events: outcome.triggered_events.clone(),
            armed_events: outcome
                .event_triggers
                .iter()
                .map(|trigger| trigger.event)
                .collect(),
            cache_ram: outcome.cache_ram,
        }
    }

    /// The same snapshot taken from a status a [`Worker`] published.
    ///
    /// A status carries the run's rows and its whole-run header, and nothing
    /// else: the worker publishes what the run *produced*, never how the
    /// schedule walked it. So this fills [`ran`](Self::ran) with the same names
    /// sorted, and the events — which a status also does not carry — stay
    /// empty.
    ///
    /// [`Worker`]: crate::worker::Worker
    pub(crate) fn published(graph: &TestGraph, status: &WorkerStatus) -> Self {
        let WorkerStatusKind::Completed {
            executed_node_count,
            cancelled,
            ..
        } = status.kind
        else {
            panic!("only a completed status carries a run");
        };
        let names = NameMap::of(graph);
        let mut outcome = Self {
            ran: Vec::new(),
            rows: status
                .nodes
                .iter()
                .map(|row| (names.name(row.node_id), row.clone()))
                .collect(),
            logs: status
                .logs
                .iter()
                .map(|entry| entry.message.clone())
                .collect(),
            ran_node_count: executed_node_count,
            cancelled,
            triggered_events: Vec::new(),
            armed_events: Vec::new(),
            cache_ram: status.cache_ram,
        };
        outcome.ran = outcome
            .with_status(|status| matches!(status, NodeExecutionStatus::Executed { .. }))
            .into_iter()
            .map(str::to_owned)
            .collect();
        outcome
    }

    /// Names that invoked their lambda and succeeded.
    ///
    /// In schedule order — deps before consumers — for a run this process
    /// drove. A [`published`](Self::published) outcome has no order to give and
    /// sorts them by name instead.
    pub(crate) fn ran(&self) -> Vec<&str> {
        self.ran.iter().map(String::as_str).collect()
    }

    /// Served from a cache — reused, or left resident by the pre-run cut.
    ///
    /// Sorted by name, like every set-shaped accessor here; only
    /// [`ran`](Self::ran) keeps an order, because *its* order is the answer.
    pub(crate) fn cached(&self) -> Vec<&str> {
        self.with_status(|status| matches!(status, NodeExecutionStatus::Cached))
    }

    pub(crate) fn errored(&self) -> Vec<&str> {
        self.with_status(|status| matches!(status, NodeExecutionStatus::Errored { .. }))
    }

    pub(crate) fn missing_inputs(&self) -> Vec<&str> {
        self.with_status(|status| matches!(status, NodeExecutionStatus::MissingInputs { .. }))
    }

    /// Log lines the run emitted, in order — how a `records` node reports what
    /// it saw.
    /// Names still holding RAM after the run, sorted.
    pub(crate) fn holding_ram(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self
            .rows
            .iter()
            .filter(|(_, row)| row.ram.total() > 0)
            .map(|(name, _)| name.as_str())
            .collect();
        names.sort_unstable();
        names
    }

    pub(crate) fn logs(&self) -> Vec<&str> {
        self.logs.iter().map(String::as_str).collect()
    }

    pub(crate) fn status(&self, name: &str) -> Option<&NodeExecutionStatus> {
        self.row(name).and_then(|row| row.status.as_ref())
    }

    pub(crate) fn error(&self, name: &str) -> Option<&RunError> {
        match self.status(name)? {
            NodeExecutionStatus::Errored { error, .. } => Some(error),
            _ => None,
        }
    }

    /// The input ports the run could not satisfy on this node; empty when it
    /// had none.
    pub(crate) fn missing_ports(&self, name: &str) -> &[usize] {
        match self.status(name) {
            Some(NodeExecutionStatus::MissingInputs { ports }) => ports,
            _ => &[],
        }
    }

    fn row(&self, name: &str) -> Option<&NodeStatus> {
        self.rows
            .iter()
            .find(|(row_name, _)| row_name == name)
            .map(|(_, row)| row)
    }

    /// Sorted by name, not by the program's node order: the id sort that
    /// settles that order is not something a test states or should have to
    /// predict, while a name list is readable and stable across a fixture whose
    /// ids move.
    fn with_status(&self, matches: impl Fn(&NodeExecutionStatus) -> bool) -> Vec<&str> {
        let mut names: Vec<&str> = self
            .rows
            .iter()
            .filter(|(_, row)| row.status.as_ref().is_some_and(&matches))
            .map(|(name, _)| name.as_str())
            .collect();
        names.sort_unstable();
        names
    }
}

/// What the two pre-run passes settled, before any lambda ran.
#[derive(Debug)]
pub(crate) struct PlanOutcome {
    order: Vec<String>,
    states: Vec<(String, NodeState)>,
}

impl PlanOutcome {
    fn snapshot(graph: &TestGraph, engine: &ExecutionEngine) -> Self {
        let compiled = engine.compiled();
        let names = NameMap::of(graph);
        let states = compiled
            .node_ids
            .iter()
            .copied()
            .map(|node_id| (names.name(node_id), engine.node_state(node_id)))
            .collect();
        Self {
            order: engine
                .ran_in_schedule_order()
                .into_iter()
                .map(|node_id| names.name(node_id))
                .collect(),
            states,
        }
    }

    /// Names in the order the schedule walks them — deps before consumers.
    ///
    /// Distinct from [`runnable`](Self::runnable): this is the *sequence*, and
    /// it excludes what the walk never reached.
    pub(crate) fn scheduled(&self) -> Vec<&str> {
        self.order.iter().map(String::as_str).collect()
    }

    /// Names the planner cleared to run, sorted.
    pub(crate) fn runnable(&self) -> Vec<&str> {
        self.where_state(|state| state.is_runnable())
    }

    /// Names blocked for want of a required input — the verdict that
    /// propagates to consumers.
    pub(crate) fn missing_inputs(&self) -> Vec<&str> {
        self.where_state(|state| state.missing_required_inputs())
    }

    pub(crate) fn state(&self, name: &str) -> NodeState {
        self.states
            .iter()
            .find(|(row_name, _)| row_name == name)
            .map(|(_, state)| *state)
            .unwrap_or_else(|| panic!("no node named {name:?} in the installed program"))
    }

    /// Sorted by name — see [`RunOutcome::cached`] for why.
    fn where_state(&self, matches: impl Fn(NodeState) -> bool) -> Vec<&str> {
        let mut names: Vec<&str> = self
            .states
            .iter()
            .filter(|(_, state)| *state != NodeState::Unvisited && matches(*state))
            .map(|(name, _)| name.as_str())
            .collect();
        names.sort_unstable();
        names
    }
}

/// Ids back to the names the fixture gave them.
#[derive(Debug)]
struct NameMap {
    by_id: HashMap<NodeId, String>,
}

impl NameMap {
    fn of(graph: &TestGraph) -> Self {
        Self {
            by_id: graph
                .graph
                .iter()
                .map(|node| (node.id, node.name.clone()))
                .collect(),
        }
    }

    /// A node the graph no longer holds still has to report — an eviction test
    /// names one — so an unknown id reads as its id rather than panicking.
    fn name(&self, node_id: NodeId) -> String {
        self.by_id
            .get(&node_id)
            .cloned()
            .unwrap_or_else(|| format!("{node_id:?}"))
    }
}
