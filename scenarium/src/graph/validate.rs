//! `Graph` structural validation, including library-independent checks,
//! recursive validation against a `Library`, and debug-only invariant asserts.

use common::is_debug;
use hashbrown::HashSet;

use crate::error::GraphValidationError;
use crate::graph::interface::{GraphId, GraphLink};
use crate::graph::{Binding, Graph, NodeId, NodeKind};
use crate::library::Library;
use crate::node::definition::FuncInput;
use crate::{DataType, FsPathMode, StaticValue};

type ValidationResult<T> = Result<T, GraphValidationError>;

/// Hard cap on graph-tree nesting. Enforced here as a validation error (the
/// recursive walk below is itself stack-bound), and re-asserted as a release
/// backstop in flatten's descent.
pub(crate) const MAX_NESTING_DEPTH: usize = 256;

#[derive(Debug)]
struct GraphChecker<'a> {
    library: Option<&'a Library>,
    node_ids: HashSet<NodeId>,
    checked_shared: HashSet<GraphId>,
    shared_path: HashSet<GraphId>,
    depth: usize,
}

impl<'a> GraphChecker<'a> {
    fn new(library: Option<&'a Library>) -> Self {
        Self {
            library,
            node_ids: HashSet::new(),
            checked_shared: HashSet::new(),
            shared_path: HashSet::new(),
            depth: 0,
        }
    }

    fn validate_graph(&mut self, graph: &Graph, requires_definition: bool) -> ValidationResult<()> {
        if self.depth > MAX_NESTING_DEPTH {
            return Err(GraphValidationError::NestingTooDeep {
                max: MAX_NESTING_DEPTH,
            });
        }
        let definition = graph.definition.as_ref();
        if requires_definition && definition.is_none() {
            return Err(GraphValidationError::MissingSubgraphDefinition);
        }
        if definition
            .and_then(|definition| definition.origin)
            .is_some_and(|origin| origin.is_nil())
        {
            return Err(GraphValidationError::NilOrigin);
        }
        let mut boundary_inputs = 0usize;
        let mut boundary_outputs = 0usize;
        for (node_id, node) in &graph.nodes {
            if node_id.is_nil() {
                return Err(GraphValidationError::NilNodeId);
            }
            if !self.node_ids.insert(*node_id) {
                return Err(GraphValidationError::DuplicateNodeId { node_id: *node_id });
            }
            match &node.kind {
                NodeKind::Func(func_id) => {
                    if func_id.is_nil() {
                        return Err(GraphValidationError::NilFuncId { node_id: *node_id });
                    }
                    if self
                        .library
                        .is_some_and(|library| library.by_id(func_id).is_none())
                    {
                        return Err(GraphValidationError::MissingFunc {
                            node_id: *node_id,
                            func_id: *func_id,
                        });
                    }
                }
                NodeKind::Graph(link) => {
                    if link.id().is_nil() {
                        return Err(GraphValidationError::NilGraphId { node_id: *node_id });
                    }
                    if let GraphLink::Local(graph_id) = link
                        && !graph.graphs.contains_key(graph_id)
                    {
                        return Err(GraphValidationError::MissingLocalGraph {
                            node_id: *node_id,
                            graph_id: *graph_id,
                        });
                    }
                    if let Some(library) = self.library {
                        let nested = graph
                            .resolve_graph(*link, library)
                            .ok_or(GraphValidationError::MissingGraph { node_id: *node_id })?;
                        if let GraphLink::Shared(id) = link {
                            self.validate_shared(*id, nested)?;
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
        if boundary_inputs > 1 {
            return Err(GraphValidationError::MultipleGraphInputs {
                count: boundary_inputs,
            });
        }
        if boundary_outputs > 1 {
            return Err(GraphValidationError::MultipleGraphOutputs {
                count: boundary_outputs,
            });
        }

        // Drift is tolerated everywhere below: a binding, subscription, or
        // pin referencing a port the current library no longer declares —
        // and a wire or const whose type no longer matches — stays valid
        // and degrades to unbound at flatten time (a required input
        // surfaces as a missing-input verdict; see flatten's `typed_binding`).
        // Deleting or rejecting it would destroy authored wiring that
        // revives when the library or the upstream types come back.
        for (destination, binding) in &graph.bindings {
            if !graph.nodes.contains_key(&destination.node_id) {
                return Err(GraphValidationError::BindingMissingNode {
                    node_id: destination.node_id,
                });
            }
            if let Some(library) = self.library
                && matches!(binding, Binding::Bind(_))
                && graph
                    .input_spec(library, *destination)
                    .is_some_and(|input| input.const_only)
            {
                return Err(GraphValidationError::ConstOnlyBinding { port: *destination });
            }

            if let Binding::Bind(src) = binding
                && !graph.nodes.contains_key(&src.node_id)
            {
                return Err(GraphValidationError::BindingMissingProducer {
                    destination: *destination,
                    producer: *src,
                });
            }
        }

        for subscription in &graph.subscriptions {
            if !graph.nodes.contains_key(&subscription.emitter) {
                return Err(GraphValidationError::MissingSubscriptionEmitter {
                    node_id: subscription.emitter,
                });
            }
            if !graph.nodes.contains_key(&subscription.subscriber) {
                return Err(GraphValidationError::MissingSubscriber {
                    emitter: subscription.emitter,
                    event_idx: subscription.event_idx,
                    subscriber: subscription.subscriber,
                });
            }
        }

        for port in &graph.pinned_outputs {
            if !graph.nodes.contains_key(&port.node_id) {
                return Err(GraphValidationError::PinnedOutputMissingNode {
                    node_id: port.node_id,
                });
            }
        }

        if let Some(definition) = definition {
            for event in &definition.events {
                if !graph.nodes.contains_key(&event.emitter) {
                    return Err(GraphValidationError::ExposedEventMissingEmitter {
                        name: event.name.clone(),
                        emitter: event.emitter,
                    });
                }
            }
        }

        for (graph_id, nested) in &graph.graphs {
            if graph_id.is_nil() {
                return Err(GraphValidationError::NilLocalGraphId);
            }
            self.depth += 1;
            let nested_result = self.validate_graph(nested, true);
            self.depth -= 1;
            nested_result.map_err(|source| GraphValidationError::LocalGraph {
                name: subgraph_name(nested),
                source: Box::new(source),
            })?;
        }

        Ok(())
    }

    fn validate_shared(&mut self, graph_id: GraphId, graph: &Graph) -> ValidationResult<()> {
        if self.checked_shared.contains(&graph_id) {
            return Ok(());
        }
        if !self.shared_path.insert(graph_id) {
            return Err(GraphValidationError::RecursiveGraph {
                name: subgraph_name(graph),
            });
        }
        self.depth += 1;
        let result =
            self.validate_graph(graph, true)
                .map_err(|source| GraphValidationError::SharedGraph {
                    name: subgraph_name(graph),
                    source: Box::new(source),
                });
        self.depth -= 1;
        self.shared_path.remove(&graph_id);
        result?;
        self.checked_shared.insert(graph_id);
        Ok(())
    }
}

fn subgraph_name(graph: &Graph) -> String {
    graph
        .definition
        .as_ref()
        .map(|definition| definition.name.clone())
        .unwrap_or_else(|| "<missing definition>".to_owned())
}

impl Graph {
    /// Validate this graph and its complete local graph tree.
    pub fn validate(&self) -> ValidationResult<()> {
        GraphChecker::new(None).validate_graph(self, false)
    }

    /// Validate this graph as a reusable subgraph definition.
    pub fn validate_subgraph(&self) -> ValidationResult<()> {
        GraphChecker::new(None).validate_graph(self, true)
    }

    /// Debug-only assert form of [`Self::validate`].
    pub fn validate_debug(&self) {
        if !is_debug() {
            return;
        }
        self.validate()
            .expect("graph structural invariant violated");
    }

    /// Validate an execution entry and every local or reachable shared graph
    /// against `library`.
    pub fn validate_for_execution(&self, library: &Library) -> ValidationResult<()> {
        if self.definition.is_some() {
            return Err(GraphValidationError::EntryDefinition);
        }
        if self.nodes.values().any(|node| node.kind.is_boundary()) {
            return Err(GraphValidationError::EntryBoundaryNodes);
        }
        GraphChecker::new(Some(library)).validate_graph(self, false)
    }

    /// Debug-only assert form of [`Self::validate_for_execution`].
    pub fn validate_for_execution_debug(&self, library: &Library) {
        if !is_debug() {
            return;
        }
        self.validate_for_execution(library)
            .expect("graph structural invariant violated");
    }
}

/// Whether a `Const` literal `value` may sit on `input` — the `Const` half of
/// the flatten-time type degrade (the `Bind` half uses
/// [`DataType::compatible_with`]); a literal that doesn't satisfy its port
/// flattens as unbound. Matched directly rather than via
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
pub(crate) fn const_satisfies(library: &Library, input: &FuncInput, value: &StaticValue) -> bool {
    if !input.value_variants.is_empty() {
        return input.value_variants.iter().any(|v| v.value == *value);
    }
    // `Null` is "explicitly unset": lens's config machinery authors it on
    // optional (`Option`-field) inputs and reads it back as `None`. Keep in
    // lockstep with `FuncInput::default_fits`, the declaration-time twin.
    if matches!(value, StaticValue::Null) {
        return !input.required;
    }
    match &input.data_type {
        DataType::Any => true,
        DataType::Float | DataType::Int | DataType::Bool => matches!(
            value,
            StaticValue::Float(_) | StaticValue::Int(_) | StaticValue::Bool(_)
        ),
        DataType::String => matches!(value, StaticValue::String(_)),
        DataType::FsPath(config) => match config.mode {
            FsPathMode::ExistingFiles => matches!(value, StaticValue::FsPaths(_)),
            FsPathMode::ExistingFile | FsPathMode::NewFile | FsPathMode::Directory => {
                matches!(value, StaticValue::FsPath(_))
            }
        },
        DataType::Enum(type_id) => matches!(
            value,
            StaticValue::Enum(name)
                if library.enum_variants(type_id).is_some_and(|vs| vs.iter().any(|v| v == name))
        ),
        DataType::Custom(_) => false,
    }
}
