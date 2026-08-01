//! The fixture that turns a graph into a placed, laid-out [`Document`] and the
//! library it resolves against.
//!
//! Lives with [`Document`] rather than beside any one test because the document
//! is what every altitude above it composes — a context fixture and a canvas
//! harness both start by building one.

use glam::Vec2;
use scenarium::testing::graph::TestGraph;
use scenarium::{
    DataType, Func, FuncId, FuncInput, FuncOutput, Graph, Library, Node, NodeId, NodeKind, testing,
};

use crate::core::document::dock::DockOp;
use crate::core::document::{Document, GraphView, ItemPlacement, TabRef};

/// Lay every placement out along a row so no node lands off-viewport and gets
/// culled — [`GraphView::new`] seeds every item at the origin.
fn spread(view: &mut GraphView) {
    for (i, (id, _)) in view.paint_order().into_iter().enumerate() {
        view.item_placements
            .get_mut(&id)
            .expect("paint order lists only placed items")
            .pos = row_pos(i);
    }
}

/// Place `node_id` at `pos`, frontmost — so a fixture's paint order is the
/// order its constructors added in, which is what [`DocFixture::node`] indexes.
fn place(view: &mut GraphView, node_id: NodeId, pos: Vec2) {
    let z = view.front_z();
    view.item_placements
        .insert(node_id, ItemPlacement { pos, z });
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
pub(crate) struct DocFixture {
    pub(crate) doc: Document,
    pub(crate) library: Library,
}

impl DocFixture {
    /// A document over a hand-built `graph` and the library it resolves
    /// against — the [`TestGraph`] conversion for the cases that fixture can't
    /// express, like a node naming a func the library was never given.
    pub(crate) fn with_library(graph: Graph, library: Library) -> Self {
        let mut doc = Document::from(graph);
        spread(&mut doc.main_view);
        Self { doc, library }
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
        let probe = testing::with_stub_lambda(
            Func::new(FuncId::unique(), "probe")
                .pure()
                .input(FuncInput::optional("a", DataType::Int))
                .output(FuncOutput::new("out", DataType::Int)),
        );
        let mut fixture = Self::default();
        for _ in 0..n {
            fixture.add(&probe);
        }
        fixture
    }

    /// Nodes at caller-chosen ids and exact positions, each from a func no
    /// library holds — so they resolve as portless stubs. Enough for the tests
    /// that read only a node's identity and where it sits, and that need to
    /// name the ids before the document exists.
    pub(crate) fn stubs(nodes: impl IntoIterator<Item = (NodeId, Vec2)>) -> Self {
        let mut fixture = Self::default();
        for (node_id, pos) in nodes {
            fixture
                .doc
                .graph
                .insert(node_id, Node::new(NodeKind::Func(FuncId::unique())));
            place(&mut fixture.doc.main_view, node_id, pos);
        }
        fixture
    }

    /// Add one node projected from `func`, at the next slot in the row, and
    /// declare `func` in the library unless it is already there — so the node
    /// resolves rather than reading as a stub.
    pub(crate) fn add(&mut self, func: &Func) -> NodeId {
        if self.library.by_id(func.id).is_none() {
            self.library.add(func.clone());
        }
        let node_id = self.doc.graph.add_func_node(func);
        let slot = self.doc.main_view.item_placements.len();
        place(&mut self.doc.main_view, node_id, row_pos(slot));
        node_id
    }

    /// Add a bare `Func`-kind node at exactly `pos`, naming a func no library
    /// holds — a portless stub, for the tests that read only a node's identity
    /// and where it sits.
    pub(crate) fn stub_at(&mut self, pos: Vec2) -> NodeId {
        let node_id = self
            .doc
            .graph
            .add(Node::new(NodeKind::Func(FuncId::unique())));
        place(&mut self.doc.main_view, node_id, pos);
        node_id
    }

    /// The `i`th node in placement order — the order the constructors add in,
    /// and the canvas's paint stack.
    pub(crate) fn node(&self, i: usize) -> NodeId {
        self.doc
            .main_view
            .paint_order()
            .get(i)
            .expect("the fixture placed that many nodes")
            .0
    }

    /// Move `id` to exactly `at`, overriding the row — for the tests that care
    /// *which* node a gesture reaches rather than merely that all of them are
    /// on screen.
    pub(crate) fn placed(mut self, id: NodeId, at: Vec2) -> Self {
        match self.doc.main_view.item_placements.get_mut(&id) {
            // Repositioning a node the fixture already placed must not
            // restack it — `node(i)` indexes paint order, and several tests
            // move one node after adding others.
            Some(placement) => placement.pos = at,
            None => place(&mut self.doc.main_view, id, at),
        }
        self
    }

    /// Open `tab` in the primary group and make it that group's active one —
    /// only a visible tab draws, so a test about drawing needs both halves.
    pub(crate) fn with_tab(mut self, tab: TabRef) -> Self {
        let primary = self.doc.layout.primary().id;
        self.doc.layout.find_or_insert(tab, primary);
        self.doc.layout.apply(DockOp::ActivateTab { tab });
        self
    }
}

impl From<TestGraph> for DocFixture {
    /// A document over `g`'s graph, resolving against the library `g` declared
    /// its funcs in. Every fixture above this one takes an `impl
    /// Into<DocFixture>`, so a test can hand its [`TestGraph`] straight to a
    /// context or a canvas.
    fn from(g: TestGraph) -> Self {
        Self::with_library(g.graph, g.library)
    }
}
