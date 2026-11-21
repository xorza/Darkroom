//! The fixture that turns a graph into a placed, laid-out [`Document`] and the
//! library it resolves against.
//!
//! Lives with [`Document`] because every layer above composes one: a context fixture and a
//! canvas harness both start by building it.

#![cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "a test fixture tree: the harness's project fixture uses part of it"
    )
)]

use glam::Vec2;
use scenarium::internals::test_graph::TestGraph;
use scenarium::{
    Binding, Compound, CompoundId, ConstValue, DataType, DetachReason, EventPort, Func, FuncId,
    GhostBinding, GhostNode, GraphId, GraphSet, InputDecl, InputPort, InputState, Library, Node,
    NodeId, OutputDecl, OutputPort, PortId, Subscription, internals,
};

use palantir::DockOperation;

use crate::core::document::Document;
use crate::core::document::compound_side::CompoundSide;
use crate::core::document::graph_view::{GraphView, NodePlacement};
use crate::core::document::route::Route;
use crate::core::document::tab_ref::TabRef;
use crate::core::document::tie::{Tie, TieAngle, TieId};
use crate::core::document::wire_key::{DataWire, WireKey};

/// Lay every placement out along a row so none is culled off-viewport; [`GraphView::new`] seeds
/// every item at the origin.
fn spread(view: &mut GraphView) {
    let mut order = Vec::new();
    view.paint_order(&mut order);
    for (i, item) in order.into_iter().enumerate() {
        view.node_placements
            .get_mut(&item.node_id)
            .expect("paint order lists only placed items")
            .pos = row_pos(i);
    }
}

/// Put `compound` into `document` under `id`, with a side whose view places its nodes in a row.
pub(crate) fn insert_compound(document: &mut Document, id: CompoundId, compound: Compound) {
    let mut side = CompoundSide::new(&compound.graph);
    spread(&mut side.view);
    document.insert_compound(id, compound, side);
}

/// Place `node_id` at `pos`, frontmost, so paint order is the order constructors added in, which
/// [`DocumentFixture::node`] indexes.
fn place(view: &mut GraphView, node_id: NodeId, pos: Vec2) {
    let z = view.front_z();
    view.node_placements
        .insert(node_id, NodePlacement { pos, z });
}

/// The `i`th node in placement order, which [`place`] adds in and the canvas paints in.
///
/// A free fn because the context fixture one level up indexes the same order over an
/// `OpenDocument` it owns, with no `DocumentFixture` left to ask.
pub(crate) fn nth_in_paint_order(view: &GraphView, i: usize) -> NodeId {
    let mut order = Vec::new();
    view.paint_order(&mut order);
    order
        .get(i)
        .expect("the fixture placed that many nodes")
        .node_id
}

/// The func of [`DocumentFixture::plain_node`].
fn plain_func() -> Func {
    internals::stub_func(
        FuncId::literal("1ed07f16-02ed-4fb2-9677-5aa67801c7ae"),
        "plain",
    )
    .sink()
    .input(1, InputDecl::optional("a", DataType::Int))
    .input(2, InputDecl::optional("b", DataType::Int))
    .input(3, InputDecl::optional("c", DataType::Int))
    .output(1, OutputDecl::new("out", DataType::Int))
    .event(1, "tick", internals::stub_event())
    .build()
}

/// The `i`th slot of the row [`spread`] lays out.
fn row_pos(i: usize) -> Vec2 {
    Vec2::new(40.0 + i as f32 * 220.0, 40.0)
}

/// A document and the library it resolves against, built together.
///
/// Every constructor spreads: [`Document::from`] seeds each placement at the origin, so without
/// it a recording test culls all but the topmost node.
#[derive(Debug, Default)]
pub(crate) struct DocumentFixture {
    pub(crate) document: Document,
    pub(crate) library: Library,
}

impl DocumentFixture {
    /// A document over hand-built `graphs` and the library they resolve against, for cases
    /// [`TestGraph`] can't express, like a node naming a func the library was never given.
    pub(crate) fn with_library(graphs: impl Into<GraphSet>, library: Library) -> Self {
        let mut document = Document::from(graphs.into());
        spread(&mut document.main_view);
        for side in document.sides.values_mut() {
            spread(&mut side.view);
        }
        Self { document, library }
    }

    /// [`TestGraph::sample`]'s stock multi-node graph.
    pub(crate) fn sample() -> Self {
        TestGraph::sample().into()
    }

    /// `n` nodes laid out along the row, all projected from one `probe` func with an input and an
    /// output, so each records the full body and a port index means the same on every one.
    pub(crate) fn probes(n: usize) -> Self {
        let probe = internals::stub_func(FuncId::unique(), "probe")
            .pure()
            .input(1, InputDecl::optional("a", DataType::Int))
            .output(1, OutputDecl::new("out", DataType::Int))
            .build();
        let mut fixture = Self::default();
        for _ in 0..n {
            fixture.add(&probe);
        }
        fixture
    }

    /// Add one node projected from `func`, at the next slot in the row, and declare `func` in the
    /// library unless it is already there, so the node resolves rather than reading as a ghost node.
    pub(crate) fn add(&mut self, func: &Func) -> NodeId {
        if self.library.by_id(func.id()).is_none() {
            self.library.add(func.clone());
        }
        let node_id = self.document.graphs.main_mut().add_func_node(func);
        let slot = self.document.main_view.node_placements.len();
        place(&mut self.document.main_view, node_id, row_pos(slot));
        node_id
    }

    /// Add an instance of compound `id`, which the document holds, at the next slot in the row.
    pub(crate) fn add_instance(&mut self, id: CompoundId) -> NodeId {
        assert!(
            self.document.graphs.compound(id).is_some(),
            "the document holds the compound"
        );
        let node = Node::instance(id);
        let node_id = self.document.graphs.main_mut().add(node);
        let slot = self.document.main_view.node_placements.len();
        place(&mut self.document.main_view, node_id, row_pos(slot));
        node_id
    }

    /// A node of the fixture's plain func, which the library then holds: a sink, so it hears on
    /// its pin, with three optional `Int` inputs, one `Int` output and one event.
    pub(crate) fn plain_node(&mut self) -> Node {
        let func = plain_func();
        if self.library.by_id(func.id()).is_none() {
            self.library.add(func.clone());
        }
        Node::from(&func)
    }

    /// Add a [`Self::plain_node`] at exactly `pos`.
    pub(crate) fn node_at(&mut self, pos: Vec2) -> NodeId {
        let node = self.plain_node();
        let node_id = self.document.graphs.main_mut().add(node);
        place(&mut self.document.main_view, node_id, pos);
        node_id
    }

    /// Add a ghost node at exactly `pos`: a node naming a func no library holds, kept beside
    /// the graph with no wiring.
    pub(crate) fn ghost_node_at(&mut self, pos: Vec2) -> NodeId {
        self.insert_ghost_node(Self::missing_node(), pos)
    }

    /// Keep `node`, which the library must not resolve, as a ghost node with no
    /// wiring at the next slot in the row.
    pub(crate) fn add_ghost_node(&mut self, node: Node) -> NodeId {
        let slot = self.document.main_view.node_placements.len();
        self.insert_ghost_node(node, row_pos(slot))
    }

    fn insert_ghost_node(&mut self, node: Node, pos: Vec2) -> NodeId {
        let reason = self
            .document
            .graphs
            .scope(&self.library, GraphId::Main)
            .resolve(&node)
            .expect_err("a ghost node's func does not resolve");
        let node_id = NodeId::unique();
        self.document.graphs.main_mut().insert_ghost_node(
            node_id,
            GhostNode {
                node,
                reason: DetachReason::from(reason),
                saved: None,
            },
        );
        place(&mut self.document.main_view, node_id, pos);
        node_id
    }

    /// A `Func`-kind node naming a func no library holds — what
    /// [`Self::ghost_node_at`] places, for a test that adds it itself.
    pub(crate) fn missing_node() -> Node {
        internals::unresolved_node(FuncId::unique())
    }

    /// Keep `value` as a ghost on input 1 of the ghost node `ghost`.
    pub(crate) fn bind_ghost_node(&mut self, ghost: NodeId, value: ConstValue) {
        self.document.graphs.main_mut().set_input_state(
            InputPort::new(ghost, PortId::new(1)),
            InputState::Ghost(GhostBinding::from(Binding::Const(value))),
        );
    }

    /// The `i`th node in placement order — the order the constructors add in,
    /// and the canvas's paint stack.
    pub(crate) fn node(&self, i: usize) -> NodeId {
        nth_in_paint_order(&self.document.main_view, i)
    }

    /// Move `id` to exactly `at`, overriding the row.
    pub(crate) fn placed(mut self, id: NodeId, at: Vec2) -> Self {
        match self.document.main_view.node_placements.get_mut(&id) {
            // Must not restack: `node(i)` indexes paint order.
            Some(placement) => placement.pos = at,
            None => place(&mut self.document.main_view, id, at),
        }
        self
    }

    /// Wire `input` to `output`, with no check that the ports exist or the
    /// types fit.
    pub(crate) fn wire(&mut self, output: OutputPort, input: InputPort) -> WireKey {
        self.document
            .graphs
            .main_mut()
            .set_input_binding(input, Binding::Wire(output));
        WireKey::Data(DataWire { output, input })
    }

    /// Subscribe `subscriber` to `event`.
    pub(crate) fn subscribe(&mut self, event: EventPort, subscriber: NodeId) -> WireKey {
        self.document.graphs.main_mut().subscribe(event, subscriber);
        WireKey::Event(Subscription::new(event, subscriber))
    }

    /// Run each of `wires` through a new tie at `pos` that turns itself, last
    /// on the wire's route.
    pub(crate) fn tie(&mut self, pos: Vec2, wires: impl IntoIterator<Item = WireKey>) -> TieId {
        let tie = TieId::unique();
        let view = &mut self.document.main_view;
        view.ties.insert(
            tie,
            Tie {
                pos,
                angle: TieAngle::Auto,
            },
        );
        for wire in wires {
            let route = match view.routes.get(&wire) {
                Some(route) => Route::new(route.iter().copied().chain([tie])),
                None => Route::new([tie]),
            };
            view.routes.insert(wire, route);
        }
        tie
    }

    /// Open `tab` in the primary group, focused and active, as only a visible tab draws.
    pub(crate) fn with_tab(mut self, tab: TabRef) -> Self {
        let group = self.document.layout.primary().id();
        let layout = &mut self.document.layout;
        layout.apply(DockOperation::FocusGroup { group });
        layout.apply(DockOperation::OpenTab { tab });
        self
    }
}

impl From<TestGraph> for DocumentFixture {
    /// A document over `g`'s graph, resolving against the library `g` declared its funcs in.
    /// Every fixture above this one takes an `impl Into<DocumentFixture>`, so a test hands its
    /// [`TestGraph`] straight to a context or a canvas.
    fn from(g: TestGraph) -> Self {
        let TestGraph {
            graphs, library, ..
        } = g;
        Self::with_library(graphs, library)
    }
}
