//! The compile artifact: the lowered graph — topology + code — plus every
//! question a host asks of one.
//!
//! Built once by the [`Compiler`](crate::exec::compile::Compiler)'s walk — no mutators, and no
//! pass that fills a field in afterwards — then installed as runtime state; deliberately not a
//! persistence format. Mutable state is split between the per-run schedule/executor and the
//! cross-run runtime cache.
//!
//! Self-contained: everything a run needs was copied out of the
//! [`Library`](crate::library::Library) at compile.
//!
//! **One program node, one execution node, one [`ProgramId`].** An authored node outside every
//! instance is one program node; one inside a compound is one per instance, each under its own
//! path. Resolving a program id against the artifact is a search of its own id column.

use std::mem;
use std::ops::Index;
use std::sync::Arc;

use crate::containers::col::{Col, Span};
use crate::data::codec::Codecs;
use crate::data::const_value::ConstValue;
use crate::exec::compile::consumer_cone::ConsumerCone;
use crate::exec::idx::{EventIdx, InputIdx, NodeIdx, OutputAddr, OutputIdx, SubscriberIdx};
use crate::graph::func::FuncBehavior;
use crate::graph::func::event::EventLambda;
use crate::graph::func::lambda::FuncLambda;
use crate::graph::identity::FuncRef;
use crate::graph::instance_path::InstancePath;
use crate::graph::node::CacheMode;
use crate::graph::node::special::SpecialNode;
use crate::graph::program_id::ProgramId;

#[derive(Clone, Debug, Default)]
pub(crate) enum ExecBinding {
    #[default]
    None,
    Const(ConstValue),
    Wire(OutputAddr),
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ExecInput {
    pub(crate) required: bool,
    /// Whether the port declares a path: a bound value's filesystem referent contributes to this
    /// input's digest, and a run refuses a value this system cannot read as written.
    pub(crate) fs_path: bool,
    pub(crate) binding: ExecBinding,
}

#[derive(Debug)]
pub(crate) struct ExecEvent {
    /// This event's run in [`CompiledGraph::subscribers`].
    pub(crate) subscribers: Span<SubscriberIdx>,
    pub(crate) lambda: EventLambda,
}

/// Topology + code for one lowered node. Immutable across runs; mutable per-run
/// state lives in `NodeIdx`-aligned columns, and cross-run cache slots are
/// keyed by the node's program id.
///
/// Everything here is `Copy` but the lambda, an `Arc`, so taking one off a declaration is a
/// refcount bump and the library stays readable behind it — which is what `Clone` is for.
#[derive(Debug, Clone)]
pub(crate) struct ExecNode {
    pub(crate) sink: bool,
    /// The authoring node is disabled. Ambient planning excludes it; an
    /// explicit node seed overrides it for that run.
    pub(crate) disabled: bool,
    /// Copied from the node's func. Only `Pure` is content-cacheable; the digest of an `Impure`
    /// node (or any node downstream of one) is `None`.
    pub(crate) behavior: FuncBehavior,

    /// The authoring node's cache mode, copied at lowering. Its two bits
    /// ([`caches_in_ram`](crate::graph::node::CacheMode::caches_in_ram) /
    /// [`persists_to_disk`](crate::graph::node::CacheMode::persists_to_disk)) gate RAM retention
    /// and the disk load/store; disk is honored only when the node has a content digest and a disk
    /// root is configured — see the `digest` and `disk_store` modules.
    pub(crate) cache: CacheMode,

    /// `Some` for a built-in [`SpecialNode`] (lowered from a
    /// [`BodySource::Special`](crate::graph::node::BodySource::Special) node). The planner
    /// recognizes the kind — a subscribed `RunSinks` promotes a fired event
    /// into a full sinks run — and it marks the node's interface as coming
    /// from the hardcoded spec rather than the library.
    pub(crate) special: Option<SpecialNode>,

    pub(crate) inputs: Span<InputIdx>,
    pub(crate) outputs: Span<OutputIdx>,
    pub(crate) events: Span<EventIdx>,

    pub(crate) func_ref: FuncRef,

    pub(crate) lambda: FuncLambda,
}

/// The compile artifact: the lowered, immutable program — lambdas, bindings,
/// and bound-path stamping metadata. Self-contained: executing it
/// needs neither the authoring graph nor the library.
///
/// [`Default`] is the empty artifact an engine holds before anything is installed.
#[derive(Debug, Default)]
pub struct CompiledGraph {
    /// The dense node column — every per-run column and set aligns to it. Ordered by
    /// `ProgramId`, so compiled artifacts are deterministic however the authoring graphs were
    /// walked.
    pub(crate) e_nodes: Col<NodeIdx, ExecNode>,
    /// `NodeIdx` → program id, for the host boundary (reports, seeds, eviction, cache slots) —
    /// **sorted**, so it answers the reverse direction too. See [`node`](Self::node).
    pub(crate) program_ids: Col<NodeIdx, ProgramId>,
    pub(crate) inputs: Col<InputIdx, ExecInput>,
    pub(crate) events: Col<EventIdx, ExecEvent>,
    /// Every event's subscribers, one event's run after another.
    pub(crate) subscribers: Col<SubscriberIdx, NodeIdx>,
    /// The output ports of every node, which each node's `outputs` run
    /// addresses: the length of each per-run output column.
    pub(crate) output_count: u32,
    /// The library's disk codecs when this was compiled, used by the cache to encode and decode
    /// this program's values.
    pub(crate) codecs: Arc<Codecs>,
}

impl Index<NodeIdx> for CompiledGraph {
    type Output = ExecNode;

    fn index(&self, idx: NodeIdx) -> &ExecNode {
        &self.e_nodes[idx]
    }
}

impl CompiledGraph {
    /// Where a program node landed, or `None` if this artifact holds no
    /// compiled work for it.
    ///
    /// The one place the host's ids cross into the dense space. A binary search rather than a
    /// side index: the compile places nodes in id order, so `program_ids` is already arranged for
    /// this lookup.
    pub(crate) fn node(&self, program_id: ProgramId) -> Option<NodeIdx> {
        self.program_ids.search_sorted(&program_id)
    }

    /// Whether this artifact holds compiled work for a program node — answers both "can this
    /// node seed a run" and "does a report naming this node belong to this install".
    pub fn contains(&self, program_id: ProgramId) -> bool {
        self.node(program_id).is_some()
    }

    /// Each program node inside the instances `path` names, the nodes of nested instances
    /// included, in id order.
    pub fn program_ids_within(&self, path: InstancePath) -> impl Iterator<Item = ProgramId> {
        self.program_ids
            .iter()
            .copied()
            .filter(move |program_id| program_id.ids().starts_with(path.ids()))
    }

    /// Whether any program node persists its value to disk, so a run of this program has a disk
    /// store to write to.
    pub fn persists_to_disk(&self) -> bool {
        self.e_nodes
            .iter()
            .any(|e_node| e_node.cache.persists_to_disk())
    }

    /// Every node that reads `seeds`' outputs, transitively, plus the seeds themselves — in this
    /// artifact's id order. Seeds this artifact holds no work for contribute nothing.
    ///
    /// Exactly the set a [`WorkerMessage::EvictCache`] clears, so a host that requests an eviction
    /// learns what it reaches. Allocates its scratch per call: the questions that ask this are
    /// user actions, not per-run work.
    ///
    /// [`WorkerMessage::EvictCache`]: crate::WorkerMessage::EvictCache
    pub fn consumer_cone(&self, seeds: impl IntoIterator<Item = ProgramId>) -> Vec<ProgramId> {
        let mut cone = ConsumerCone::default();
        cone.of(self, seeds.into_iter().filter_map(|id| self.node(id)))
            .iter()
            .map(|node_idx| self.program_ids[node_idx])
            .collect()
    }

    /// Move `col`, aligned to `prev`, onto this program's dense node space,
    /// pairing entries by stable id: an entry whose node both programs hold moves
    /// to its new index through `carry`, one whose node this program dropped is
    /// dropped, and a node this program adds gets `fresh`.
    ///
    /// A merge walk rather than a map: both id columns are sorted.
    pub(crate) fn realign<T>(
        &self,
        prev: &Self,
        col: &mut Col<NodeIdx, T>,
        mut carry: impl FnMut(NodeIdx, T) -> T,
        mut fresh: impl FnMut(NodeIdx) -> T,
    ) {
        debug_assert_eq!(
            col.len(),
            prev.program_ids.len(),
            "the column spans the program it is moving from"
        );
        let mut old = mem::replace(col, Col::with_capacity(self.program_ids.len()));
        let mut old = prev.program_ids.iter().copied().zip(old.drain()).peekable();
        for (node_idx, &program_id) in self.program_ids.iter_indexed() {
            while old.next_if(|(old_id, _)| *old_id < program_id).is_some() {}
            let entry = match old.next_if(|(old_id, _)| *old_id == program_id) {
                Some((_, value)) => carry(node_idx, value),
                None => fresh(node_idx),
            };
            col.push(entry);
        }
    }

    pub(crate) fn output_idx(&self, address: OutputAddr) -> OutputIdx {
        self[address.node_idx].outputs.nth(address.port_idx)
    }
}

#[cfg(feature = "internals")]
pub(crate) mod internals {
    use crate::containers::col::Span;
    use crate::exec::compile::compiled_graph::{CompiledGraph, ExecNode};
    use crate::exec::idx::NodeIdx;
    use crate::graph::func::FuncBehavior;
    use crate::graph::func::lambda::FuncLambda;
    use crate::graph::identity::FuncId;
    use crate::graph::node::CacheMode;
    use crate::graph::program_id::ProgramId;
    use crate::internals::portless_func_ref;

    impl ExecNode {
        /// An enabled, impure, uncached node with no ports, no func id yet and
        /// a body that does nothing — what a hand-built fixture starts from.
        pub(crate) fn bare() -> Self {
            Self {
                sink: false,
                disabled: false,
                behavior: FuncBehavior::Impure,
                cache: CacheMode::NEVER,
                special: None,
                inputs: Span::default(),
                outputs: Span::default(),
                events: Span::default(),
                func_ref: portless_func_ref(FuncId::nil()),
                lambda: FuncLambda::stub(),
            }
        }
    }

    impl CompiledGraph {
        /// Append one node, assigning the next dense index — the fixture form of the placement a
        /// compile performs.
        ///
        /// Ascending ids, because [`node`](CompiledGraph::node) binary-searches `program_ids`.
        /// Ascending also means unique.
        pub(crate) fn push(&mut self, id: ProgramId, e_node: ExecNode) -> NodeIdx {
            assert!(
                self.program_ids.iter().last().is_none_or(|last| *last < id),
                "a program's nodes are placed in ascending id order"
            );
            let node_idx = NodeIdx(self.e_nodes.len() as u32);
            self.program_ids.push(id);
            self.e_nodes.push(e_node);
            node_idx
        }

        /// A program of portless nodes under `program_ids` — enough for a host
        /// test that only resolves program ids against a program. Sorted on
        /// the way in, like the real walk.
        pub fn bare(program_ids: impl IntoIterator<Item = ProgramId>) -> Self {
            let mut program_ids: Vec<ProgramId> = program_ids.into_iter().collect();
            program_ids.sort_unstable();
            let mut compiled = Self::default();
            for program_id in program_ids {
                compiled.push(program_id, ExecNode::bare());
            }
            compiled
        }
    }

    /// Id lookups for a unit test that stood a program up by hand. Production paths carry
    /// `NodeIdx`, so nothing outside a test pays the search.
    #[cfg(test)]
    impl CompiledGraph {
        pub(crate) fn by_id(&self, id: ProgramId) -> &ExecNode {
            &self[self.node(id).expect("the fixture placed this node")]
        }

        pub(crate) fn by_id_mut(&mut self, id: ProgramId) -> &mut ExecNode {
            let node_idx = self.node(id).expect("the fixture placed this node");
            &mut self.e_nodes[node_idx]
        }
    }
}
