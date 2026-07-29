//! A reusable graph definition: the [`GraphDef`] type — what it exposes
//! plus the [`Graph`] body implementing it — and everything asked of one,
//! alongside the vocabulary for naming one: its [`GraphId`], the
//! [`GraphEvent`]s it re-exports, and the [`GraphLink`] a node reaches it by.
//!
//! One impl, like [`Graph`]'s in [`super`]: the builders that assemble a
//! definition and every question asked of one, in the file that defines it.

use ::serde::{Deserialize, Serialize};
use hashbrown::HashSet;

use crate::graph::Graph;
use crate::graph::error::{GraphValidationError, ValidationResult};
use crate::graph::func::{FuncInput, FuncOutput};
use crate::graph::identity::{GraphId, NodeId};
use crate::graph::interface::{NodeEvents, NodePorts};
use crate::library::Library;

/// A reusable graph definition: what it exposes — identity in a palette,
/// the ports an instance node declares, the events it re-exports — plus the
/// [`Graph`] body implementing them.
///
/// Distinct from [`Graph`] — an entry graph, which exposes nothing and
/// cannot be instantiated — so "a definition exposes ports" is a type fact
/// rather than a validated invariant. Not `Clone`, for the same reason
/// `Graph` isn't: see [`Self::clone_mapped`] and [`Self::clone_verbatim`].
///
/// Deliberately *not* `Deref<Target = Graph>`: an inherited method would see
/// only the body, silently skipping the exposed half on anything whole-value
/// (`validate` would check none of it; `serialize` would drop it). Reach the
/// body explicitly through `body`.
#[derive(Default, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct GraphDef {
    pub name: String,
    pub category: String,

    /// Exposed ports in port order. `inputs[i]` corresponds to `GraphInput`
    /// output port `i`; `outputs[j]` corresponds to `GraphOutput` input port
    /// `j`.
    #[serde(default)]
    pub inputs: Vec<FuncInput>,
    #[serde(default)]
    pub outputs: Vec<FuncOutput>,

    /// Exposed outgoing events re-exported from interior emitters.
    #[serde(default)]
    pub events: Vec<GraphEvent>,

    /// Shared-library graph this value was copied from, if any.
    #[serde(default)]
    pub origin: Option<GraphId>,

    #[serde(default)]
    pub body: Graph,
}

impl GraphDef {
    /// An empty definition named `name`, exposing no ports yet.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Default::default()
        }
    }
    pub fn category(mut self, category: impl Into<String>) -> Self {
        self.category = category.into();
        self
    }
    pub fn input(mut self, input: FuncInput) -> Self {
        self.inputs.push(input);
        self
    }
    pub fn inputs(mut self, inputs: impl IntoIterator<Item = FuncInput>) -> Self {
        self.inputs.extend(inputs);
        self
    }
    pub fn output(mut self, output: FuncOutput) -> Self {
        self.outputs.push(output);
        self
    }
    pub fn outputs(mut self, outputs: impl IntoIterator<Item = FuncOutput>) -> Self {
        self.outputs.extend(outputs);
        self
    }
    pub fn event(mut self, event: GraphEvent) -> Self {
        self.events.push(event);
        self
    }
    pub fn events(mut self, events: impl IntoIterator<Item = GraphEvent>) -> Self {
        self.events.extend(events);
        self
    }
    pub fn origin(mut self, origin: GraphId) -> Self {
        self.origin = Some(origin);
        self
    }
    /// What this definition exposes, as instance ports — what a node linking
    /// to it declares.
    pub fn ports(&self) -> NodePorts<'_> {
        NodePorts {
            name: &self.name,
            description: None,
            inputs: &self.inputs,
            outputs: &self.outputs,
            events: NodeEvents::Graph(&self.events),
            func: None,
        }
    }
    /// [`Graph::clone_mapped`] for a definition: remapped identities and no
    /// library lineage. The exposed half travels along, with exposed-event
    /// emitters following the remapped interior nodes.
    pub fn clone_mapped(&self) -> Self {
        let mapped = self.body.clone_mapped_with_ids();
        let events = self
            .events
            .iter()
            .map(|event| GraphEvent {
                name: event.name.clone(),
                emitter: mapped
                    .node_ids
                    .get(&event.emitter)
                    .copied()
                    .unwrap_or(event.emitter),
                emitter_event_idx: event.emitter_event_idx,
            })
            .collect();
        Self {
            name: self.name.clone(),
            category: self.category.clone(),
            inputs: self.inputs.clone(),
            outputs: self.outputs.clone(),
            events,
            origin: None,
            body: mapped.graph,
        }
    }
    /// [`Graph::clone_verbatim`] for a definition, with the same soundness
    /// condition.
    pub fn clone_verbatim(&self) -> Self {
        Self {
            name: self.name.clone(),
            category: self.category.clone(),
            inputs: self.inputs.clone(),
            outputs: self.outputs.clone(),
            events: self.events.clone(),
            origin: self.origin,
            body: self.body.clone_verbatim(),
        }
    }
    /// Validate this definition and its complete local graph tree,
    /// structurally: its interface, then the body implementing it. Distinct
    /// from [`Graph::validate`] on the body, which checks no interface at all.
    pub fn validate(&self) -> ValidationResult<()> {
        self.validate_shape(&mut HashSet::new(), &mut HashSet::new(), 0)
    }

    /// Validate this definition against `library`: structurally first, then
    /// every reference it makes — [`Graph::validate_with`] for a definition.
    pub fn validate_with(&self, library: &Library) -> ValidationResult<()> {
        self.validate()?;
        self.validate_references(library, &mut HashSet::new(), &mut HashSet::new(), 0)
    }

    /// The interface's own checks, then the body's. See
    /// [`Graph::validate_shape`].
    pub(super) fn validate_shape(
        &self,
        node_ids: &mut HashSet<NodeId>,
        graph_ids: &mut HashSet<GraphId>,
        depth: usize,
    ) -> ValidationResult<()> {
        if self.origin.is_some_and(|origin| origin.is_nil()) {
            return Err(GraphValidationError::NilOrigin);
        }
        for event in &self.events {
            if !self.body.nodes.contains_key(&event.emitter) {
                return Err(GraphValidationError::ExposedEventMissingEmitter {
                    name: event.name.clone(),
                    emitter: event.emitter,
                });
            }
        }
        // A definition body is never the entry, however it was reached.
        self.body.validate_shape(node_ids, graph_ids, depth, false)
    }

    /// See [`Graph::validate_references`].
    pub(super) fn validate_references(
        &self,
        library: &Library,
        validated: &mut HashSet<GraphId>,
        path: &mut HashSet<GraphId>,
        depth: usize,
    ) -> ValidationResult<()> {
        self.body
            .validate_references(library, validated, path, depth, false)
    }

    /// Validate a *shared* definition reached through a link, once.
    ///
    /// `validated` is the memo — a definition instantiated twice is walked
    /// once — and `path` is what the descent is currently inside, which is what
    /// catches a definition containing itself. Both passes run here, because
    /// nothing else has seen this definition: it lives in the library, not in
    /// the document being validated.
    pub(super) fn validate_shared(
        &self,
        graph_id: GraphId,
        library: &Library,
        validated: &mut HashSet<GraphId>,
        path: &mut HashSet<GraphId>,
        depth: usize,
    ) -> ValidationResult<()> {
        if validated.contains(&graph_id) {
            return Ok(());
        }
        if !path.insert(graph_id) {
            return Err(GraphValidationError::RecursiveGraph {
                name: self.name.clone(),
            });
        }
        let result = self
            .validate_shape(&mut HashSet::new(), &mut HashSet::new(), depth + 1)
            .and_then(|()| self.validate_references(library, validated, path, depth + 1))
            .map_err(|source| GraphValidationError::SharedGraph {
                name: self.name.clone(),
                source: Box::new(source),
            });
        path.remove(&graph_id);
        result?;
        validated.insert(graph_id);
        Ok(())
    }
}

/// One outgoing event re-exported from an interior emitter.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct GraphEvent {
    pub name: String,
    pub emitter: NodeId,
    pub emitter_event_idx: usize,
}

/// Registry selected by a graph-instance node.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum GraphLink {
    Shared(GraphId),
    Local(GraphId),
}

impl GraphLink {
    pub fn id(&self) -> GraphId {
        match self {
            GraphLink::Shared(id) | GraphLink::Local(id) => *id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::identity::FuncId;
    use crate::graph::node::{Node, NodeKind, NodeSearch};

    /// The link of the one graph-instance node `graph` holds.
    fn instance_link(graph: &Graph) -> Option<GraphLink> {
        graph.iter().find_map(|node| match node.kind {
            NodeKind::Graph(link) => Some(link),
            _ => None,
        })
    }

    #[test]
    fn graph_link_preserves_registry_and_identity() {
        let id = GraphId::unique();
        assert_eq!(GraphLink::Local(id).id(), id);
        assert_eq!(GraphLink::Shared(id).id(), id);
        assert_ne!(GraphLink::Local(id), GraphLink::Shared(id));
    }

    #[test]
    fn clone_mapped_remaps_nodes_events_and_nested_graphs() {
        let child_id = GraphId::unique();
        let child_origin = GraphId::unique();
        let mut child = GraphDef::new("child").origin(child_origin);
        let child_node = child.body.add(Node::new(NodeKind::Func(FuncId::unique())));

        let graph_origin = GraphId::unique();
        let mut graph = GraphDef::new("parent").origin(graph_origin);
        let emitter = graph.body.add(Node::new(NodeKind::Func(FuncId::unique())));
        graph.events.push(GraphEvent {
            name: "done".into(),
            emitter,
            emitter_event_idx: 0,
        });
        let instance = Node::graph_instance(&child, GraphLink::Local(child_id));
        graph.body.insert_graph(child_id, child);
        graph.body.add(instance);

        let copy = graph.clone_mapped();
        assert_eq!(copy.origin, None);
        let copied_emitter = copy.events[0].emitter;
        assert_ne!(copied_emitter, emitter);
        assert!(
            copy.body
                .find(copied_emitter, NodeSearch::TopLevel)
                .is_some(),
            "event emitter follows the copied node"
        );
        assert_eq!(
            copy.body.graphs.len(),
            1,
            "the nested def travels with the copy"
        );
        let (copied_child_id, copied_child) = copy.body.graphs.iter().next().unwrap();
        assert_ne!(
            *copied_child_id, child_id,
            "nested graph identities are remapped"
        );
        assert_eq!(copied_child.origin, None);
        assert!(
            copied_child
                .body
                .find(child_node, NodeSearch::TopLevel)
                .is_none(),
            "nested node identities are remapped"
        );
        let linked = instance_link(&copy.body).expect("instance node copied");
        assert_eq!(
            linked,
            GraphLink::Local(*copied_child_id),
            "the instance's Local link follows the remapped def id"
        );

        // The other copy mode is the exact inverse on every axis `clone_mapped`
        // touches — that contrast is why `Graph` isn't `Clone`.
        let verbatim = graph.clone_verbatim();
        assert_eq!(verbatim, graph, "a verbatim copy is field-for-field equal");
        assert_eq!(
            verbatim.origin,
            Some(graph_origin),
            "library lineage survives, where clone_mapped clears it"
        );
        assert_eq!(verbatim.events[0].emitter, emitter);
        let (verbatim_child_id, verbatim_child) = verbatim.body.graphs.iter().next().unwrap();
        assert_eq!(*verbatim_child_id, child_id, "nested def keeps its id");
        assert_ne!(
            *verbatim_child_id, *copied_child_id,
            "the two copy modes disagree on nested identity"
        );
        assert_eq!(verbatim_child.origin, Some(child_origin));
        assert!(
            verbatim_child
                .body
                .find(child_node, NodeSearch::TopLevel)
                .is_some(),
            "nested node ids are preserved, where clone_mapped remaps them"
        );
        assert_eq!(
            instance_link(&verbatim.body),
            Some(GraphLink::Local(child_id)),
            "the instance's Local link still names the original def"
        );
    }
}
