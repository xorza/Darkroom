//! The compile artifact: what an installed [`CompiledGraph`] *is*, and every
//! question a host asks of one.
//!
//! [`CompiledGraph`] pairs the immutable [`Program`] a run executes with the
//! indices that answer for it in *authoring* terms — which execution nodes an
//! authored node covers, which authored node an execution id came from, what a
//! "run this node" request should seed. Those relations cannot be derived from
//! the program: flattening dissolves composites, so they are built once, at
//! link, and read for the life of the install.
//!
//! Building one belongs to the compiler's link stage. Everything here is the
//! finished artifact answering for itself.

use hashbrown::HashMap;

use crate::common::column::Column;
use crate::common::pool::{Pool, PoolRange};
use crate::common::set::IdxSet;
use crate::execution::error::ExecutionIdentityError;
use crate::execution::identity::ExecutionNodeId;
use crate::execution::identity::NodeIdx;
use crate::execution::program::{ExecutionBinding, Program};
use crate::execution::source_map::{Attribution, AttributionValidationError};
use crate::graph::func::FuncBehavior;
use crate::graph::identity::NodeId;

/// The compile artifact: the flattened, immutable program (lambdas, resolved
/// output types, and bound-path stamping metadata) plus the `Attribution` that
/// names the authored node behind each execution node. Self-contained — executing
/// it needs neither the authoring graph nor the library.
#[derive(Debug)]
pub struct CompiledGraph {
    /// The one canonical runtime program. The outer `CompiledGraph` may be
    /// shared by the engine and its cache, but the program itself has no
    /// independent owner or allocation.
    pub(crate) program: Program,
    /// Each execution node's authored origin, dense in the program's index
    /// space — the walk's own column, adopted as it came.
    attribution: Attribution,
    /// The packed backing of all three relations below: each of them owns
    /// runs of this one buffer rather than a `Vec` of its own per key.
    node_lists: Pool<NodeIdx>,
    /// Authored node → every execution node covering it, ascending.
    ///
    /// The inverse of [`Attribution::of`], and the direction every
    /// question a host asks runs in: what does running this node mean, what
    /// does evicting it reach, is it a sink. Attribution answers one node at a
    /// time, so deriving this on demand costs a walk of the whole program per
    /// question — and the editor asks two per node per frame. Inverting once
    /// here is the same single pass, paid at compile.
    footprints: HashMap<NodeId, PoolRange<NodeIdx>>,
    /// Data edges reversed: which nodes read each node's outputs. A pure
    /// function of the program, so it is built with the program rather than
    /// rebuilt by each caller that needs it.
    ///
    /// A column rather than a map: the key is a dense index, and the two
    /// walks that read it — `run_targets` per footprint node, the eviction
    /// closure per node reached — ask for one node after another.
    consumers: Column<NodeIdx, PoolRange<NodeIdx>>,
    /// Graph instance → the execution nodes behind its exposed output
    /// ports, resolved from the pairs the flatten walk recorded.
    ///
    /// Not derivable from `consumers`: flattening removes the
    /// `GraphOutput` edges, so this is the only surviving record of which
    /// interior nodes an instance exists to produce.
    exposed: HashMap<NodeId, PoolRange<NodeIdx>>,
}

/// Group `entries` by authored node, each group one packed run of `lists`.
///
/// Sorted as pairs, so a group is contiguous *and* ascending by index
/// inside — the order [`CompiledGraph::run_targets`] binary-searches.
fn pack_groups(
    mut entries: Vec<(NodeId, NodeIdx)>,
    lists: &mut Pool<NodeIdx>,
) -> HashMap<NodeId, PoolRange<NodeIdx>> {
    entries.sort_unstable();
    let mut ranges = HashMap::new();
    for group in entries.chunk_by(|left, right| left.0 == right.0) {
        ranges.insert(
            group[0].0,
            lists.append(group.iter().map(|&(_, node_idx)| node_idx)),
        );
    }
    ranges
}

impl CompiledGraph {
    /// Index the three relations every host query needs, over a program and
    /// the attribution beside it. Called once both are final — the last step of
    /// linking, and the only production way a `CompiledGraph` comes into
    /// being, since the indices are not optional state something could attach
    /// afterwards.
    ///
    /// Each relation is collected flat, then grouped into one shared
    /// buffer, so all three cost one allocation between them rather than a
    /// `Vec` per authored node and per producer — in a structure the host
    /// holds for as long as the compiled graph lives.
    pub(super) fn indexed(
        program: Program,
        attribution: Attribution,
        exposed_pairs: Vec<(NodeId, ExecutionNodeId)>,
    ) -> Self {
        let mut node_lists = Pool::default();

        let exposed_producers = exposed_pairs
            .into_iter()
            .map(|(instance, producer)| {
                // Every producer recorded is a node the same walk emitted.
                // Dropping one it could not find would leave the instance
                // without the record `run_targets` exists to read — the
                // silent miss the record was added to end.
                let node_idx = *program
                    .e_node_index
                    .get(&producer)
                    .expect("flatten records exposed producers it emitted");
                (instance, node_idx)
            })
            .collect();
        let exposed = pack_groups(exposed_producers, &mut node_lists);

        // Invert the walk's column: attribution answers one node, this answers
        // one authored id, and the editor asks the latter twice per node per
        // frame.
        let mut occurrences = Vec::new();
        for (node_idx, _) in program.e_nodes.iter_indexed() {
            occurrences.extend(attribution.of(node_idx).map(|node_id| (node_id, node_idx)));
        }
        let footprints = pack_groups(occurrences, &mut node_lists);

        let mut edges = Vec::new();
        for (node_idx, e_node) in program.e_nodes.iter_indexed() {
            for input in &program.inputs[e_node.inputs] {
                if let ExecutionBinding::Bind(address) = &input.binding {
                    edges.push((address.node_idx, node_idx));
                }
            }
        }
        // The same grouping as `pack_groups`, into a column: the key is a
        // dense index, so every read is an offset rather than a hash.
        edges.sort_unstable();
        let mut consumers = Column::default();
        consumers.reset(program.e_nodes.len(), PoolRange::default());
        for group in edges.chunk_by(|left, right| left.0 == right.0) {
            consumers[group[0].0] = node_lists.append(group.iter().map(|&(_, reader)| reader));
        }

        Self {
            program,
            attribution,
            node_lists,
            footprints,
            consumers,
            exposed,
        }
    }

    /// Every execution node an authored node covers — its *footprint* —
    /// ascending, empty when it covers no compiled work.
    ///
    /// A leaf in the entry graph covers itself; a leaf inside a definition
    /// covers one occurrence per instance of that definition; a graph instance
    /// covers its whole flattened interior. This is the only way from an
    /// authored id to execution ids: a composite dissolves at flatten time and
    /// has no id of its own, so *deriving* one
    /// ([`ExecutionNodeId::from_authoring`]) answers for a top-level leaf and
    /// nothing else.
    fn footprint(&self, node_id: NodeId) -> &[NodeIdx] {
        self.footprints
            .get(&node_id)
            .map_or(&[][..], |&range| &self.node_lists[range])
    }

    /// The nodes reading `node_idx`'s outputs. Empty when nothing does.
    fn consumers_of(&self, node_idx: NodeIdx) -> &[NodeIdx] {
        &self.node_lists[self.consumers[node_idx]]
    }

    /// Attribute one flat execution id to its authored node followed by every
    /// enclosing graph instance, innermost first.
    ///
    /// The program resolves the host's stable id, exactly as a seed or a
    /// report row is resolved; everything after that is a dense read.
    pub fn attribution(
        &self,
        e_node_id: ExecutionNodeId,
    ) -> Result<impl Iterator<Item = NodeId> + '_, ExecutionIdentityError> {
        let node_idx = *self
            .program
            .e_node_index
            .get(&e_node_id)
            .ok_or(ExecutionIdentityError::NodeNotFound { e_node_id })?;
        Ok(self.attribution.of(node_idx))
    }

    /// Whether an authored node performs sink work — runs for its effect
    /// rather than for a value some consumer reads.
    ///
    /// A func is one when its declaration says so. A graph instance is one
    /// when anything inside it is: a sinks run reaches that interior sink
    /// either way, and disabling or subscribing the instance is what governs
    /// it. Having outputs of its own does not stop a composite being a sink,
    /// the way a portless func signals it.
    ///
    /// `None` where the node covers no compiled work — a boundary node, a
    /// definition no instance reaches, or a program not built yet. There is
    /// nothing to answer from, so the caller keeps whatever the authoring
    /// graph alone tells it.
    pub fn is_sink(&self, node_id: NodeId) -> Option<bool> {
        let footprint = self.footprint(node_id);
        (!footprint.is_empty()).then(|| footprint.iter().any(|&idx| self.program.e_nodes[idx].sink))
    }

    /// Whether an authored node holds work that recomputes every run.
    ///
    /// An impure node has no content digest, so nothing keys a cache on it.
    /// A graph instance inherits that from its interior: one impure node in
    /// there is enough for the instance to stop being reusable as a whole,
    /// even though its pure upstream still caches.
    ///
    /// `None` on a node with no footprint, as in [`Self::is_sink`].
    pub fn is_impure(&self, node_id: NodeId) -> Option<bool> {
        let footprint = self.footprint(node_id);
        (!footprint.is_empty()).then(|| {
            footprint
                .iter()
                .any(|&idx| self.program.e_nodes[idx].behavior == FuncBehavior::Impure)
        })
    }

    /// The execution nodes a "run this node" seeds: those producing what the
    /// node exposes, plus any sink it contains.
    ///
    /// Stated without naming a node kind — an occurrence qualifies when it
    /// is a sink, or when its value leaves the footprint (something outside
    /// consumes it, or nothing does). For a leaf that is the node itself,
    /// exactly as before; for a graph instance it is the interior producers
    /// behind its output ports plus its interior sinks, and *not* the
    /// interior wiring between them — that still runs, as their upstream
    /// cone.
    ///
    /// Empty when the node has no footprint at all: a boundary node, or one
    /// absent from this program.
    pub fn run_targets(&self, node_id: NodeId) -> Vec<ExecutionNodeId> {
        let footprint = self.footprint(node_id);
        // Membership is a search, and a silently wrong one on an unsorted
        // footprint — the ascending order is `pack_groups`' pair sort.
        debug_assert!(footprint.is_sorted());
        let inside = |node_idx: &NodeIdx| footprint.binary_search(node_idx).is_ok();
        // What the instance exposes, taken from the record flatten kept
        // rather than inferred. "Its value leaves the footprint" is not
        // observable in the finished program: the `GraphOutput` edge that
        // carried it is gone, so an exposed producer that an interior node
        // also reads looked purely internal and dropped out of the seeds —
        // while a dead interior terminal, with no readers at all, stayed
        // in. The request then ran the wrong cone entirely.
        let exposed = self
            .exposed
            .get(&node_id)
            .map_or(&[][..], |&range| &self.node_lists[range]);
        footprint
            .iter()
            .filter(|&&node_idx| {
                let readers = self.consumers_of(node_idx);
                self.program.e_nodes[node_idx].sink
                    || exposed.contains(&node_idx)
                    || readers.is_empty()
                    || !readers.iter().all(inside)
            })
            .map(|&node_idx| self.program.e_node_ids[node_idx])
            .collect()
    }

    /// Resolve authored nodes or graph instances to their flattened occurrences,
    /// then return their reflexive transitive closure over data-consumer edges.
    pub(crate) fn data_consumer_closure(
        &self,
        authored_node_ids: &[NodeId],
    ) -> Vec<ExecutionNodeId> {
        let mut in_closure = IdxSet::default();
        in_closure.reset(self.program.e_nodes.len());
        let mut pending: Vec<NodeIdx> = authored_node_ids
            .iter()
            .flat_map(|node_id| self.footprint(*node_id))
            .copied()
            .filter(|&node_idx| {
                // Two authored ids can name overlapping footprints — an
                // instance and something inside it — so the seeds dedup too.
                let fresh = !in_closure.contains(node_idx);
                in_closure.insert(node_idx);
                fresh
            })
            .collect();
        while let Some(node_idx) = pending.pop() {
            for &consumer_idx in self.consumers_of(node_idx) {
                if !in_closure.contains(consumer_idx) {
                    in_closure.insert(consumer_idx);
                    pending.push(consumer_idx);
                }
            }
        }

        let closure: Vec<ExecutionNodeId> = in_closure
            .iter()
            .map(|node_idx| self.program.e_node_ids[node_idx])
            .collect();
        debug_assert!(
            closure.is_sorted(),
            "dense indices are assigned in id order, so an ascending index walk yields ascending ids"
        );
        closure
    }

    /// Check the private source relation against the program index space it
    /// answers for. Compile owns the complete artifact validation; this narrow
    /// method keeps the attribution field encapsulated here.
    pub(crate) fn validate_attribution(&self) -> Result<(), AttributionValidationError> {
        self.attribution.validate(self.program.e_nodes.len())
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use std::ops::Deref;
    use std::sync::Arc;

    use hashbrown::HashMap;

    use crate::common::column::Column;
    use crate::common::pool::Pool;
    use crate::execution::compiled::CompiledGraph;
    use crate::execution::identity::ExecutionNodeId;
    use crate::execution::program::Program;
    use crate::execution::source_map::Attribution;
    use crate::graph::identity::NodeId;

    impl CompiledGraph {
        /// Every execution node an authored node covers, in ascending id
        /// order — `CompiledGraph::footprint` spelled out.
        ///
        /// Production never needs the set itself, only the questions asked of
        /// it (`run_targets`, `is_sink`, `is_impure`, `data_consumer_closure`),
        /// so this exists to test the relation those four share once rather
        /// than four times through their filters.
        pub(crate) fn occurrences(&self, node_id: NodeId) -> Vec<ExecutionNodeId> {
            self.footprint(node_id)
                .iter()
                .map(|&node_idx| self.program.e_node_ids[node_idx])
                .collect()
        }

        /// Build an artifact around a hand-built program, leaving every host
        /// index empty. Production artifacts can only be created by the
        /// linker, which supplies the source relations alongside the program —
        /// so this is reached through [`TestCompiledGraph`] rather than
        /// standing on its own.
        fn from_program(program: Program) -> Self {
            Self {
                program,
                attribution: Attribution::default(),
                node_lists: Pool::default(),
                footprints: HashMap::default(),
                consumers: Column::default(),
                exposed: HashMap::default(),
            }
        }
    }

    /// Convenience owner for tests that hand-build a [`Program`] but still install
    /// the same outer artifact production uses.
    #[derive(Debug)]
    pub(crate) struct TestCompiledGraph {
        compiled: Arc<CompiledGraph>,
    }

    impl TestCompiledGraph {
        pub(crate) fn new(program: Program) -> Self {
            Self {
                compiled: Arc::new(CompiledGraph::from_program(program)),
            }
        }

        pub(crate) fn program_mut(&mut self) -> &mut Program {
            &mut Arc::get_mut(&mut self.compiled)
                .expect("the fixture is built before its artifact is shared")
                .program
        }

        pub(crate) fn shared(&self) -> &Arc<CompiledGraph> {
            &self.compiled
        }
    }

    impl Default for TestCompiledGraph {
        fn default() -> Self {
            Self::new(Program::default())
        }
    }

    impl Deref for TestCompiledGraph {
        type Target = Program;

        fn deref(&self) -> &Self::Target {
            &self.compiled.program
        }
    }
}
