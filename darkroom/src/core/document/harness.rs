//! Fixtures for building a [`Document`] to test against: the libraries a test
//! projects its nodes from, and the builder that turns a graph into a placed,
//! laid-out document.
//!
//! Lives with [`Document`] rather than beside any one test because the document
//! is what every altitude above it composes — a context fixture and a canvas
//! harness both start by building one.

use glam::Vec2;
use scenarium::testing::graph::TestGraph;
use scenarium::{
    DataType, Func, FuncId, FuncInput, FuncOutput, Graph, Library, Node, NodeId, NodeKind,
    OutputType, testing,
};

use crate::core::document::dock::DockOp;
use crate::core::document::{Document, GraphView, TabRef};

/// One func with an input and an output, so a node projected from it
/// records the full body: header badges, both port columns, and the
/// const editor on the unbound input.
pub(crate) fn one_func_library() -> Library {
    let mut library = Library::default();
    library.add(testing::with_stub_lambda(
        Func::new(FuncId::unique(), "probe")
            .pure()
            .input(FuncInput::optional("a", DataType::Int))
            .output(FuncOutput::new("out", DataType::Int)),
    ));
    library
}

/// [`one_func_library`] plus a passthrough: one input and one wildcard output
/// mirroring it, so the projected type of its output is whatever the *graph*
/// wires in rather than anything its declaration states.
pub(crate) fn wildcard_library() -> Library {
    let mut library = one_func_library();
    let mut passthrough = Func::new(FuncId::unique(), "passthrough")
        .pure()
        .input(FuncInput::optional("in", DataType::Any));
    passthrough.outputs.push(FuncOutput {
        name: "out".to_owned(),
        description: None,
        ty: OutputType::Wildcard { mirrors: 0 },
    });
    library.add(testing::with_stub_lambda(passthrough));
    library
}

/// Lay every placement out along a row so no node lands off-viewport and gets
/// culled — [`GraphView::for_graph`] seeds every item at the origin.
pub(crate) fn spread(view: &mut GraphView) {
    for (i, pos) in view.item_placements.values_mut().enumerate() {
        *pos = row_pos(i);
    }
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
    /// A document over `g`'s graph, resolving against the library `g` declared
    /// its funcs in.
    pub(crate) fn over(g: TestGraph) -> Self {
        Self::with_library(g.graph, g.library)
    }

    /// A document over a hand-built `graph` and the library it resolves
    /// against — [`Self::over`] for the cases [`TestGraph`] can't express, like
    /// a node naming a func the library was never given.
    pub(crate) fn with_library(graph: Graph, library: Library) -> Self {
        let mut doc = Document::from(graph);
        spread(&mut doc.main_view);
        Self { doc, library }
    }

    /// [`TestGraph::sample`]'s stock multi-node graph — what a test wants when
    /// it needs several wired nodes and doesn't care which is which.
    pub(crate) fn sample() -> Self {
        Self::over(TestGraph::sample())
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
            fixture.doc.main_view.item_placements.insert(node_id, pos);
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
        self.doc
            .main_view
            .item_placements
            .insert(node_id, row_pos(slot));
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
        self.doc.main_view.item_placements.insert(node_id, pos);
        node_id
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
