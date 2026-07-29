//! Graph flattening: the pipeline's first stage. Walks the authoring `Graph`,
//! expands every composite instance into its interior func nodes, and appends
//! them straight into the [`FlatGraph`] it returns — no intermediate `Graph` is
//! materialized and no final [`Program`](crate::execution::program::Program)
//! is touched. Boundary nodes
//! (`GraphInput`/`GraphOutput`) and composites dissolve; their edges are
//! short-circuited so the result is a flat, func-only graph on which
//! the existing scheduler (dead-branch pruning, caching, cycle detection)
//! works across composite boundaries unchanged. Both data bindings and event
//! subscriptions are short-circuited across boundaries (exposed events resolve
//! to their interior emitter; triggering a composite reaches the interior
//! nodes wired to its `GraphInput`).
//! Data-edge and event-edge routing live in separate private modules; this
//! module owns only traversal, descent state, and leaf emission.
//!
//! Everything here is in the **stable-id space**: the walk names producers,
//! subscribers, and emitters by [`ExecutionNodeId`] because that is all it can
//! know — a node's dense index is its position after the id sort, which is
//! linking's to assign. So no type in this module mentions `NodeIdx`,
//! `OutputIdx`, or `OutputAddr`, and each of its port types is the stage-local
//! half of a program one.
//!
//! Flattening is lossy — composites and their boundary edges dissolve — so
//! where each flat node came from is recorded as the walk goes, in the [`Leaf`]
//! and source-map-owned scope table: the raw half of attribution, in authored
//! ids and scope indices. The
//! [`source_map`](crate::execution::source_map) turns them into the queryable,
//! placed attribution held by the artifact.
//!
//! See `README.md` Part A §5.

mod bindings;
mod events;

use hashbrown::HashSet;

use crate::DataType;
use crate::execution::compile::flat::{FlatEvent, FlatGraph, FlatInput, FlatNode, FlatOutput};
use crate::execution::identity::ExecutionNodeId;
use crate::execution::source_map::Leaf;
use crate::graph::Graph;
use crate::graph::MAX_NESTING_DEPTH;
use crate::graph::definition::GraphLink;
use crate::graph::func::{Func, OutputType};
use crate::graph::identity::NodeId;
use crate::graph::identity::{GraphId, InputPort};
use crate::graph::node::NodeKind;
use crate::graph::node::special::SpecialNode;
use crate::library::Library;

/// Reusable traversal scratch owned by the
/// [`Compiler`](crate::execution::compile::Compiler). Only the descent's own
/// state lives here; everything the walk *produces* goes straight into the
/// [`FlatGraph`] it returns, so no part of one flatten survives into the next.
/// The per-build resolved-graph stack lives on `Run` (it borrows the build's
/// graph), keeping this struct free of borrowed references.
#[derive(Debug, Default)]
pub(super) struct Flattener {
    path: Vec<NodeId>,
    /// Scope indices parallel to the emit-descent in `path` — the scope each
    /// level's nodes live in.
    scope_stack: Vec<u32>,
    /// Shared graphs currently on the emit-descent path.
    seen_shared: HashSet<GraphId>,
    /// One node's resolved inputs, refilled per node. They are resolved before
    /// the ports are appended, so a [`FlatInput`] is whole the moment it exists.
    node_inputs: Vec<FlatInput>,
}

impl Flattener {
    /// Lower `root` against `library` into a flat graph. One step, one value:
    /// the walk descends the authoring graph and appends what it finds, and
    /// what comes back is complete — no field of it is filled in later and no
    /// final program or artifact type is touched on the way.
    pub(super) fn flatten(&mut self, root: &Graph, library: &Library) -> FlatGraph {
        self.path.clear();
        self.seen_shared.clear();
        self.node_inputs.clear();
        // The descent opens child scopes under the root, which every walk
        // starts on.
        self.scope_stack.clear();
        self.scope_stack.push(0);

        let mut flat = FlatGraph::default();
        let mut run = Run {
            library,
            path: &mut self.path,
            levels: vec![root],
            scope_stack: &mut self.scope_stack,
            seen_shared: &mut self.seen_shared,
            node_inputs: &mut self.node_inputs,
            flat: &mut flat,
        };
        run.emit(false);
        flat
    }
}

/// One flattening pass. Borrows the reusable `path` buffer from `Flattener`;
/// `levels` carries the resolved graph per descent level (root at the
/// bottom), so the current graph is one stack read, not a root re-walk.
#[derive(Debug)]
struct Run<'a> {
    library: &'a Library,
    path: &'a mut Vec<NodeId>,
    /// Resolved graphs parallel to `path`, plus the root at the bottom:
    /// `levels.len() == path.len() + 1` and `levels.last()` is the current
    /// level's graph.
    levels: Vec<&'a Graph>,
    /// Scope indices parallel to `path` (the emit descent). `last()` is the
    /// scope the current level's nodes live in.
    scope_stack: &'a mut Vec<u32>,
    seen_shared: &'a mut HashSet<GraphId>,
    /// Per-node scratch — see [`Flattener::node_inputs`].
    node_inputs: &'a mut Vec<FlatInput>,
    /// Everything the walk produces, appended as it goes.
    flat: &'a mut FlatGraph,
}

impl<'a> Run<'a> {
    fn current(&self) -> &'a Graph {
        self.levels
            .last()
            .copied()
            .expect("the root level always exists")
    }

    /// Descend one composite level. A release-build backstop against stack
    /// overflow — validation already rejected trees past the cap, but its
    /// shared-graph memoization measures a graph's depth only at first
    /// encounter, so flatten re-checks the true instance depth. Compile is a
    /// cold path; the assert stays in release.
    fn push_level(&mut self, instance_id: NodeId, graph: &'a Graph) {
        assert!(
            self.path.len() < MAX_NESTING_DEPTH,
            "graph nesting exceeds {MAX_NESTING_DEPTH} levels"
        );
        self.path.push(instance_id);
        self.levels.push(graph);
    }

    /// Ascend one composite level — the inverse of [`Self::push_level`].
    fn pop_level(&mut self) {
        self.path.pop().expect("cannot pop the root level");
        self.levels.pop().unwrap();
    }

    /// Open a scope for `instance_id` under the current one, returning its
    /// index. Parents always precede their children, which is what makes a
    /// leaf's walk to the root terminate.
    fn push_scope(&mut self, instance_id: NodeId) -> u32 {
        let parent = *self.scope_stack.last().unwrap();
        self.flat.scopes.push(instance_id, parent)
    }

    fn execution_node_id(&mut self, node_id: NodeId) -> ExecutionNodeId {
        self.path.push(node_id);
        let e_node_id = ExecutionNodeId::from_authoring(self.path);
        self.path.pop();
        e_node_id
    }

    /// Emit execution nodes for the current level's graph, recursing into
    /// composite instances.
    fn emit(&mut self, ancestor_disabled: bool) {
        let graph = self.current();

        for node in graph.iter() {
            let disabled = ancestor_disabled || node.disabled;
            // A graph recurses; boundary nodes emit nothing. A func or a
            // special node both resolve to a `&Func` spec and emit one leaf —
            // the spec is the only difference (`library` vs. the hardcoded
            // `SpecialNode::func`), so the emit body below is shared.
            let (func, special): (&Func, Option<SpecialNode>) = match &node.kind {
                NodeKind::Func(func_id) => (
                    self.library
                        .by_id(*func_id)
                        .expect("func resolved by update's validate_for_execution validation"),
                    None,
                ),
                NodeKind::Special(s) => (s.func(), Some(*s)),
                NodeKind::Graph(link) => {
                    self.emit_instance(node.id, *link, disabled);
                    continue;
                }
                NodeKind::GraphInput | NodeKind::GraphOutput => continue,
            };

            let e_node_id = self.execution_node_id(node.id);

            // Every port is read fresh from the func each build (never carried
            // over from the last one): the library can evolve between updates —
            // a changed `required` flag, a grown input list, a retyped output —
            // and this is where that lands.
            //
            // Bindings are resolved *before* the ports are appended, so each
            // input is whole when it enters the pool rather than being revisited
            // by index afterwards.
            self.node_inputs.clear();
            for (port_idx, func_input) in func.inputs.iter().enumerate() {
                let port = InputPort::new(node.id, port_idx);
                let binding = self.typed_binding(graph, func_input, graph.bindings.get(&port));
                self.node_inputs.push(FlatInput {
                    required: func_input.required,
                    stamps_fs_path: matches!(&func_input.data_type, DataType::FsPath(_)),
                    binding,
                });
            }
            let inputs = self.flat.inputs.append(self.node_inputs.drain(..));

            let outputs =
                self.flat.outputs.append(func.outputs.iter().map(
                    |func_output| match &func_output.ty {
                        OutputType::Fixed(data_type) => FlatOutput::Fixed(data_type.clone()),
                        // The mirrored input's declared type comes along so linking
                        // can resolve a `Const` mirror without the library.
                        OutputType::Wildcard { mirrors } => FlatOutput::Wildcard {
                            mirrors: *mirrors as u32,
                            mirrored_declared: func.inputs[*mirrors].data_type.clone(),
                        },
                    },
                ));

            let events = self
                .flat
                .events
                .append(func.events.iter().map(|func_event| FlatEvent {
                    lambda: func_event.event_lambda.clone(),
                }));

            // Id uniqueness is enforced when linking places these, so the walk
            // just appends. The leaf goes in the same push — where this flat
            // node came from (current scope + authoring id) is known here and
            // nowhere later.
            self.flat.nodes.push(FlatNode {
                id: e_node_id,
                leaf: Leaf {
                    scope: *self.scope_stack.last().unwrap(),
                    node_id: node.id,
                },
                sink: func.sink,
                disabled,
                behavior: func.behavior,
                cache: node.cache,
                special,
                inputs,
                outputs,
                events,
                func_id: func.id,
                version: func.version,
                lambda: func.lambda.clone(),
            });
        }

        if !ancestor_disabled {
            self.collect_subscriptions(graph);
        }
    }

    /// Dissolve one composite instance: open its scope, emit its interior in
    /// place, and leave every stack as it was found.
    fn emit_instance(&mut self, instance_id: NodeId, link: GraphLink, disabled: bool) {
        let shared_id = match link {
            GraphLink::Shared(id) => Some(id),
            GraphLink::Local(_) => None,
        };
        if let Some(id) = shared_id
            && !self.seen_shared.insert(id)
        {
            panic!("recursive shared graph {id:?} (it contains itself)");
        }
        let nested = self
            .current()
            .resolve_graph(link, self.library)
            .expect("graph node references a missing graph");

        self.push_level(instance_id, &nested.body);
        self.record_exposed_outputs(instance_id, nested);
        // Open this instance's scope under the current one; its interior
        // nodes record their leaves against it.
        let scope = self.push_scope(instance_id);
        self.scope_stack.push(scope);

        self.emit(disabled);

        self.scope_stack.pop();
        self.pop_level();
        if let Some(id) = shared_id {
            self.seen_shared.remove(&id);
        }
    }
}

#[cfg(test)]
mod tests;
