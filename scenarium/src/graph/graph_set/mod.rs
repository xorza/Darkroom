//! [`GraphSet`]: every graph of a document, and the rules that lie across them.
//!
//! # Decisions
//!
//! - **One type owns every graph of a document.** The main graph and the compounds travel
//!   together, and each rule that lies across graphs is a method here: the scope of a graph, the
//!   recursion and the bounds, an interface edit, the load's repair. Refused: the main graph and
//!   the compounds as two parameters, which no call can pass apart.
//! - **The hierarchy lives here, not in a host.** The compounds sit beside the main graph and the
//!   compile flattens them, as MaterialX keeps node graphs in its core and flattens there
//!   ([`NodeGraph::flattenCompounds`](https://www.sidefx.com/docs/hdk/class_node_graph.html)).
//! - **Compounds do not recurse.** No compound places itself, directly or through others, as
//!   Blender ("Recursive node groups are prohibited") and Substance Designer forbid
//!   ([graph instances](https://helpx.adobe.com/substance-3d-designer/substance-model-graphs/graph-instances-in-substance-model-graphs.html)).
//!   A load breaks a loop at the instance that closes it in a walk in id order, so every load
//!   makes the same choice; [`GraphSet::admit_nodes`] is the edit gate's question.
//! - **Every graph's expansion has a bound: [`ProgramShape::MAX_LEVELS`] levels and
//!   [`ProgramShape::MAX_NODES`] program nodes.** Two instances a level over thirty levels are
//!   2³⁰ program nodes, and a compile has no expected failure, so the edit that would pass a
//!   bound is refused and a load detaches the instance that passes it. Held for every graph, not
//!   the main graph alone, so a compound within its bounds is one that fits where it is placed
//!   at the top; an add inside a compound grows each graph that places it, so the gate asks them
//!   all. Cost: a ninth level is refused.

pub(crate) mod digest;

use std::collections::BTreeMap;
use std::mem;

use hashbrown::{HashMap, HashSet};
use serde::{Deserialize, Serialize};

use crate::data::data_type::DataType;
use crate::graph::Graph;
use crate::graph::compound::Compound;
use crate::graph::error::{
    DetachReason, GraphSetValidationError, GraphValidationError, PlacementRejection,
};
use crate::graph::ghost_node::GhostNode;
use crate::graph::ghost_wiring::GhostSource;
use crate::graph::identity::{CompoundId, GraphId, NodeAddr, NodeId, PortId};
use crate::graph::instance_path::InstancePath;
use crate::graph::interface::Interface;
use crate::graph::manifest::Manifest;
use crate::graph::node::{Node, NodeKind};
use crate::graph::node_decl::NodeDecl;
use crate::graph::ports::PortList;
use crate::graph::program_id::ProgramId;
use crate::graph::reconciled::{GraphSetReconciled, Reconciled, Repair};
use crate::graph::saved_decl::SavedDecl;
use crate::graph::scope::Scope;
use crate::graph::type_names::{TypeNames, TypeRef};
use crate::graph::{Before, Detaching, RefusedWiring};
use crate::library::{Library, UNKNOWN_TYPE};

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

    /// What one node of kind `kind` adds, given the shape of every compound it may place.
    fn of_node(kind: &NodeKind, shapes: &HashMap<CompoundId, Self>) -> Self {
        match kind {
            NodeKind::Body { .. } => Self {
                levels: 1,
                node_count: 1,
            },
            NodeKind::Instance { compound, .. } => {
                let inner = shapes
                    .get(compound)
                    .expect("a placed compound's shape is known before its instance's");
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

/// Every graph of a document: its main graph, and the compounds it holds by id.
///
/// The fields are private, so each rule across graphs is a method here and no caller holds the
/// main graph apart from the compounds it resolves against.
#[derive(Clone, Default, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct GraphSet {
    main: Graph,
    /// A `BTreeMap`, so a walk in id order is the map's own, and decoded entry by entry, so a
    /// document that repeats an id is refused rather than loaded with the last.
    #[serde(deserialize_with = "crate::graph::serde::deserialize_compounds")]
    compounds: BTreeMap<CompoundId, Compound>,
    /// The names files gave the types the library lacks, as [`TypeNames`] says: learned at a
    /// load and as a definition enters, written by the next save's manifest.
    #[serde(skip)]
    type_names: TypeNames,
}

/// A type an interface names that the library does not register, and each interface port that
/// names it: what a load reports, as the ports are kept as written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnknownType {
    pub data_type: DataType,
    pub ports: Vec<InterfacePort>,
}

/// One port of a compound's interface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InterfacePort {
    pub compound: CompoundId,
    /// [`PortList::Inputs`] or [`PortList::Outputs`]: an event carries no type to be unknown.
    pub list: PortList,
    pub port: PortId,
}

/// What a walk of the compounds found: the instances that close a loop, and the compounds in an
/// order where each comes after every compound it places, the closing instances aside.
#[derive(Debug, Default)]
struct DefWalk {
    closing: Vec<NodeAddr>,
    post_order: Vec<CompoundId>,
}

impl From<Graph> for GraphSet {
    /// The set of `main` alone.
    fn from(main: Graph) -> Self {
        Self {
            main,
            compounds: BTreeMap::new(),
            type_names: TypeNames::default(),
        }
    }
}

impl GraphSet {
    /// The graph `graph` names: the main graph, or a compound's.
    ///
    /// # Panics
    /// If `graph` names a compound this set does not hold.
    pub fn graph(&self, graph: GraphId) -> &Graph {
        match graph {
            GraphId::Main => &self.main,
            GraphId::Compound(id) => {
                &self
                    .compound(id)
                    .expect("a named compound is in the set")
                    .graph
            }
        }
    }

    /// [`Self::graph`] for writing.
    ///
    /// # Panics
    /// If `graph` names a compound this set does not hold.
    pub fn graph_mut(&mut self, graph: GraphId) -> &mut Graph {
        match graph {
            GraphId::Main => &mut self.main,
            GraphId::Compound(id) => {
                &mut self
                    .compounds
                    .get_mut(&id)
                    .expect("a named compound is in the set")
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

    pub fn compound(&self, id: CompoundId) -> Option<&Compound> {
        self.compounds.get(&id)
    }

    /// In id order.
    pub fn compounds(&self) -> impl ExactSizeIterator<Item = (CompoundId, &Compound)> {
        self.compounds.iter().map(|(id, compound)| (*id, compound))
    }

    /// Every graph, the main graph first, then the compounds' in id order.
    pub fn graphs(&self) -> impl Iterator<Item = (GraphId, &Graph)> {
        [(GraphId::Main, &self.main)].into_iter().chain(
            self.compounds
                .iter()
                .map(|(id, compound)| (GraphId::Compound(*id), &compound.graph)),
        )
    }

    /// # Panics
    /// If `id` is nil or already held: an add takes a fresh id.
    pub fn insert_compound(&mut self, id: CompoundId, compound: Compound) {
        assert!(!id.is_nil(), "cannot add a compound with a nil id");
        let prev = self.compounds.insert(id, compound);
        assert!(
            prev.is_none(),
            "compound {id:?} already exists; adds must use a fresh id"
        );
    }

    /// Take out the compound under `id`, or `None` if this set holds none.
    ///
    /// # Panics
    /// If a node or a ghost node in any graph places it: a host removes a compound no one uses.
    pub fn remove_compound(&mut self, id: CompoundId) -> Option<Compound> {
        assert!(
            self.uses(id).next().is_none(),
            "compound {id:?} is removed while placed"
        );
        self.compounds.remove(&id)
    }

    /// Put `interface` on compound `id` and change no binding: what an interface write does once
    /// it has taken out what the new interface does not hold.
    pub(super) fn set_interface(&mut self, id: CompoundId, interface: Interface) {
        self.compounds
            .get_mut(&id)
            .expect("an interface edit's compound is in the set")
            .interface = interface;
    }

    /// Name compound `id` `name`, and hand back the name it replaces. A name holds no rule
    /// across graphs, so nothing else changes.
    ///
    /// # Panics
    /// If the set holds no compound `id`.
    pub fn rename_compound(&mut self, id: CompoundId, name: String) -> String {
        let held = self
            .compounds
            .get_mut(&id)
            .expect("a renamed compound is in the set");
        mem::replace(&mut held.name, name)
    }

    /// Hold `compound` under `id`, in place of the definition held there or beside the others,
    /// and hand back the one it replaces. No check runs: what places it may no longer fit its
    /// ports, so the caller reconciles the set after, as a publish into a library file does.
    pub fn put_compound(&mut self, id: CompoundId, compound: Compound) -> Option<Compound> {
        self.compounds.insert(id, compound)
    }

    /// Put `compound` in place of the definition held under `id`, its interface the same, and
    /// hand back the one it replaces: what an update from the library writes once the interface
    /// edit before it made the two interfaces one. Each instance keeps its ports and bindings.
    ///
    /// # Panics
    /// If the set holds no compound `id`, or `compound`'s interface is not the held one's.
    pub fn replace_compound(&mut self, id: CompoundId, compound: Compound) -> Compound {
        let held = self
            .compounds
            .get_mut(&id)
            .expect("a replaced compound is in the set");
        assert!(
            held.interface == compound.interface,
            "a replacement keeps the interface an interface edit set"
        );
        mem::replace(held, compound)
    }

    /// What the nodes of `graph` resolve against.
    ///
    /// # Panics
    /// If `graph` names a compound this set does not hold.
    pub fn scope<'a>(&'a self, library: &'a Library, graph: GraphId) -> Scope<'a> {
        let owner = match graph {
            GraphId::Main => None,
            GraphId::Compound(id) => {
                Some(self.compound(id).expect("a scope's compound is in the set"))
            }
        };
        Scope::new(library, self, owner)
    }

    /// The graph `path` ends in: each of its ids an instance in the graph the path before it ends
    /// in. `None` if one is not, as a path an edit since has broken.
    pub fn graph_at(&self, path: InstancePath) -> Option<GraphId> {
        let mut graph = GraphId::Main;
        for &instance in path.ids() {
            let placed = self.graph(graph).find(instance)?.compound()?;
            self.compound(placed)?;
            graph = GraphId::Compound(placed);
        }
        Some(graph)
    }

    /// Every instance whose expansion is `graph`, as the compile expands them, in path order:
    /// the main graph's own path for the main graph, none for a compound no node places. A ghost node
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
                if let Some(placed) = node.compound()
                    && self.compound(placed).is_some()
                {
                    open.push((path.child(node.node_id), GraphId::Compound(placed)));
                }
            }
        }
        found.sort_unstable();
        found
    }

    /// Compound `id` and each compound it places, however deep, each once and after those it
    /// places: the order a copy of them goes into another set so each instance's compound is
    /// held before it. Bounded by the depth gate; a placed compound the set lacks is left out.
    pub fn placed_first(&self, id: CompoundId) -> Vec<CompoundId> {
        let mut order = Vec::new();
        self.push_placed_first(id, &mut order);
        order
    }

    fn push_placed_first(&self, id: CompoundId, order: &mut Vec<CompoundId>) {
        if order.contains(&id) {
            return;
        }
        let Some(compound) = self.compound(id) else {
            return;
        };
        for placed in compound.graph.iter().filter_map(|node| node.compound()) {
            self.push_placed_first(placed, order);
        }
        order.push(id);
    }

    /// Every node that places compound `id`, each ghost node included, in each graph: the main graph's
    /// first, then the compounds' in id order.
    pub fn uses(&self, id: CompoundId) -> impl Iterator<Item = NodeAddr> {
        self.graphs().flat_map(move |(graph, held)| {
            let nodes = held
                .iter()
                .filter(move |node| node.compound() == Some(id))
                .map(|node| node.node_id);
            let ghosts = held
                .ghost_nodes()
                .filter(move |(_, ghost)| ghost.node.compound() == Some(id))
                .map(|(node_id, _)| node_id);
            nodes
                .chain(ghosts)
                .map(move |node_id| NodeAddr::new(graph, node_id))
        })
    }

    /// Whether `nodes` may be added to `graph`: no instance among them places a compound inside
    /// itself, and with them in place every graph keeps within both bounds — `graph`, and each
    /// graph that places it, however deep. The edit gate's question for an add, a paste and an
    /// update.
    ///
    /// # Panics
    /// If an instance among `nodes` places a compound this set does not hold, which the gate
    /// refuses before as a node that does not resolve.
    pub fn admit_nodes<'a>(
        &self,
        graph: GraphId,
        nodes: impl IntoIterator<Item = &'a Node>,
    ) -> Result<(), PlacementRejection> {
        let nodes: Vec<&Node> = nodes.into_iter().collect();
        if nodes
            .iter()
            .filter_map(|node| node.compound())
            .any(|placed| self.would_recurse(graph, placed))
        {
            return Err(PlacementRejection::Recursive);
        }
        let walk = self.walk();
        debug_assert!(walk.closing.is_empty(), "a set a host keeps holds no loop");
        // What the nodes add, read off the shapes as they are: a compound an added instance
        // places does not hold `graph`, or the add would recurse, so the add leaves its shape
        // alone, wherever the walk puts it.
        let before = self.shapes(&walk.post_order);
        let grown = nodes.iter().fold(ProgramShape::default(), |grown, node| {
            grown.beside(ProgramShape::of_node(&node.kind, &before))
        });
        let mut shapes: HashMap<CompoundId, ProgramShape> =
            HashMap::with_capacity(walk.post_order.len());
        let shape_with = |id: GraphId, held: &Graph, shapes: &HashMap<CompoundId, ProgramShape>| {
            let shape = shape_of(held, shapes);
            if id == graph {
                shape.beside(grown)
            } else {
                shape
            }
        };
        for &id in &walk.post_order {
            let shape = shape_with(GraphId::Compound(id), &self.compounds[&id].graph, &shapes);
            shapes.insert(id, shape);
        }
        let main = shape_with(GraphId::Main, &self.main, &shapes);
        let own = match graph {
            GraphId::Main => main,
            GraphId::Compound(id) => *shapes
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
                .map(|(id, shape)| (GraphId::Compound(*id), *shape)),
        );
        others
            .filter_map(|(id, shape)| shape.rejection(id).map(|rejection| (id, rejection)))
            .min_by_key(|(id, _)| *id)
            .map_or(Ok(()), |(_, rejection)| Err(rejection))
    }

    /// Validate this set as one a host may compile against `library`: each graph holds the load
    /// contract in its own [`Scope`] ([`Graph::validate_with`]), no compound places itself, and
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

    /// The saved declaration of each func a node of this set names, a built-in's included, as
    /// `library` gives it: what a save writes beside the set, and the next load reads to tell
    /// what changed. A ghost node holds its own.
    pub fn manifest(&self, library: &Library) -> Manifest {
        let funcs: Manifest = self
            .graphs()
            .flat_map(|(graph, held)| {
                let scope = self.scope(library, graph);
                held.iter().filter_map(move |node| {
                    let func = scope.resolve(&node).ok()?.func()?;
                    Some((func.id(), NodeDecl::Func(func).saved()))
                })
            })
            .collect();
        let saved = funcs.iter().flat_map(|(_, decl)| decl_types(decl));
        let types = saved
            .chain(self.named_types())
            .filter_map(TypeRef::of)
            .filter_map(|type_ref| {
                let name = library
                    .registered_name(type_ref)
                    .or_else(|| self.type_names.get(type_ref))?;
                Some((type_ref, name))
            })
            .collect();
        funcs.with_types(types)
    }

    /// Every type the set itself names, apart from what its nodes' funcs declare: each
    /// interface's ports, each ghost node's saved declaration and each ghost's saved ports.
    fn named_types(&self) -> impl Iterator<Item = DataType> {
        let interfaces = self.compounds.values().flat_map(|compound| {
            let interface = &compound.interface;
            let inputs = interface.inputs().iter().map(|input| input.data_type);
            inputs.chain(interface.outputs().iter().map(|output| output.data_type))
        });
        let ghosts = self.graphs().flat_map(|(_, graph)| {
            let nodes = graph
                .ghost_nodes()
                .filter_map(|(_, ghost)| ghost.saved.as_ref())
                .flat_map(decl_types);
            let ports = graph.ghost_wiring().bindings().flat_map(|(_, ghost)| {
                let source = match &ghost.source {
                    GhostSource::Wire { saved, .. } => saved.as_ref(),
                    GhostSource::Const(_) => None,
                };
                ghost
                    .input
                    .iter()
                    .chain(source)
                    .map(|saved| saved.data_type)
            });
            nodes.chain(ports)
        });
        interfaces.chain(ghosts)
    }

    /// The name `data_type` shows: the library's, else the one a file gave it
    /// ([`TypeNames`]), else "Unknown Type".
    pub fn type_name<'a>(&'a self, library: &'a Library, data_type: DataType) -> &'a str {
        match TypeRef::of(data_type) {
            None => library.type_name(data_type),
            Some(type_ref) => library
                .registered_name(type_ref)
                .or_else(|| self.type_names.get(type_ref))
                .unwrap_or(UNKNOWN_TYPE),
        }
    }

    /// Learn each name `names` gives a type `library` lacks, as a definition from a file enters
    /// the set.
    pub fn learn_type_names(&mut self, names: &TypeNames, library: &Library) {
        self.type_names.learn(names, library);
    }

    /// Make a decoded set one `library` can run, and report each change, graph by graph. In each
    /// graph: remove a node under the nil id, a ghost node under a node's id and the wiring that
    /// names a missing node; move each node and ghost node of a func the library redirects to the
    /// func its redirects end at; attach each ghost node that resolves again; keep one boundary node
    /// a side in a compound's; make a ghost node of each node its scope does not resolve. Across
    /// them, make a ghost node of each instance that closes a loop, nests past
    /// [`ProgramShape::MAX_LEVELS`] or expands past [`ProgramShape::MAX_NODES`], taken in id
    /// order. Then in each graph make a ghost of each binding and subscription its scope refuses
    /// and of each wire that closes a data cycle, and make live each ghost it can hold again.
    /// Afterwards [`Self::validate_with`] holds.
    ///
    /// `manifest` is what the set's file says each func declared at the save: a ghost node takes
    /// its saved declaration from it and a ghost the saved port its node lost; a ghost constant
    /// that was its port's default goes; and an input a func gained since takes its default.
    /// With an empty one a load knows none of that and does none of it.
    ///
    /// Each step reads a graph under its scope, then writes it: the scope borrows the whole set.
    /// A ghost node attached and detached again as it was is no change, and its repairs are left
    /// out, so a set the load leaves as it found reports nothing.
    pub fn reconcile(&mut self, library: &Library, manifest: &Manifest) -> GraphSetReconciled {
        self.learn_type_names(manifest.types(), library);
        let ids: Vec<GraphId> = self.graphs().map(|(graph, _)| graph).collect();
        let mut reports: BTreeMap<GraphId, Reconciled> = BTreeMap::new();
        let mut reattached: Vec<(GraphId, NodeId, GhostNode)> = Vec::new();

        for &graph in &ids {
            let report = reports.entry(graph).or_default();
            self.graph_mut(graph).repair_structure(report);
            self.graph_mut(graph)
                .redirect_all(library, manifest, report);
            let resolving = self
                .graph(graph)
                .resolving_ghost_nodes(self.scope(library, graph));
            let ghosts = self.graph_mut(graph).reattach(&resolving, report);
            reattached.extend(
                resolving
                    .iter()
                    .zip(ghosts)
                    .map(|(&node_id, ghost)| (graph, node_id, ghost)),
            );
            if matches!(graph, GraphId::Compound(_)) {
                self.graph_mut(graph).add_missing_boundaries(report);
            }
            let unresolved = self
                .graph(graph)
                .unresolved(self.scope(library, graph), manifest);
            self.graph_mut(graph).detach_into_ghosts(unresolved, report);
        }

        let walk = self.walk();
        let mut recursive: BTreeMap<GraphId, Vec<NodeId>> = BTreeMap::new();
        for closing in walk.closing {
            recursive
                .entry(closing.graph)
                .or_default()
                .push(closing.node_id);
        }
        for (graph, mut closing) in recursive {
            closing.sort_unstable();
            let refused = closing
                .into_iter()
                .map(|node_id| Refusal {
                    node_id,
                    reason: DetachReason::Recursive,
                })
                .collect();
            self.detach(library, manifest, graph, refused, &mut reports);
        }

        let mut shapes = HashMap::with_capacity(walk.post_order.len());
        for &id in &walk.post_order {
            let graph = GraphId::Compound(id);
            let refused = over_bounds(self.graph(graph), &shapes);
            self.detach(library, manifest, graph, refused, &mut reports);
            shapes.insert(id, shape_of(self.graph(graph), &shapes));
        }
        let refused = over_bounds(&self.main, &shapes);
        self.detach(library, manifest, GraphId::Main, refused, &mut reports);

        for &graph in &ids {
            let refused: Vec<RefusedWiring> =
                self.graph(graph).refused_wiring(self.scope(library, graph));
            let report = reports.entry(graph).or_default();
            self.graph_mut(graph).ghost_refused(&refused, report);
            self.graph_mut(graph).drop_impossible_ghosts(report);
            let admitted = self
                .graph(graph)
                .admitted_ghosts(self.scope(library, graph));
            self.graph_mut(graph).restore(&admitted, report);

            // What each ghost node this load attached declared.
            let returned: HashMap<NodeId, SavedDecl> = reattached
                .iter()
                .filter(|(of, ..)| *of == graph)
                .filter_map(|(_, node_id, ghost)| Some((*node_id, ghost.saved.clone()?)))
                .collect();
            let before = Before {
                manifest,
                returned: &returned,
            };
            let notes = self
                .graph(graph)
                .ghost_notes(self.scope(library, graph), before);
            self.graph_mut(graph).note_ghosts(&notes, report);
            let seeds =
                self.graph(graph)
                    .gained_defaults(self.scope(library, graph), before, &notes);
            self.graph_mut(graph).seed_defaults(seeds, report);
        }

        for (graph, node_id, before) in reattached {
            if self.graph(graph).ghost_node(node_id) == Some(&before) {
                reports
                    .get_mut(&graph)
                    .expect("a graph with a reattached ghost node has a report")
                    .repairs
                    .retain(|repair| !reattaches(repair, node_id));
            }
        }

        let mut report = GraphSetReconciled::default();
        for (graph, reconciled) in reports {
            match graph {
                GraphId::Main => report.main = reconciled,
                GraphId::Compound(id) if !reconciled.is_empty() => {
                    report.compounds.insert(id, reconciled);
                }
                GraphId::Compound(_) => {}
            }
        }
        debug_assert!(
            self.validate_with(library).is_ok(),
            "a reconciled set holds the load contract: {:?}",
            self.validate_with(library)
        );
        report
    }

    /// Make a ghost node of each node of `refused` in `graph`, each with what it declares.
    fn detach(
        &mut self,
        library: &Library,
        manifest: &Manifest,
        graph: GraphId,
        refused: Vec<Refusal>,
        reports: &mut BTreeMap<GraphId, Reconciled>,
    ) {
        let scope = self.scope(library, graph);
        let detaching: Vec<Detaching> = refused
            .into_iter()
            .map(|Refusal { node_id, reason }| {
                self.graph(graph)
                    .detaching(scope, manifest, node_id, reason)
            })
            .collect();
        self.graph_mut(graph)
            .detach_into_ghosts(detaching, reports.entry(graph).or_default());
    }

    /// Each type an interface names that `library` does not register, in the order the
    /// interfaces first name it, with the ports that name it: compounds in id order, the inputs
    /// of each before its outputs.
    pub fn unknown_types(&self, library: &Library) -> Vec<UnknownType> {
        let mut unknown: Vec<UnknownType> = Vec::new();
        for (compound, held) in self.compounds() {
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
                    compound,
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

    /// Whether placing an instance of `placed` in `graph` would put a compound inside itself:
    /// `graph` is the graph of `placed`, or of a compound `placed` holds, however deep.
    fn would_recurse(&self, graph: GraphId, placed: CompoundId) -> bool {
        let GraphId::Compound(owner) = graph else {
            return false;
        };
        let mut stack = vec![placed];
        let mut seen: HashSet<CompoundId> = HashSet::from_iter([placed]);
        while let Some(id) = stack.pop() {
            if id == owner {
                return true;
            }
            let Some(compound) = self.compound(id) else {
                continue;
            };
            for node in compound.graph.iter() {
                if let Some(inner) = node.compound()
                    && seen.insert(inner)
                {
                    stack.push(inner);
                }
            }
        }
        false
    }

    /// A depth-first walk of what each compound places, from every compound in id order and
    /// through each one's instances in node-id order: each instance that returns to a compound on
    /// the current path closes a loop. Instances of a compound the set lacks are passed over.
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
            Enter(CompoundId),
            Instance {
                owner: CompoundId,
                node_id: NodeId,
                placed: CompoundId,
            },
            Exit(CompoundId),
        }

        let mut marks: HashMap<CompoundId, Mark> = HashMap::with_capacity(self.compounds.len());
        let mut stack = Vec::new();
        let mut walk = DefWalk {
            closing: Vec::new(),
            post_order: Vec::with_capacity(self.compounds.len()),
        };
        for &root in self.compounds.keys() {
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
                        stack.extend(self.compounds[&owner].graph.iter().filter_map(|node| {
                            node.compound().map(|placed| Visit::Instance {
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
                            .push(NodeAddr::new(GraphId::Compound(owner), node_id)),
                        Some(Mark::Done) => {}
                        None if self.compounds.contains_key(&placed) => {
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

    /// The shape of each compound, taken in `post_order`, so each is known before an instance of
    /// it is counted.
    fn shapes(&self, post_order: &[CompoundId]) -> HashMap<CompoundId, ProgramShape> {
        let mut shapes = HashMap::with_capacity(post_order.len());
        for id in post_order {
            shapes.insert(*id, shape_of(&self.compounds[id].graph, &shapes));
        }
        shapes
    }
}

/// A node a rule across graphs refuses, and which.
#[derive(Clone, Copy, Debug)]
struct Refusal {
    node_id: NodeId,
    reason: DetachReason,
}

/// Whether `repair` is one of the two an attach and a detach of the ghost node `node_id` report.
fn reattaches(repair: &Repair, node_id: NodeId) -> bool {
    match repair {
        Repair::Reattached { node_id: ghost } | Repair::GhostedNode { node_id: ghost, .. } => {
            *ghost == node_id
        }
        _ => false,
    }
}

/// What `graph` expands into, given the shape of every compound it places.
fn shape_of(graph: &Graph, shapes: &HashMap<CompoundId, ProgramShape>) -> ProgramShape {
    graph.iter().fold(ProgramShape::default(), |shape, node| {
        shape.beside(ProgramShape::of_node(&node.kind, shapes))
    })
}

/// The nodes of `graph` that pass its bounds, in node-id order: each one whose own levels pass
/// [`ProgramShape::MAX_LEVELS`], and each one that would carry the count of the nodes before it
/// past [`ProgramShape::MAX_NODES`].
fn over_bounds(graph: &Graph, shapes: &HashMap<CompoundId, ProgramShape>) -> Vec<Refusal> {
    let nodes = graph
        .iter()
        .map(|node| (node.node_id, ProgramShape::of_node(&node.kind, shapes)));
    let mut kept = ProgramShape::default();
    let mut refused = Vec::new();
    for (node_id, shape) in nodes {
        if shape.levels > ProgramShape::MAX_LEVELS {
            refused.push(Refusal {
                node_id,
                reason: DetachReason::TooDeep,
            });
        } else if kept.beside(shape).node_count > ProgramShape::MAX_NODES {
            refused.push(Refusal {
                node_id,
                reason: DetachReason::TooLarge,
            });
        } else {
            kept = kept.beside(shape);
        }
    }
    refused
}

/// Each type a saved declaration's ports name.
fn decl_types(decl: &SavedDecl) -> impl Iterator<Item = DataType> {
    let inputs = decl.inputs.iter().map(|input| input.data_type);
    inputs.chain(decl.outputs.iter().map(|output| output.data_type))
}

#[cfg(test)]
mod tests;
