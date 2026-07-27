use glam::{UVec2, Vec2};
use palantir::internals::UiHarness;
use palantir::{Configure, Panel, Sizing, Ui};
use scenarium::{
    Binding, DataType, Func, FuncId, FuncInput, FuncOutput, Graph, GraphDef, GraphId, InputPort,
    Library, Node, NodeKind, testing,
};

use crate::core::document::{GraphRef, GraphView};
use crate::core::edit::intent::sink::Intents;
use crate::gui::app::AppContext;
use crate::gui::canvas::GraphUI;
use crate::gui::canvas::inspector::{inspect_badge_wid, inspect_panel_wid};
use crate::gui::graph_toolbar;
use crate::gui::node::node_widget_id;
use crate::gui::run_state::RunState;
use crate::gui::scene::{GraphProjection, Scene, SceneSource};
use crate::gui::theme::Theme;

/// One func with an input and an output, so a node projected from it
/// records the full body: header badges, both port columns, and the
/// const editor on the unbound input.
fn one_func_library() -> Library {
    let mut library = Library::default();
    library.add(testing::with_stub_lambda(
        Func::new(FuncId::unique(), "probe")
            .pure()
            .input(FuncInput::optional("a", DataType::Int))
            .output(FuncOutput::new("out", DataType::Int)),
    ));
    library
}

/// Spread the placements so no node lands off-viewport and gets culled —
/// `GraphView::for_graph` seeds every item at the origin.
fn spread(view: &mut GraphView) {
    for (i, pos) in view.item_placements.values_mut().enumerate() {
        *pos = Vec2::new(40.0 + i as f32 * 220.0, 40.0);
    }
}

/// Two graph panes drawn in one frame must not record a widget id twice.
///
/// Every pane runs the *same* draw code, so anything keyed by a constant
/// rather than by the pane (`GraphRef`) or by a document-unique domain id
/// (`NodeId` / `PortRef`) collides the moment a second pane opens — and a
/// collision silently re-keys cross-frame widget state, so the panes fight
/// over one scroll offset / text cursor / animation row. Palantir reports
/// explicit-id collisions through `Forest.collisions`, which is what this
/// asserts on: a root pane and a local-definition pane, side by side.
#[test]
fn two_graph_panes_record_no_duplicate_widget_ids() {
    let library = one_func_library();
    let probe = library.by_name("probe").expect("just added");

    // Root pane: two func nodes, the second bound to the first, so wires,
    // both port kinds, and a const editor all record.
    let mut root = Graph::default();
    let upstream = root.add_func_node(probe);
    let downstream = root.add_func_node(probe);
    root.set_input_binding(InputPort::new(downstream, 0), Binding::bind(upstream, 0));

    // Local-definition pane: boundary nodes, which draw the port-rename
    // widgets the root pane never records.
    let mut def = GraphDef::new("Adder")
        .inputs([FuncInput::optional("a", DataType::Int)])
        .output(FuncOutput::new("sum", DataType::Int));
    let def_in = def.body.add(Node::new(NodeKind::GraphInput));
    let def_out = def.body.add(Node::new(NodeKind::GraphOutput));
    def.body
        .set_input_binding(InputPort::new(def_out, 0), Binding::bind(def_in, 0));

    let local = GraphRef::Local(GraphId::unique());
    let mut root_view = GraphView::for_graph(&root);
    let mut def_view = GraphView::for_graph(&def.body);
    spread(&mut root_view);
    spread(&mut def_view);

    let theme = Theme::default();
    let run_state = RunState::default();
    let ctx = AppContext {
        theme: &theme,
        library: &library,
        run_state: &run_state,
        status_error: None,
    };

    let mut graph_ui = GraphUI::default();
    let mut scene = Scene::default();
    let mut intents = Intents::default();
    let mut command = None;
    let mut harness = UiHarness::new(UVec2::new(1600, 900));

    let mut draw = |ui: &mut Ui| {
        scene.rebuild(
            ui,
            &library,
            &run_state,
            [
                GraphProjection {
                    target: GraphRef::Main,
                    source: SceneSource::Entry(&root),
                    view: &root_view,
                },
                GraphProjection {
                    target: local,
                    source: SceneSource::Def(&def),
                    view: &def_view,
                },
            ],
        );
        graph_ui.prepass(ui, &scene, &mut intents);
        // Mirrors `main_window`'s per-pane content closure: each pane's
        // subtree under its own `("graph_overlay", target)` parent.
        Panel::hstack()
            .id_salt("panes")
            .size((Sizing::FILL, Sizing::FILL))
            .show(ui, |ui| {
                for target in [GraphRef::Main, local] {
                    let graph = scene.graph(target).expect("projected");
                    Panel::zstack()
                        .id_salt(("graph_overlay", target))
                        .size((Sizing::FILL, Sizing::FILL))
                        .show(ui, |ui| {
                            graph_ui.draw(ui, &ctx, graph, &mut intents, &mut command);
                            graph_toolbar::show(ui, &ctx, graph, &graph_ui.geometry, &mut intents);
                        });
                }
            });
    };

    // Two frames: the first fills `CanvasGeometry` and the response cache
    // the second draws against, which is when the node subtrees record in
    // full. A collision on any frame fails the assert below.
    let mut seen_collisions = Vec::new();
    for _ in 0..2 {
        harness.frame(&mut draw);
        seen_collisions.extend(harness.collisions());
    }

    // Open an inspector and draw again. `Inspectors::modes` is a
    // document-wide `NodeId` map that *every* pane iterates, so the panel
    // ids are the one place where a per-pane draw walks a set it doesn't
    // own — it stays single-pane only because `GraphScene::node` filters
    // on `owner`. Exercise it rather than trusting the read.
    harness.click_on(inspect_badge_wid(upstream));
    harness.frame(&mut draw);
    seen_collisions.extend(harness.collisions());
    assert!(
        harness.rect(inspect_panel_wid(upstream)).is_some(),
        "the inspector panel never opened — the owner filter went untested"
    );

    // Guard against a vacuous pass: a frame that recorded nothing also
    // reports no collisions.
    for node in [upstream, downstream, def_in, def_out] {
        assert!(
            harness.rect(node_widget_id(node)).is_some(),
            "node {node:?} never recorded — the frame drew nothing to audit"
        );
    }
    assert!(
        seen_collisions.is_empty(),
        "two panes recorded the same widget id: {seen_collisions:?}"
    );
}
