//! [`GraphSet`]: every graph of a document, and the rules that lie across them.
//!
//! # Decisions
//!
//! - **One type owns every graph of a document.** The main graph and the subgraphs travel
//!   together, and each rule that lies across graphs is a method here: the scope of a graph, the
//!   recursion and the bounds, an interface edit, the load's repair. Refused: the main graph and
//!   the subgraphs as two parameters, which no call can pass apart.
//! - **The hierarchy lives here, not in a host.** The subgraphs sit beside the main graph and the
//!   compile flattens them, as MaterialX keeps node graphs in its core and flattens there
//!   ([`NodeGraph::flattenSubgraphs`](https://www.sidefx.com/docs/hdk/class_node_graph.html)).
//! - **Subgraphs do not recurse.** No subgraph places itself, directly or through others, as
//!   Blender ("Recursive node groups are prohibited") and Substance Designer forbid
//!   ([graph instances](https://helpx.adobe.com/substance-3d-designer/substance-model-graphs/graph-instances-in-substance-model-graphs.html)).
//!   A load breaks a loop at the instance that closes it in a walk in id order, so every load
//!   makes the same choice; [`GraphSet::admit_nodes`] is the edit gate's question.
//! - **Every graph's expansion has a bound: [`ProgramShape::MAX_LEVELS`] levels and
//!   [`ProgramShape::MAX_NODES`] program nodes.** Two instances a level over thirty levels are
//!   2³⁰ program nodes, and a compile has no expected failure, so the edit that would pass a
//!   bound is refused and a load detaches the instance that passes it. Held for every graph, not
//!   the main graph alone, so a subgraph within its bounds is one that fits where it is placed
//!   at the top; an add inside a subgraph grows each graph that places it, so the gate asks them
//!   all. Cost: a ninth level is refused.

pub(crate) mod digest;

use std::collections::BTreeMap;
use std::mem;

use hashbrown::{HashMap, HashSet};
use serde::{Deserialize, Serialize};

use crate::data::data_type::DataType;
use crate::graph::Graph;
use crate::graph::error::{
    DetachReason, GraphSetValidationError, GraphValidationError, PlacementRejection,
};
use crate::graph::identity::{GraphId, NodeAddr, NodeId, PortId, SubgraphId};
use crate::graph::instance_path::InstancePath;
use crate::graph::interface::Interface;
use crate::graph::node::{Node, NodeKind};
use crate::graph::ports::PortList;
use crate::graph::program_id::ProgramId;
use crate::graph::reconciled::{GraphSetReconciled, Reconciled, Repair};
use crate::graph::scope::Scope;
use crate::graph::stub::Stub;
use crate::graph::subgraph::Subgraph;
use crate::graph::{Detaching, RefusedWiring};
use crate::library::Library;

/// What a graph expands into: how deep its program paths run and how many program nodes it
/// makes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProgramShape {
    /// The longest program path among its nodes: 1 for a graph of funcs alone, one more for each
    /// instance a node lies inside.
    pub levels: usize,
    pub node_count: usize,
}

impl ProgramShape {
    /// The deepest a graph nests: a [`ProgramId`] holds this many ids.
    pub const MAX_LEVELS: usize = ProgramId::MAX_LEN;
    /// The most program nodes a graph expands into: 2¹⁶, past every graph a person wires by hand
    /// yet small enough that a blowup stays a refused edit, not a compile that runs out of
    /// memory.
    pub const MAX_NODES: usize = 1 << 16;

    /// What one node of kind `kind` adds, given the shape of every subgraph it may place.
    fn of_node(kind: &NodeKind, shapes: &HashMap<SubgraphId, Self>) -> Self {
        match kind {
            NodeKind::Body { .. } => Self {
                levels: 1,
                node_count: 1,
            },
            NodeKind::Instance(subgraph_id) => {
                let inner = shapes
                    .get(subgraph_id)
                    .expect("a placed subgraph's shape is known before its instance's");
                Self {
                    levels: inner.levels + 1,
                    node_count: inner.node_count,
                }
            }
            NodeKind::Boundary(_) => Self::default(),
        }
    }

    /// This shape with `other` beside it in one graph.
    fn beside(self, other: Self) -> Self {
        Self {
            levels: self.levels.max(other.levels),
            node_count: self.node_count.saturating_add(other.node_count),
        }
    }

    /// Why a graph of this shape is one a set may not hold, or `None` if it fits.
    const fn rejection(self, graph: GraphId) -> Option<PlacementRejection> {
        if self.levels > Self::MAX_LEVELS {
            Some(PlacementRejection::TooDeep {
                graph,
                levels: self.levels,
            })
        } else if self.node_count > Self::MAX_NODES {
            Some(PlacementRejection::TooLarge {
                graph,
                node_count: self.node_count,
            })
        } else {
            None
        }
    }
}

/// Every graph of a document: its main graph, and the subgraphs it holds by id.
///
/// The fields are private, so each rule across graphs is a method here and no caller holds the
/// main graph apart from the subgraphs it resolves against.
#[derive(Clone, Default, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct GraphSet {
    main: Graph,
    /// A `BTreeMap`, so a walk in id order is the map's own, and decoded entry by entry, so a
    /// document that repeats an id is refused rather than loaded with the last.
    #[serde(deserialize_with = "crate::graph::serde::deserialize_subgraphs")]
    subgraphs: BTreeMap<SubgraphId, Subgraph>,
}

/// A type an interface names that the library does not register, and each interface port that
/// names it: what a load reports, as the ports are kept as written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnknownType {
    pub data_type: DataType,
    pub ports: Vec<InterfacePort>,
}

/// One port of a subgraph's interface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InterfacePort {
    pub subgraph: SubgraphId,
    /// [`PortList::Inputs`] or [`PortList::Outputs`]: an interface has no events.
    pub list: PortList,
    pub port: PortId,
}

/// What a walk of the subgraphs found: the instances that close a loop, and the subgraphs in an
/// order where each comes after every subgraph it places, the closing instances aside.
#[derive(Debug, Default)]
struct DefWalk {
    closing: Vec<NodeAddr>,
    post_order: Vec<SubgraphId>,
}

impl From<Graph> for GraphSet {
    /// The set of `main` alone.
    fn from(main: Graph) -> Self {
        Self {
            main,
            subgraphs: BTreeMap::new(),
        }
    }
}

impl GraphSet {
    /// The graph `graph` names: the main graph, or a subgraph's.
    ///
    /// # Panics
    /// If `graph` names a subgraph this set does not hold.
    pub fn graph(&self, graph: GraphId) -> &Graph {
        match graph {
            GraphId::Main => &self.main,
            GraphId::Subgraph(id) => {
                &self
                    .subgraph(id)
                    .expect("a named subgraph is in the set")
                    .graph
            }
        }
    }

    /// [`Self::graph`] for writing.
    ///
    /// # Panics
    /// If `graph` names a subgraph this set does not hold.
    pub fn graph_mut(&mut self, graph: GraphId) -> &mut Graph {
        match graph {
            GraphId::Main => &mut self.main,
            GraphId::Subgraph(id) => {
                &mut self
                    .subgraphs
                    .get_mut(&id)
                    .expect("a named subgraph is in the set")
                    .graph
            }
        }
    }

    /// [`Self::graph`] of the main graph.
    pub const fn main(&self) -> &Graph {
        &self.main
    }

    /// [`Self::graph_mut`] of the main graph.
    pub const fn main_mut(&mut self) -> &mut Graph {
        &mut self.main
    }

    pub fn subgraph(&self, id: SubgraphId) -> Option<&Subgraph> {
        self.subgraphs.get(&id)
    }

    /// In id order.
    pub fn subgraphs(&self) -> impl ExactSizeIterator<Item = (SubgraphId, &Subgraph)> {
        self.subgraphs.iter().map(|(id, subgraph)| (*id, subgraph))
    }

    /// Every graph, the main graph first, then the subgraphs' in id order.
    pub fn graphs(&self) -> impl Iterator<Item = (GraphId, &Graph)> {
        [(GraphId::Main, &self.main)].into_iter().chain(
            self.subgraphs
                .iter()
                .map(|(id, subgraph)| (GraphId::Subgraph(*id), &subgraph.graph)),
        )
    }

    /// # Panics
    /// If `id` is nil or already held: an add takes a fresh id.
    pub fn insert_subgraph(&mut self, id: SubgraphId, subgraph: Subgraph) {
        assert!(!id.is_nil(), "cannot add a subgraph with a nil id");
        let prev = self.subgraphs.insert(id, subgraph);
        assert!(
            prev.is_none(),
            "subgraph {id:?} already exists; adds must use a fresh id"
        );
    }

    /// Take out the subgraph under `id`, or `None` if this set holds none.
    ///
    /// # Panics
    /// If a node or a stub in any graph places it: a host removes a subgraph no one uses.
    pub fn remove_subgraph(&mut self, id: SubgraphId) -> Option<Subgraph> {
        assert!(
            self.uses(id).next().is_none(),
            "subgraph {id:?} is removed while placed"
        );
        self.subgraphs.remove(&id)
    }

    /// Put `interface` on subgraph `id` and change no binding: what an interface write does once
    /// it has taken out what the new interface does not hold.
    pub(super) fn set_interface(&mut self, id: SubgraphId, interface: Interface) {
        self.subgraphs
            .get_mut(&id)
            .expect("an interface edit's subgraph is in the set")
            .interface = interface;
    }

    /// Hold `subgraph` under `id`, in place of the definition held there or beside the others,
    /// and hand back the one it replaces. No check runs: what places it may no longer fit its
    /// ports, so the caller reconciles the set after, as a publish into a library file does.
    pub fn put_subgraph(&mut self, id: SubgraphId, subgraph: Subgraph) -> Option<Subgraph> {
        self.subgraphs.insert(id, subgraph)
    }

    /// Put `subgraph` in place of the definition held under `id`, its interface the same, and
    /// hand back the one it replaces: what an update from the library writes once the interface
    /// edit before it made the two interfaces one. Each instance keeps its ports and bindings.
    ///
    /// # Panics
    /// If the set holds no subgraph `id`, or `subgraph`'s interface is not the held one's.
    pub fn replace_subgraph(&mut self, id: SubgraphId, subgraph: Subgraph) -> Subgraph {
        let held = self
            .subgraphs
            .get_mut(&id)
            .expect("a replaced subgraph is in the set");
        assert!(
            held.interface == subgraph.interface,
            "a replacement keeps the interface an interface edit set"
        );
        mem::replace(held, subgraph)
    }

    /// What the nodes of `graph` resolve against.
    ///
    /// # Panics
    /// If `graph` names a subgraph this set does not hold.
    pub fn scope<'a>(&'a self, library: &'a Library, graph: GraphId) -> Scope<'a> {
        let owner = match graph {
            GraphId::Main => None,
            GraphId::Subgraph(id) => {
                Some(self.subgraph(id).expect("a scope's subgraph is in the set"))
            }
        };
        Scope::new(library, self, owner)
    }

    /// The graph `path` ends in: each of its ids an instance in the graph the path before it ends
    /// in. `None` if one is not, as a path an edit since has broken.
    pub fn graph_at(&self, path: InstancePath) -> Option<GraphId> {
        let mut graph = GraphId::Main;
        for &instance in path.ids() {
            let placed = self.graph(graph).find(instance)?.subgraph()?;
            self.subgraph(placed)?;
            graph = GraphId::Subgraph(placed);
        }
        Some(graph)
    }

    /// Every instance whose expansion is `graph`, as the compile expands them, in path order:
    /// the main graph's own path for the main graph, none for a subgraph no node places. A stub
    /// expands nothing, so a path through one is none.
    ///
    /// Walks the expansion from the main graph down, at most [`ProgramShape::MAX_LEVELS`] deep, as the
    /// depth gate keeps the set. For a user's action, not a frame: a fan-out of instances grows
    /// the walk with the product of their counts.
    pub fn instances(&self, graph: GraphId) -> Vec<InstancePath> {
        let mut found = Vec::new();
        let mut open = vec![(InstancePath::MAIN, GraphId::Main)];
        while let Some((path, at)) = open.pop() {
            if at == graph {
                found.push(path);
            }
            for node in self.graph(at).iter() {
                if let Some(placed) = node.subgraph()
                    && self.subgraph(placed).is_some()
                {
                    open.push((path.child(node.node_id), GraphId::Subgraph(placed)));
                }
            }
        }
        found.sort_unstable();
        found
    }

    /// Subgraph `id` and each subgraph it places, however deep, each once and after those it
    /// places: the order a copy of them goes into another set so each instance's subgraph is
    /// held before it. Bounded by the depth gate; a placed subgraph the set lacks is left out.
    pub fn placed_first(&self, id: SubgraphId) -> Vec<SubgraphId> {
        let mut order = Vec::new();
        self.push_placed_first(id, &mut order);
        order
    }

    fn push_placed_first(&self, id: SubgraphId, order: &mut Vec<SubgraphId>) {
        if order.contains(&id) {
            return;
        }
        let Some(subgraph) = self.subgraph(id) else {
            return;
        };
        for placed in subgraph.graph.iter().filter_map(|node| node.subgraph()) {
            self.push_placed_first(placed, order);
        }
        order.push(id);
    }

    /// Every node that places subgraph `id`, a stub's included, in each graph: the main graph's
    /// first, then the subgraphs' in id order.
    pub fn uses(&self, id: SubgraphId) -> impl Iterator<Item = NodeAddr> {
        self.graphs().flat_map(move |(graph, held)| {
            let nodes = held
                .iter()
                .filter(move |node| node.subgraph() == Some(id))
                .map(|node| node.node_id);
            let stubs = held
                .stubs()
                .filter(move |stub| stub.record.node().subgraph() == Some(id))
                .map(Stub::node_id);
            nodes
                .chain(stubs)
                .map(move |node_id| NodeAddr::new(graph, node_id))
        })
    }

    /// Whether `nodes` may be added to `graph`: no instance among them places a subgraph inside
    /// itself, and with them in place every graph keeps within both bounds — `graph`, and each
    /// graph that places it, however deep. The edit gate's question for an add, a paste and an
    /// update.
    ///
    /// # Panics
    /// If an instance among `nodes` places a subgraph this set does not hold, which the gate
    /// refuses before as a node that does not resolve.
    pub fn admit_nodes<'a>(
        &self,
        graph: GraphId,
        nodes: impl IntoIterator<Item = &'a Node>,
    ) -> Result<(), PlacementRejection> {
        let nodes: Vec<&Node> = nodes.into_iter().collect();
        if nodes
            .iter()
            .filter_map(|node| node.subgraph())
            .any(|placed| self.would_recurse(graph, placed))
        {
            return Err(PlacementRejection::Recursive);
        }
        let walk = self.walk();
        debug_assert!(walk.closing.is_empty(), "a set a host keeps holds no loop");
        // What the nodes add, read off the shapes as they are: a subgraph an added instance
        // places does not hold `graph`, or the add would recurse, so the add leaves its shape
        // alone, wherever the walk puts it.
        let before = self.shapes(&walk.post_order);
        let grown = nodes.iter().fold(ProgramShape::default(), |grown, node| {
            grown.beside(ProgramShape::of_node(&node.kind, &before))
        });
        let mut shapes: HashMap<SubgraphId, ProgramShape> =
            HashMap::with_capacity(walk.post_order.len());
        let shape_with = |id: GraphId, held: &Graph, shapes: &HashMap<SubgraphId, ProgramShape>| {
            let shape = shape_of(held, shapes);
            if id == graph {
                shape.beside(grown)
            } else {
                shape
            }
        };
        for &id in &walk.post_order {
            let shape = shape_with(GraphId::Subgraph(id), &self.subgraphs[&id].graph, &shapes);
            shapes.insert(id, shape);
        }
        let main = shape_with(GraphId::Main, &self.main, &shapes);
        let own = match graph {
            GraphId::Main => main,
            GraphId::Subgraph(id) => *shapes
                .get(&id)
                .expect("the graph an add lands in is in the set"),
        };
        if let Some(rejection) = own.rejection(graph) {
            return Err(rejection);
        }
        // The graphs that place `graph` grow with it; the first in id order that passes a bound
        // is the one named.
        let others = [(GraphId::Main, main)].into_iter().chain(
            shapes
                .iter()
                .map(|(id, shape)| (GraphId::Subgraph(*id), *shape)),
        );
        others
            .filter_map(|(id, shape)| shape.rejection(id).map(|rejection| (id, rejection)))
            .min_by_key(|(id, _)| *id)
            .map_or(Ok(()), |(_, rejection)| Err(rejection))
    }

    /// Validate this set as one a host may compile against `library`: each graph holds the load
    /// contract in its own [`Scope`] ([`Graph::validate_with`]), no subgraph places itself, and
    /// every graph's expansion fits its bounds.
    ///
    /// What [`Self::reconcile`] establishes at load and the edit gate keeps, so a failure here
    /// is the host's bug: a compile asserts it.
    pub fn validate_with(&self, library: &Library) -> Result<(), GraphSetValidationError> {
        for (graph, held) in self.graphs() {
            held.validate_with(self.scope(library, graph))
                .map_err(|error| GraphSetValidationError { graph, error })?;
        }
        let walk = self.walk();
        if let Some(closing) = walk.closing.first() {
            return Err(GraphSetValidationError {
                graph: closing.graph,
                error: GraphValidationError::Recursive {
                    node_id: closing.node_id,
                },
            });
        }
        let shapes = self.shapes(&walk.post_order);
        for (graph, held) in self.graphs() {
            let shape = shape_of(held, &shapes);
            let error = if shape.levels > ProgramShape::MAX_LEVELS {
                GraphValidationError::TooDeep {
                    levels: shape.levels,
                }
            } else if shape.node_count > ProgramShape::MAX_NODES {
                GraphValidationError::TooLarge {
                    node_count: shape.node_count,
                }
            } else {
                continue;
            };
            return Err(GraphSetValidationError { graph, error });
        }
        Ok(())
    }

    /// Make a decoded set one `library` can run, and report each change, graph by graph. In each
    /// graph: remove a node under the nil id, a stub under a node's id and the wiring that names
    /// a missing node; attach each stub whose node resolves again; keep one boundary node a side
    /// in a subgraph's; detach into a stub each node its scope does not resolve. Across them,
    /// detach each instance that closes a loop, nests past [`ProgramShape::MAX_LEVELS`] or
    /// expands past [`ProgramShape::MAX_NODES`], taken in id order. Then in each graph remove each
    /// binding and subscription its scope refuses, and each wire that closes a data cycle.
    /// Afterwards [`Self::validate_with`] holds.
    ///
    /// Each step reads a graph under its scope, then writes it: the scope borrows the whole set.
    /// A stub attached and detached again as it was is no change, and its repairs are left out,
    /// so a set the load leaves as it found reports nothing.
    pub fn reconcile(&mut self, library: &Library) -> GraphSetReconciled {
        let ids: Vec<GraphId> = self.graphs().map(|(graph, _)| graph).collect();
        let mut reports: BTreeMap<GraphId, Reconciled> = BTreeMap::new();
        let mut reattached: Vec<(GraphId, Stub)> = Vec::new();

        for &graph in &ids {
            let report = reports.entry(graph).or_default();
            self.graph_mut(graph).repair_structure(report);
            let resolving = self
                .graph(graph)
                .resolving_stubs(self.scope(library, graph));
            reattached.extend(resolving.iter().map(|&node_id| {
                let stub = self
                    .graph(graph)
                    .stub(node_id)
                    .expect("a resolving stub is held");
                (graph, stub.clone())
            }));
            self.graph_mut(graph).reattach(&resolving, report);
            if matches!(graph, GraphId::Subgraph(_)) {
                self.graph_mut(graph).add_missing_boundaries(report);
            }
            let unresolved = self.graph(graph).unresolved(self.scope(library, graph));
            self.graph_mut(graph).detach_into_stubs(&unresolved, report);
        }

        let walk = self.walk();
        let mut recursive: BTreeMap<GraphId, Vec<Detaching>> = BTreeMap::new();
        for closing in walk.closing {
            recursive.entry(closing.graph).or_default().push(Detaching {
                node_id: closing.node_id,
                reason: DetachReason::Recursive,
            });
        }
        for (graph, mut refused) in recursive {
            refused.sort_unstable_by_key(|detaching| detaching.node_id);
            let report = reports.entry(graph).or_default();
            self.graph_mut(graph).detach_into_stubs(&refused, report);
        }

        let mut shapes = HashMap::with_capacity(walk.post_order.len());
        for &id in &walk.post_order {
            let graph = GraphId::Subgraph(id);
            let refused = over_bounds(self.graph(graph), &shapes);
            let report = reports.entry(graph).or_default();
            self.graph_mut(graph).detach_into_stubs(&refused, report);
            shapes.insert(id, shape_of(self.graph(graph), &shapes));
        }
        let refused = over_bounds(&self.main, &shapes);
        let report = reports.entry(GraphId::Main).or_default();
        self.main.detach_into_stubs(&refused, report);

        for &graph in &ids {
            let refused: Vec<RefusedWiring> =
                self.graph(graph).refused_wiring(self.scope(library, graph));
            let report = reports.entry(graph).or_default();
            self.graph_mut(graph).remove_refused(&refused, report);
        }

        for (graph, before) in reattached {
            let node_id = before.node_id();
            if self.graph(graph).stub(node_id) == Some(&before) {
                reports
                    .get_mut(&graph)
                    .expect("a graph with a reattached stub has a report")
                    .repairs
                    .retain(|repair| !concerns_stub(repair, node_id));
            }
        }

        let mut report = GraphSetReconciled::default();
        for (graph, reconciled) in reports {
            match graph {
                GraphId::Main => report.main = reconciled,
                GraphId::Subgraph(id) if !reconciled.is_empty() => {
                    report.subgraphs.insert(id, reconciled);
                }
                GraphId::Subgraph(_) => {}
            }
        }
        debug_assert!(
            self.validate_with(library).is_ok(),
            "a reconciled set holds the load contract: {:?}",
            self.validate_with(library)
        );
        report
    }

    /// Each type an interface names that `library` does not register, in the order the
    /// interfaces first name it, with the ports that name it: subgraphs in id order, the inputs
    /// of each before its outputs.
    pub fn unknown_types(&self, library: &Library) -> Vec<UnknownType> {
        let mut unknown: Vec<UnknownType> = Vec::new();
        for (subgraph, held) in self.subgraphs() {
            let ports = held
                .interface
                .inputs()
                .iter()
                .map(|input| (PortList::Inputs, input.id, input.data_type));
            let ports = ports.chain(
                held.interface
                    .outputs()
                    .iter()
                    .map(|output| (PortList::Outputs, output.id, output.data_type)),
            );
            for (list, port, data_type) in ports {
                if library.knows(data_type) {
                    continue;
                }
                let port = InterfacePort {
                    subgraph,
                    list,
                    port,
                };
                match unknown
                    .iter_mut()
                    .find(|entry| entry.data_type == data_type)
                {
                    Some(entry) => entry.ports.push(port),
                    None => unknown.push(UnknownType {
                        data_type,
                        ports: vec![port],
                    }),
                }
            }
        }
        unknown
    }

    /// Whether placing an instance of `placed` in `graph` would put a subgraph inside itself:
    /// `graph` is the graph of `placed`, or of a subgraph `placed` holds, however deep.
    fn would_recurse(&self, graph: GraphId, placed: SubgraphId) -> bool {
        let GraphId::Subgraph(owner) = graph else {
            return false;
        };
        let mut stack = vec![placed];
        let mut seen: HashSet<SubgraphId> = HashSet::from_iter([placed]);
        while let Some(id) = stack.pop() {
            if id == owner {
                return true;
            }
            let Some(subgraph) = self.subgraph(id) else {
                continue;
            };
            for node in subgraph.graph.iter() {
                if let Some(inner) = node.subgraph()
                    && seen.insert(inner)
                {
                    stack.push(inner);
                }
            }
        }
        false
    }

    /// A depth-first walk of what each subgraph places, from every subgraph in id order and
    /// through each one's instances in node-id order: each instance that returns to a subgraph on
    /// the current path closes a loop. Instances of a subgraph the set lacks are passed over.
    /// Removing the closing instances leaves no loop, and the id order makes the choice the same
    /// on every load.
    fn walk(&self) -> DefWalk {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        enum Mark {
            OnPath,
            Done,
        }
        #[derive(Debug)]
        enum Visit {
            Enter(SubgraphId),
            Instance {
                owner: SubgraphId,
                node_id: NodeId,
                placed: SubgraphId,
            },
            Exit(SubgraphId),
        }

        let mut marks: HashMap<SubgraphId, Mark> = HashMap::with_capacity(self.subgraphs.len());
        let mut stack = Vec::new();
        let mut walk = DefWalk {
            closing: Vec::new(),
            post_order: Vec::with_capacity(self.subgraphs.len()),
        };
        for &root in self.subgraphs.keys() {
            if marks.contains_key(&root) {
                continue;
            }
            stack.push(Visit::Enter(root));
            while let Some(visit) = stack.pop() {
                match visit {
                    Visit::Enter(owner) => {
                        marks.insert(owner, Mark::OnPath);
                        stack.push(Visit::Exit(owner));
                        let first = stack.len();
                        stack.extend(self.subgraphs[&owner].graph.iter().filter_map(|node| {
                            node.subgraph().map(|placed| Visit::Instance {
                                owner,
                                node_id: node.node_id,
                                placed,
                            })
                        }));
                        let instances = &mut stack[first..];
                        instances.sort_unstable_by_key(|visit| match visit {
                            Visit::Instance { node_id, .. } => *node_id,
                            Visit::Enter(_) | Visit::Exit(_) => unreachable!(),
                        });
                        instances.reverse();
                    }
                    Visit::Instance {
                        owner,
                        node_id,
                        placed,
                    } => match marks.get(&placed) {
                        Some(Mark::OnPath) => walk
                            .closing
                            .push(NodeAddr::new(GraphId::Subgraph(owner), node_id)),
                        Some(Mark::Done) => {}
                        None if self.subgraphs.contains_key(&placed) => {
                            stack.push(Visit::Enter(placed));
                        }
                        None => {}
                    },
                    Visit::Exit(owner) => {
                        marks.insert(owner, Mark::Done);
                        walk.post_order.push(owner);
                    }
                }
            }
        }
        walk
    }

    /// The shape of each subgraph, taken in `post_order`, so each is known before an instance of
    /// it is counted.
    fn shapes(&self, post_order: &[SubgraphId]) -> HashMap<SubgraphId, ProgramShape> {
        let mut shapes = HashMap::with_capacity(post_order.len());
        for id in post_order {
            shapes.insert(*id, shape_of(&self.subgraphs[id].graph, &shapes));
        }
        shapes
    }
}

/// Whether `repair` is one of the changes an attach and a detach of the stub `node_id` report.
fn concerns_stub(repair: &Repair, node_id: NodeId) -> bool {
    match repair {
        Repair::Reattached { node_id: stub }
        | Repair::StubBindingDropped { node_id: stub, .. }
        | Repair::StubSubscriptionDropped { node_id: stub, .. }
        | Repair::Stubbed { node_id: stub, .. } => *stub == node_id,
        _ => false,
    }
}

/// What `graph` expands into, given the shape of every subgraph it places.
fn shape_of(graph: &Graph, shapes: &HashMap<SubgraphId, ProgramShape>) -> ProgramShape {
    graph.iter().fold(ProgramShape::default(), |shape, node| {
        shape.beside(ProgramShape::of_node(&node.kind, shapes))
    })
}

/// The nodes of `graph` that pass its bounds, in node-id order: each one whose own levels pass
/// [`ProgramShape::MAX_LEVELS`], and each one that would carry the count of the nodes before it
/// past [`ProgramShape::MAX_NODES`].
fn over_bounds(graph: &Graph, shapes: &HashMap<SubgraphId, ProgramShape>) -> Vec<Detaching> {
    let mut nodes: Vec<_> = graph
        .iter()
        .map(|node| (node.node_id, ProgramShape::of_node(&node.kind, shapes)))
        .collect();
    nodes.sort_unstable_by_key(|(node_id, _)| *node_id);
    let mut kept = ProgramShape::default();
    let mut refused = Vec::new();
    for (node_id, shape) in nodes {
        if shape.levels > ProgramShape::MAX_LEVELS {
            refused.push(Detaching {
                node_id,
                reason: DetachReason::TooDeep,
            });
        } else if kept.beside(shape).node_count > ProgramShape::MAX_NODES {
            refused.push(Detaching {
                node_id,
                reason: DetachReason::TooLarge,
            });
        } else {
            kept = kept.beside(shape);
        }
    }
    refused
}

#[cfg(test)]
mod tests;
