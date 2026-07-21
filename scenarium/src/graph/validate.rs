//! `Graph` structural validation, including library-independent checks,
//! recursive validation against a `Library`, and debug-only invariant asserts.

use anyhow::{self, Context, Result, ensure};
use common::is_debug;
use hashbrown::HashSet;

use crate::graph::interface::{GraphId, GraphLink};
use crate::graph::{Binding, Graph, NodeId, NodeKind};
use crate::library::Library;
use crate::node::definition::FuncInput;
use crate::{DataType, StaticValue};

#[derive(Debug)]
struct GraphChecker<'a> {
    library: Option<&'a Library>,
    node_ids: HashSet<NodeId>,
    checked_shared: HashSet<GraphId>,
    shared_path: HashSet<GraphId>,
}

impl<'a> GraphChecker<'a> {
    fn new(library: Option<&'a Library>) -> Self {
        Self {
            library,
            node_ids: HashSet::new(),
            checked_shared: HashSet::new(),
            shared_path: HashSet::new(),
        }
    }

    fn check_graph(&mut self, graph: &Graph) -> Result<()> {
        ensure!(
            graph.origin.is_none_or(|origin| !origin.is_nil()),
            "graph has a nil origin"
        );
        let mut boundary_inputs = 0usize;
        let mut boundary_outputs = 0usize;
        for (node_id, node) in &graph.nodes {
            ensure!(!node_id.is_nil(), "graph contains a node with a nil id");
            ensure!(
                self.node_ids.insert(*node_id),
                "node id {:?} occurs in more than one authoring graph",
                node_id
            );
            match &node.kind {
                NodeKind::Func(func_id) => {
                    ensure!(!func_id.is_nil(), "node {:?} has a nil func_id", node_id);
                    if let Some(library) = self.library {
                        ensure!(
                            library.by_id(func_id).is_some(),
                            "node {:?} references func {:?}, absent from the library",
                            node_id,
                            func_id
                        );
                    }
                }
                NodeKind::Graph(link) => {
                    ensure!(!link.id().is_nil(), "node {:?} has a nil graph id", node_id);
                    if let GraphLink::Local(id) = link {
                        ensure!(
                            graph.graphs.contains_key(id),
                            "node {:?} references missing local graph {:?}",
                            node_id,
                            id
                        );
                    }
                    if let Some(library) = self.library {
                        let nested = graph.resolve_graph(*link, library).with_context(|| {
                            format!("node {:?} references a missing graph", node_id)
                        })?;
                        if let GraphLink::Shared(id) = link {
                            self.check_shared(*id, nested)?;
                        }
                    }
                }
                NodeKind::Special(_) => {}
                NodeKind::GraphInput => {
                    boundary_inputs += 1;
                }
                NodeKind::GraphOutput => {
                    boundary_outputs += 1;
                }
            }
        }
        ensure!(
            boundary_inputs <= 1,
            "a graph holds at most one GraphInput, found {boundary_inputs}"
        );
        ensure!(
            boundary_outputs <= 1,
            "a graph holds at most one GraphOutput, found {boundary_outputs}"
        );

        for (destination, binding) in &graph.bindings {
            let consumer = graph
                .nodes
                .get(&destination.node_id)
                .with_context(|| format!("binding on missing node {:?}", destination.node_id))?;
            if let Some(library) = self.library {
                let input_count = graph
                    .input_count(consumer, library)
                    .expect("node reference resolved before binding validation");
                ensure!(
                    destination.port_idx < input_count,
                    "binding on node {:?} input {} is out of range",
                    destination.node_id,
                    destination.port_idx
                );
                if let Binding::Bind(_) = binding {
                    ensure!(
                        !graph
                            .input_spec(library, *destination)
                            .is_some_and(|input| input.const_only),
                        "input {} on node {:?} is const-only and cannot be wired to an upstream output",
                        destination.port_idx,
                        destination.node_id
                    );
                }
            }

            if let Binding::Bind(src) = binding {
                let producer = graph.nodes.get(&src.node_id).with_context(|| {
                    format!(
                        "node {:?} input {} binds to missing node {:?}",
                        destination.node_id, destination.port_idx, src.node_id
                    )
                })?;
                if let Some(library) = self.library {
                    let output_count = graph
                        .output_count(producer, library)
                        .expect("node reference resolved before binding validation");
                    ensure!(
                        src.port_idx < output_count,
                        "binding from node {:?} output {} is out of range",
                        src.node_id,
                        src.port_idx
                    );
                    if let Some(sink_ty) = graph.input_type(library, *destination) {
                        let source_ty = graph.resolve_output_type(library, *src);
                        ensure!(
                            sink_ty.compatible_with(&source_ty),
                            "node {:?} input {} expects {:?} but is wired from an incompatible {:?}",
                            destination.node_id,
                            destination.port_idx,
                            sink_ty,
                            source_ty
                        );
                    }
                }
            }

            if let (Some(library), Binding::Const(value)) = (self.library, binding)
                && let Some(spec) = graph.input_spec(library, *destination)
            {
                ensure!(
                    const_satisfies(library, spec, value),
                    "node {:?} input {} holds a constant incompatible with its type {:?}",
                    destination.node_id,
                    destination.port_idx,
                    spec.data_type
                );
            }
        }

        for subscription in &graph.subscriptions {
            let emitter = graph.nodes.get(&subscription.emitter).with_context(|| {
                format!(
                    "subscription from missing emitter {:?}",
                    subscription.emitter
                )
            })?;
            ensure!(
                graph.nodes.contains_key(&subscription.subscriber),
                "node {:?} event {} has missing subscriber {:?}",
                subscription.emitter,
                subscription.event_idx,
                subscription.subscriber
            );
            if let Some(library) = self.library {
                let event_count = graph
                    .event_count(emitter, library)
                    .expect("node reference resolved before subscription validation");
                ensure!(
                    subscription.event_idx < event_count,
                    "subscription event index {} out of range on {:?}",
                    subscription.event_idx,
                    subscription.emitter
                );
            }
        }

        for port in &graph.pinned_outputs {
            let node = graph
                .nodes
                .get(&port.node_id)
                .with_context(|| format!("pinned output on missing node {:?}", port.node_id))?;
            if let Some(library) = self.library {
                let output_count = graph
                    .output_count(node, library)
                    .expect("node reference resolved before pinned-output validation");
                ensure!(
                    port.port_idx < output_count,
                    "pinned output on node {:?} output {} is out of range",
                    port.node_id,
                    port.port_idx
                );
            }
        }

        for event in &graph.events {
            let emitter = graph.nodes.get(&event.emitter).with_context(|| {
                format!(
                    "exposed event {:?} names missing emitter {:?}",
                    event.name, event.emitter
                )
            })?;
            if let Some(library) = self.library {
                let event_count = graph
                    .event_count(emitter, library)
                    .expect("node reference resolved before exposed-event validation");
                ensure!(
                    event.emitter_event_idx < event_count,
                    "exposed event index {} out of range on {:?}",
                    event.emitter_event_idx,
                    event.emitter
                );
            }
        }

        for (graph_id, nested) in &graph.graphs {
            ensure!(!graph_id.is_nil(), "local graph has a nil id");
            self.check_graph(nested)
                .map_err(|error| anyhow::anyhow!("in local graph {:?}: {error:#}", nested.name))?;
        }

        Ok(())
    }

    fn check_shared(&mut self, graph_id: GraphId, graph: &Graph) -> Result<()> {
        if self.checked_shared.contains(&graph_id) {
            return Ok(());
        }
        ensure!(
            self.shared_path.insert(graph_id),
            "graph {:?} is recursive (contains itself)",
            graph.name
        );
        let result = self
            .check_graph(graph)
            .map_err(|error| anyhow::anyhow!("in shared graph {:?}: {error:#}", graph.name));
        self.shared_path.remove(&graph_id);
        result?;
        self.checked_shared.insert(graph_id);
        Ok(())
    }
}

impl Graph {
    /// Validate this reusable graph and its complete local graph tree.
    pub fn check(&self) -> Result<()> {
        GraphChecker::new(None).check_graph(self)
    }

    /// Debug-only assert form of [`Self::check`].
    pub fn debug_check(&self) {
        if !is_debug() {
            return;
        }
        self.check().expect("graph structural invariant violated");
    }

    /// Validate an execution entry and every local or reachable shared graph
    /// against `library`.
    pub fn check_for_execution(&self, library: &Library) -> Result<()> {
        ensure!(
            self.inputs.is_empty() && self.outputs.is_empty() && self.events.is_empty(),
            "entry graph cannot expose an interface"
        );
        ensure!(
            self.nodes.values().all(|node| !node.kind.is_boundary()),
            "entry graph cannot contain interface boundary nodes"
        );
        GraphChecker::new(Some(library)).check_graph(self)
    }

    /// Debug-only assert form of [`Self::check_for_execution`].
    pub fn debug_check_for_execution(&self, library: &Library) {
        if !is_debug() {
            return;
        }
        self.check_for_execution(library)
            .expect("graph structural invariant violated");
    }
}

/// Whether a `Const` literal `value` may sit on `input` — the `Const` half of
/// the compile-boundary type check (the `Bind` half uses
/// [`DataType::compatible_with`]). Matched directly rather than via
/// `compatible_with` because a bare `StaticValue` can't be turned back into a
/// `DataType` (it lacks the `FsPathConfig`, and the enum's variant list lives in
/// `library`).
///
/// An input carrying `value_variants` is a *pick-or-wire* port (e.g. lens's
/// preset-or-config inputs, which are `Custom`-typed for the wired case yet
/// hold an `Enum` preset literal): its constant must be exactly one of the
/// offered picks. Otherwise the literal must match the declared type — scalar
/// numerics coerce, an `Enum` literal must name a registered variant, and a
/// `Custom` port has no literal form.
fn const_satisfies(library: &Library, input: &FuncInput, value: &StaticValue) -> bool {
    if !input.value_variants.is_empty() {
        return input.value_variants.iter().any(|v| v.value == *value);
    }
    match &input.data_type {
        DataType::Any => true,
        DataType::Float | DataType::Int | DataType::Bool => matches!(
            value,
            StaticValue::Float(_) | StaticValue::Int(_) | StaticValue::Bool(_)
        ),
        DataType::String => matches!(value, StaticValue::String(_)),
        DataType::FsPath(_) => matches!(value, StaticValue::FsPath(_)),
        DataType::Enum(type_id) => matches!(
            value,
            StaticValue::Enum(name)
                if library.enum_variants(type_id).is_some_and(|vs| vs.iter().any(|v| v == name))
        ),
        DataType::Custom(_) => false,
    }
}
