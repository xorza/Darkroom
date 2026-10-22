//! The compile artifact: the lowered graph — topology + code — plus every
//! question a host asks of one.
//!
//! Built once by the [`Compiler`](crate::exec::compile::Compiler)'s walk —
//! there are no mutators here, and no pass that fills a field in afterwards —
//! then installed as runtime state; it is deliberately not a persistence format.
//! Mutable state is split between the per-run schedule/executor and the
//! cross-run runtime cache.
//!
//! Self-contained: everything a run needs was copied out of the
//! [`Library`](crate::library::Library) at compile, so nothing here refers to one.
//!
//! **One authored node, one execution node, one [`NodeId`].** Nothing splits or
//! merges on the way in, so resolving an authored id against the artifact is a
//! search of the id column it already carries rather than a lookup through a
//! table beside it.

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
use crate::graph::identity::{FuncRef, NodeId};
use crate::graph::node::CacheMode;
use crate::graph::node::special::SpecialNode;

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
    /// Whether a bound value's filesystem referent contributes to this input's digest.
    pub(crate) stamps_fs_path: bool,
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
/// keyed by the node's stable id.
///
/// Built whole by the compiler's walk, straight into the dense index space the
/// id sort settled before it. Everything here is `Copy` but the lambda, and that
/// is an `Arc`, so taking one off a declaration is a refcount bump and the
/// library stays readable behind it — which is what `Clone` is for.
#[derive(Debug, Clone)]
pub(crate) struct ExecNode {
    pub(crate) sink: bool,
    /// The authoring node is disabled. Ambient planning excludes it; an
    /// explicit node seed overrides it for that run.
    pub(crate) disabled: bool,
    /// Copied from the node's func at lowering. Only `Pure` is content-cacheable;
    /// the digest of an `Impure` node (or any node downstream of one) is `None`.
    pub(crate) behavior: FuncBehavior,

    /// The authoring node's cache mode, copied from
    /// [`CacheMode`] at lowering. Its two bits
    /// ([`caches_in_ram`](crate::graph::node::CacheMode::caches_in_ram) /
    /// [`persists_to_disk`](crate::graph::node::CacheMode::persists_to_disk)) gate RAM retention
    /// and the disk load/store; disk is honored only when the node has a
    /// content digest (a reproducible cone) and a disk root is configured — see `digest.rs`
    /// and `disk_store.rs`.
    pub(crate) cache: CacheMode,

    /// `Some` for a built-in [`SpecialNode`] (lowered from
    /// [`NodeKind::Special`](crate::graph::node::NodeKind::Special)). The planner
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
/// [`Default`] is the empty artifact, which is what an engine holds before
/// anything is installed.
#[derive(Debug, Default)]
pub struct CompiledGraph {
    /// The dense node column — every per-run column and set aligns to it.
    /// Ordered by `NodeId`, so compiled artifacts and program walks are
    /// deterministic however the authoring graph was walked.
    pub(crate) e_nodes: Col<NodeIdx, ExecNode>,
    /// `NodeIdx` → authoring-derived id, for the host boundary (reports,
    /// seeds, eviction, cache slots) — **sorted**, which is what lets it answer
    /// the reverse direction too. See [`node`](Self::node).
    pub(crate) node_ids: Col<NodeIdx, NodeId>,
    pub(crate) inputs: Col<InputIdx, ExecInput>,
    pub(crate) events: Col<EventIdx, ExecEvent>,
    /// Every event's subscribers, one event's run after another.
    pub(crate) subscribers: Col<SubscriberIdx, NodeIdx>,
    /// The output ports of every node, which each node's `outputs` run
    /// addresses: the length of each per-run output column.
    pub(crate) output_count: u32,
    /// The library's disk codecs when this was compiled, which the cache
    /// encodes and decodes this program's values with.
    pub(crate) codecs: Arc<Codecs>,
}

impl Index<NodeIdx> for CompiledGraph {
    type Output = ExecNode;

    fn index(&self, idx: NodeIdx) -> &ExecNode {
        &self.e_nodes[idx]
    }
}

impl CompiledGraph {
    /// Where an authored node landed, or `None` if this artifact holds no
    /// compiled work for it.
    ///
    /// The one place the authoring space crosses into the dense one. A binary
    /// search rather than a side index: the compile places nodes in id order, so
    /// `node_ids` is *already* arranged for this lookup — a map beside it would
    /// be a second copy of that arrangement, allocated per compile and carried
    /// for the life of every installed artifact.
    pub(crate) fn node(&self, node_id: NodeId) -> Option<NodeIdx> {
        self.node_ids.search_sorted(&node_id)
    }

    /// Whether this artifact holds compiled work for an authored node.
    ///
    /// The one question the authoring space can ask of a program without
    /// entering the dense space: it answers both "can this node seed a run"
    /// and "does a report naming this node belong to this install".
    pub fn contains(&self, node_id: NodeId) -> bool {
        self.node(node_id).is_some()
    }

    /// Every node that reads `seeds`' outputs, transitively, plus the seeds
    /// themselves — in this artifact's id order. Seeds this artifact holds no
    /// work for contribute nothing.
    ///
    /// This is exactly the set a [`WorkerMessage::EvictCache`] clears, which is
    /// what it is public for: a host that requests an eviction learns what it
    /// reaches, rather than having to assume the whole program. The eviction
    /// resolves its own cone against the installed artifact, so a caller asking
    /// the artifact it installs gets the same answer.
    ///
    /// Allocates its scratch per call — the reversed edges it walks are a pure
    /// function of the artifact and are never kept, and the questions that ask
    /// this are user actions, not per-run work.
    ///
    /// [`WorkerMessage::EvictCache`]: crate::WorkerMessage::EvictCache
    pub fn consumer_cone(&self, seeds: impl IntoIterator<Item = NodeId>) -> Vec<NodeId> {
        let mut cone = ConsumerCone::default();
        cone.of(self, seeds.into_iter().filter_map(|id| self.node(id)))
            .iter()
            .map(|node_idx| self.node_ids[node_idx])
            .collect()
    }

    /// Move `col`, aligned to `prev`, onto this program's dense node space,
    /// pairing entries by stable id: an entry whose node both programs hold moves
    /// to its new index through `carry`, one whose node this program dropped is
    /// dropped, and a node this program adds gets `fresh`.
    ///
    /// A merge walk rather than a map: both id columns are sorted, so one pass
    /// over each pairs every survivor.
    pub(crate) fn realign<T>(
        &self,
        prev: &Self,
        col: &mut Col<NodeIdx, T>,
        mut carry: impl FnMut(NodeIdx, T) -> T,
        mut fresh: impl FnMut(NodeIdx) -> T,
    ) {
        debug_assert_eq!(
            col.len(),
            prev.node_ids.len(),
            "the column spans the program it is moving from"
        );
        let mut old = mem::replace(col, Col::with_capacity(self.node_ids.len()));
        let mut old = prev.node_ids.iter().copied().zip(old.drain()).peekable();
        for (node_idx, &node_id) in self.node_ids.iter_indexed() {
            while old.next_if(|(old_id, _)| *old_id < node_id).is_some() {}
            let entry = match old.next_if(|(old_id, _)| *old_id == node_id) {
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
    use crate::graph::identity::{FuncId, NodeId};
    use crate::graph::node::CacheMode;
    use crate::internals::portless_func_ref;

    impl ExecNode {
        /// An enabled, impure, uncached node with no ports, no func id yet and
        /// a body that does nothing — what a hand-built fixture starts from.
        pub(crate) fn bare() -> Self {
            Self {
                sink: false,
                disabled: false,
                behavior: FuncBehavior::Impure,
                cache: CacheMode::None,
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
        /// Append one node, assigning the next dense index — the fixture form
        /// of the placement a compile performs before it walks.
        ///
        /// Ascending ids, because [`node`](CompiledGraph::node) binary-searches
        /// `node_ids`: a fixture that pushed out of order would build a program
        /// whose own nodes it cannot find. Ascending also means unique, so one
        /// id cannot name two nodes.
        pub(crate) fn push(&mut self, id: NodeId, e_node: ExecNode) -> NodeIdx {
            assert!(
                self.node_ids.iter().last().is_none_or(|last| *last < id),
                "a program's nodes are placed in ascending id order"
            );
            let node_idx = NodeIdx(self.e_nodes.len() as u32);
            self.node_ids.push(id);
            self.e_nodes.push(e_node);
            node_idx
        }

        /// A program of portless nodes under `node_ids` — enough for a host
        /// test that only resolves authored ids against a program. Sorted on
        /// the way in, like the real walk.
        pub fn bare(node_ids: impl IntoIterator<Item = NodeId>) -> Self {
            let mut node_ids: Vec<NodeId> = node_ids.into_iter().collect();
            node_ids.sort_unstable();
            let mut compiled = Self::default();
            for node_id in node_ids {
                compiled.push(node_id, ExecNode::bare());
            }
            compiled
        }
    }

    /// Id lookups for a unit test that stood a program up by hand and knows its
    /// nodes by the ids it gave them. Production paths carry `NodeIdx`, so
    /// nothing outside a test pays the search.
    #[cfg(test)]
    impl CompiledGraph {
        pub(crate) fn by_id(&self, id: NodeId) -> &ExecNode {
            &self[self.node(id).expect("the fixture placed this node")]
        }

        pub(crate) fn by_id_mut(&mut self, id: NodeId) -> &mut ExecNode {
            let node_idx = self.node(id).expect("the fixture placed this node");
            &mut self.e_nodes[node_idx]
        }
    }
}
