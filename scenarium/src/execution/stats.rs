use std::time::Instant;

use hashbrown::HashMap;

use crate::data::{DynamicValue, RamUsage};
use crate::execution::RunError;
use crate::execution::event::EventRef;
use crate::graph::{InputPort, NodeId};

/// A record of how the authoring graph was flattened for execution.
///
/// Execution dissolves subgraphs: an interior node's id is remapped (it's
/// hashed with the chain of composite-instance ids it was reached
/// through), so the flattened `node_id`s in the stat lists don't match
/// the authoring graph for anything inside a subgraph. `FlattenMap` is
/// the reverse: a `scopes` arena (the tree of composite expansions, index
/// 0 = root) plus, per flattened node, the scope it landed in and its
/// authoring id within that scope's (sub)graph.
///
/// Ancestry is shared through the arena's parent links rather than stored
/// as a path per node, so it stays flat and allocation-light. Consume it
/// with [`FlattenMap::attribution`], which yields the authoring node a
/// flattened node maps to plus every enclosing composite instance.
#[derive(Debug, Clone, Default)]
pub struct FlattenMap {
    scopes: Vec<Scope>,
    leaves: HashMap<NodeId, Leaf>,
}

#[derive(Debug, Clone, Copy)]
struct Scope {
    /// Composite-instance node id that opened this scope; `None` for root.
    instance: Option<NodeId>,
    /// Enclosing scope index. Root points at itself (0); the walk stops on
    /// the `None` instance, so the self-loop is never followed.
    parent: u32,
}

#[derive(Debug, Clone, Copy)]
struct Leaf {
    scope: u32,
    interior: NodeId,
}

impl FlattenMap {
    /// Reset to a single root scope (index 0), reusing the buffers — the
    /// flattener calls this at the start of every build.
    pub fn reset(&mut self) {
        self.scopes.clear();
        self.leaves.clear();
        self.scopes.push(Scope {
            instance: None,
            parent: 0,
        });
    }

    /// Open a child scope for composite instance `instance` under
    /// `parent`; returns its index.
    pub fn push_scope(&mut self, instance: NodeId, parent: u32) -> u32 {
        let idx = self.scopes.len() as u32;
        self.scopes.push(Scope {
            instance: Some(instance),
            parent,
        });
        idx
    }

    /// Record that flattened node `flat_id` lives in `scope` as authoring
    /// node `interior`.
    pub fn set_leaf(&mut self, flat_id: NodeId, scope: u32, interior: NodeId) {
        self.leaves.insert(flat_id, Leaf { scope, interior });
    }

    /// The authoring node `flat_id` maps to — its leaf's interior id — or `None`
    /// when `flat_id` is unknown. The single-id inverse of the id remapping;
    /// [`attribution`](Self::attribution) additionally yields the enclosing
    /// composite instances.
    pub fn interior(&self, flat_id: NodeId) -> Option<NodeId> {
        self.leaves.get(&flat_id).map(|l| l.interior)
    }

    /// The authoring nodes a flattened node's outcome attributes to: its
    /// own interior id, then each enclosing composite instance (innermost
    /// first, root excluded). Empty when `flat_id` is unknown. No
    /// allocation — it walks the scope arena's parent links.
    pub fn attribution(&self, flat_id: NodeId) -> Attribution<'_> {
        let leaf = self.leaves.get(&flat_id);
        Attribution {
            map: self,
            interior: leaf.map(|l| l.interior),
            scope: leaf.map(|l| l.scope),
        }
    }
}

/// Iterator over [`FlattenMap::attribution`]: the interior node id, then
/// the chain of enclosing composite-instance ids.
pub struct Attribution<'a> {
    map: &'a FlattenMap,
    interior: Option<NodeId>,
    scope: Option<u32>,
}

impl Iterator for Attribution<'_> {
    type Item = NodeId;

    fn next(&mut self) -> Option<NodeId> {
        if let Some(id) = self.interior.take() {
            return Some(id);
        }
        let scope = self.map.scopes[self.scope? as usize];
        match scope.instance {
            Some(instance) => {
                self.scope = Some(scope.parent);
                Some(instance)
            }
            None => {
                self.scope = None;
                None
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct ExecutedNodeStats {
    pub node_id: NodeId,
    pub elapsed_secs: f64,
}

/// Where a node is in its run, for live progress (emitted by the executor
/// *during* a run, ahead of the final [`ExecutionStats`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RunPhase {
    /// The node's lambda is about to be invoked. `at` is the instant the
    /// invoke began (monotonic), so a consumer can show live elapsed-so-far.
    Started { at: Instant },
    /// The node's lambda finished, taking `elapsed_secs`.
    Finished { elapsed_secs: f64 },
}

/// One live progress event for a node. `nodes` is the authoring node(s) the
/// flattened node attributes to (interior id + enclosing composite instances),
/// already resolved via the [`FlattenMap`] so the consumer needn't be.
#[derive(Debug, Clone)]
pub struct RunProgress {
    pub nodes: Vec<NodeId>,
    pub phase: RunPhase,
}

/// A just-finished node's pinned output values, sent right after its lambda
/// completes successfully — before any real consumer's read can decrement its
/// usage count or trigger eviction, so the value is guaranteed still resident.
/// Only ports that are individually pinned
/// ([`Graph::pinned_outputs`](crate::graph::Graph::pinned_outputs)), or that
/// belong to a pinned-root node (a node-seeded on-demand preview target —
/// every output counts), are included; a run with no pinned output/root on the
/// node sends nothing. `node_id` is the authoring node the pin was actually
/// set on — already projected through the compile-phase `FlattenMap`, like
/// [`RunProgress::nodes`] — *not* also folded onto enclosing composite
/// instances, since a pin lives on one specific node's port, unlike a status
/// that reasonably summarizes a whole subtree.
#[derive(Debug, Clone)]
pub struct PinnedOutputs {
    pub node_id: NodeId,
    /// `(port_idx, value)` pairs, one per qualifying output — a node-local
    /// port index, not the compiled program's flat output-pool index.
    pub values: Vec<(usize, DynamicValue)>,
}

/// One live event streamed during a run — [`RunProgress`] and [`PinnedOutputs`]
/// share this single channel/type rather than each threading its own
/// `Option<&UnboundedSender<_>>` through `Executor::run`/`ExecutionEngine::execute`.
#[derive(Debug, Clone)]
pub enum RunEvent {
    Progress(RunProgress),
    PinnedOutputs(PinnedOutputs),
}

#[derive(Debug, Clone)]
pub struct NodeError {
    pub node_id: NodeId,
    pub error: RunError,
}

/// Severity of a [`LogEntry`] emitted by a node during execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

/// One log line emitted by a node lambda (via `ContextManager::log`).
/// `node_id` is the flattened execution id; a UI projects it back onto
/// the authoring node(s) via [`FlattenMap::attribution`], like the other
/// stat lists.
#[derive(Debug, Clone)]
pub struct LogEntry {
    pub node_id: NodeId,
    pub level: LogLevel,
    pub message: String,
}

#[derive(Debug)]
pub struct ExecutionStats {
    pub elapsed_secs: f64,

    pub executed_nodes: Vec<ExecutedNodeStats>,
    pub missing_inputs: Vec<InputPort>,
    pub cached_nodes: Vec<NodeId>,
    pub triggered_events: Vec<EventRef>,
    pub node_errors: Vec<NodeError>,
    /// Log lines emitted by node lambdas this run, in emission order.
    /// Keyed by flattened node id; project via [`FlattenMap::attribution`].
    /// The map itself isn't carried here — the host compiled the graph, so it
    /// already holds the [`FlattenMap`] of the program these stats came from.
    pub logs: Vec<LogEntry>,
    /// The run was cancelled mid-flight: scheduling stopped before every
    /// node ran (the already-running node still completed). The stat lists
    /// reflect only what actually ran.
    pub cancelled: bool,
    /// RAM held by the runtime cache's resident values after this run's
    /// end-of-run eviction — system RAM vs GPU VRAM. Stamped by
    /// [`ExecutionEngine::execute`](crate::execution::ExecutionEngine) once the
    /// cache's resident set is final; the editor surfaces it as a memory readout.
    pub cache_ram: RamUsage,
    /// Per-node resident RAM for nodes holding a cached value after eviction,
    /// keyed by *flattened* execution id — project onto authoring nodes via
    /// [`FlattenMap::attribution`] like the other stat lists. Only non-zero nodes
    /// appear. Drives each node body's memory readout. Stamped alongside
    /// [`cache_ram`](Self::cache_ram).
    pub node_ram: Vec<(NodeId, RamUsage)>,
}
