//! The fixture that turns a graph into a placed, laid-out [`Document`] and the
//! library it resolves against.
//!
//! Lives with [`Document`] rather than beside any one test because the document
//! is what every altitude above it composes — a context fixture and a canvas
//! harness both start by building one.

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
    Binding, DataType, DetachedNode, DetachedNodeParts, EventPort, Func, FuncId, FuncInput,
    FuncOutput, Graph, InputPort, Library, Node, NodeId, OutputPort, Subscription, internals,
};

use palantir::DockOperation;

use crate::core::document::Document;
use crate::core::document::graph_view::{GraphView, NodePlacement};
use crate::core::document::route::Route;
use crate::core::document::tab_ref::TabRef;
use crate::core::document::tie::{Tie, TieAngle, TieId};
use crate::core::document::wire_key::{DataWire, WireKey};

/// Lay every placement out along a row so no node lands off-viewport and gets
/// culled — [`GraphView::new`] seeds every item at the origin.
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

/// Place `node_id` at `pos`, frontmost — so a fixture's paint order is the
/// order its constructors added in, which is what [`DocumentFixture::node`] indexes.
fn place(view: &mut GraphView, node_id: NodeId, pos: Vec2) {
    let z = view.front_z();
    view.node_placements
        .insert(node_id, NodePlacement { pos, z });
}

/// The `i`th node in placement order — the order [`place`] adds in, and the
/// canvas's paint stack.
///
/// A free fn rather than a [`DocumentFixture`] method because the context fixture
/// one level up indexes the same order over an `OpenDocument` it owns
/// outright, with no `DocumentFixture` left to ask.
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
    .inputs(["a", "b", "c"].map(|name| FuncInput::optional(name, DataType::Int)))
    .output(FuncOutput::new("out", DataType::Int))
    .event("tick", internals::stub_event())
    .build()
}

/// The `i`th slot of the row [`spread`] lays out.
fn row_pos(i: usize) -> Vec2 {
    Vec2::new(40.0 + i as f32 * 220.0, 40.0)
}

/// A document and the library it resolves against, built together.
///
/// Every constructor spreads: [`Document::from`] seeds each placement at the
/// origin, so a document built without one stacks its whole graph in one spot
/// and a recording test culls all but the topmost node.
#[derive(Debug, Default)]
pub(crate) struct DocumentFixture {
    pub(crate) document: Document,
    pub(crate) library: Library,
}

impl DocumentFixture {
    /// A document over a hand-built `graph` and the library it resolves
    /// against — the [`TestGraph`] conversion for the cases that fixture can't
    /// express, like a node naming a func the library was never given.
    pub(crate) fn with_library(graph: Graph, library: Library) -> Self {
        let mut document = Document::from(graph);
        spread(&mut document.main_view);
        Self { document, library }
    }

    /// [`TestGraph::sample`]'s stock multi-node graph — what a test wants when
    /// it needs several wired nodes and doesn't care which is which.
    pub(crate) fn sample() -> Self {
        TestGraph::sample().into()
    }

    /// `n` nodes laid out along the row, all projected from one `probe` func
    /// with an input and an output — so each records the full body (header
    /// badges, both port columns, the const editor on the unbound input), and
    /// a port index means the same thing on every one of them.
    pub(crate) fn probes(n: usize) -> Self {
        let probe = internals::stub_func(FuncId::unique(), "probe")
            .pure()
            .input(FuncInput::optional("a", DataType::Int))
            .output(FuncOutput::new("out", DataType::Int))
            .build();
        let mut fixture = Self::default();
        for _ in 0..n {
            fixture.add(&probe);
        }
        fixture
    }

    /// Add one node projected from `func`, at the next slot in the row, and
    /// declare `func` in the library unless it is already there — so the node
    /// resolves rather than reading as a stub.
    pub(crate) fn add(&mut self, func: &Func) -> NodeId {
        if self.library.by_id(func.id()).is_none() {
            self.library.add(func.clone());
        }
        let node_id = self.document.graph.add_func_node(func);
        let slot = self.document.main_view.node_placements.len();
        place(&mut self.document.main_view, node_id, row_pos(slot));
        node_id
    }

    /// A node of the fixture's plain func, which the library then holds: three
    /// optional `Int` inputs, one `Int` output and one event — enough for any
    /// edit a test makes of a node whose ports it does not care about.
    pub(crate) fn plain_node(&mut self) -> Node {
        let func = plain_func();
        if self.library.by_id(func.id()).is_none() {
            self.library.add(func.clone());
        }
        Node::from(&func)
    }

    /// Add a [`Self::plain_node`] at exactly `pos` — for the tests that read
    /// only a node's identity and where it sits.
    pub(crate) fn node_at(&mut self, pos: Vec2) -> NodeId {
        let node = self.plain_node();
        let node_id = self.document.graph.add(node);
        place(&mut self.document.main_view, node_id, pos);
        node_id
    }

    /// Add a stub at exactly `pos`: a node naming a func no library holds,
    /// kept beside the graph with no wiring — for the tests that read only a
    /// node's identity and where it sits.
    pub(crate) fn stub_at(&mut self, pos: Vec2) -> NodeId {
        self.insert_stub(Self::stub_node(), pos)
    }

    /// Keep `node`, which the library must not resolve, as a stub with no
    /// wiring at the next slot in the row.
    pub(crate) fn add_stub(&mut self, node: Node) -> NodeId {
        let slot = self.document.main_view.node_placements.len();
        self.insert_stub(node, row_pos(slot))
    }

    fn insert_stub(&mut self, node: Node, pos: Vec2) -> NodeId {
        assert!(
            self.library.func_of(&node).is_err(),
            "a stub's func does not resolve"
        );
        let node_id = NodeId::unique();
        let record = DetachedNode::new(DetachedNodeParts {
            node_id,
            node,
            bindings: Vec::new(),
            subscriptions: Vec::new(),
        })
        .expect("a stub with no wiring is a well-formed record");
        self.document.stubs.insert(record);
        place(&mut self.document.main_view, node_id, pos);
        node_id
    }

    /// A `Func`-kind node naming a func no library holds — what
    /// [`Self::stub_at`] places, for a test that adds it itself.
    pub(crate) fn stub_node() -> Node {
        internals::unresolved_node(FuncId::unique())
    }

    /// The `i`th node in placement order — the order the constructors add in,
    /// and the canvas's paint stack.
    pub(crate) fn node(&self, i: usize) -> NodeId {
        nth_in_paint_order(&self.document.main_view, i)
    }

    /// Move `id` to exactly `at`, overriding the row — for the tests that care
    /// *which* node a gesture reaches rather than merely that all of them are
    /// on screen.
    pub(crate) fn placed(mut self, id: NodeId, at: Vec2) -> Self {
        match self.document.main_view.node_placements.get_mut(&id) {
            // Repositioning a node the fixture already placed must not
            // restack it — `node(i)` indexes paint order, and several tests
            // move one node after adding others.
            Some(placement) => placement.pos = at,
            None => place(&mut self.document.main_view, id, at),
        }
        self
    }

    /// Wire `input` to `output`, with no check that the ports exist or the
    /// types fit.
    pub(crate) fn wire(&mut self, output: OutputPort, input: InputPort) -> WireKey {
        self.document
            .graph
            .bindings
            .insert(input, Binding::Wire(output));
        WireKey::Data(DataWire { output, input })
    }

    /// Subscribe `subscriber` to `event`.
    pub(crate) fn subscribe(&mut self, event: EventPort, subscriber: NodeId) -> WireKey {
        self.document.graph.subscribe(event, subscriber);
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

    /// Open `tab` in the primary group and make it that group's active one —
    /// only a visible tab draws, so a test about drawing needs both halves.
    pub(crate) fn with_tab(mut self, tab: TabRef) -> Self {
        let primary = self.document.layout.primary().id;
        self.document.layout.find_or_insert(tab, primary);
        self.document
            .layout
            .apply(DockOperation::ActivateTab { tab });
        self
    }
}

impl From<TestGraph> for DocumentFixture {
    /// A document over `g`'s graph, resolving against the library `g` declared
    /// its funcs in. Every fixture above this one takes an `impl
    /// Into<DocumentFixture>`, so a test can hand its [`TestGraph`] straight to a
    /// context or a canvas.
    fn from(g: TestGraph) -> Self {
        Self::with_library(g.graph, g.library)
    }
}
