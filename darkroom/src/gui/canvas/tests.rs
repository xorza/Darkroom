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
        graph_ui.prepass(ui, &scene, &Library::default(), &mut intents);
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

/// Ctrl+drag off an output port spawns a preview node already reading it, as
/// one batch — the one-gesture counterpart to the port menu's "Add preview".
///
/// Drives the real chord through the harness rather than calling the gesture
/// directly: the whole point of the modifier is that `ConnectionUI` and
/// `PreviewDrag` can't both claim the same press, and only a real press
/// exercises that.
#[test]
fn ctrl_drag_off_an_output_spawns_a_preview_wired_to_it() {
    use palantir::{Modifiers, PointerButton};

    use crate::core::document::{PortKind, PortRef};
    use crate::core::edit::intent::types::Intent;
    use crate::core::preview::{PreviewSink, preview_func};
    use crate::gui::node::port_row::port_circle_wid;

    let mut library = one_func_library();
    library.add(preview_func(std::sync::Arc::<PreviewSink>::default()));
    let probe = library.by_name("probe").expect("just added").clone();

    let mut root = Graph::default();
    let producer = root.add_func_node(&probe);
    let mut view = GraphView::for_graph(&root);
    spread(&mut view);

    let mut harness = UiHarness::new(UVec2::new(1200, 800));
    let theme = Theme::default();
    let run_state = RunState::default();
    let mut graph_ui = GraphUI::default();
    let mut scene = Scene::default();
    let out_port = PortRef {
        node_id: producer,
        kind: PortKind::Output,
        port_idx: 0,
    };

    // One frame to record the node so its output circle has a widget id and
    // `CanvasGeometry` a measured center; the gesture refuses an unmeasured
    // port outright.
    let draw = |ui: &mut Ui, graph_ui: &mut GraphUI, scene: &mut Scene| {
        let ctx = AppContext {
            theme: &theme,
            library: &library,
            run_state: &run_state,
            status_error: None,
        };
        let mut intents = Intents::default();
        scene.rebuild(
            ui,
            &library,
            &run_state,
            [GraphProjection {
                target: GraphRef::Main,
                source: SceneSource::Entry(&root),
                view: &view,
            }],
        );
        graph_ui.prepass(ui, scene, &library, &mut intents);
        let graph = scene.graph(GraphRef::Main).expect("projected");
        Panel::vstack()
            .id_salt("pane")
            .size((Sizing::FILL, Sizing::FILL))
            .show(ui, |ui| {
                graph_ui.draw(ui, &ctx, graph, &mut intents, &mut None);
            });
        intents.drain().collect::<Vec<_>>()
    };

    harness.frame(|ui| {
        draw(ui, &mut graph_ui, &mut scene);
    });
    harness.frame(|ui| {
        draw(ui, &mut graph_ui, &mut scene);
    });

    // Ctrl held, press on the output circle, then drag: the press frame is
    // where `PreviewDrag` latches.
    harness.set_modifiers(Modifiers {
        ctrl: true,
        ..Default::default()
    });
    let circle = port_circle_wid(out_port);
    harness.press_on(circle);
    harness.frame(|ui| {
        draw(ui, &mut graph_ui, &mut scene);
    });
    // `first_drag_started` polls the *drag* edge, not the press, so the
    // pointer has to actually move past palantir's threshold.
    let from = harness
        .ui()
        .response_for(circle)
        .layout_rect
        .expect("recorded")
        .center();
    harness.drag_to(from + Vec2::new(90.0, 40.0));
    let spawned = harness.frame_value(|ui| draw(ui, &mut graph_ui, &mut scene));
    harness.set_modifiers(Modifiers::default());
    harness.release_button(PointerButton::Left);

    let adds: Vec<_> = spawned
        .iter()
        .filter(|(_, intent)| matches!(intent, Intent::AddNode { .. }))
        .collect();
    assert_eq!(adds.len(), 1, "one preview spawned: {spawned:?}");
    let (target, Intent::AddNode { node_id, node, .. }) = adds[0] else {
        unreachable!("filtered to AddNode");
    };
    assert_eq!(
        *target,
        GraphRef::Main,
        "raised against the port's own pane"
    );
    assert!(
        matches!(node.kind, NodeKind::Func(id) if crate::core::preview::is_preview(id)),
        "the spawned node is a preview"
    );
    assert!(
        spawned.iter().any(|(_, intent)| matches!(
            intent,
            Intent::SetInput { input, to: Some(Binding::Bind(src)) }
                if input.node_id == *node_id
                    && src.node_id == producer
                    && src.port_idx == 0
        )),
        "and it is already reading the port the drag came off: {spawned:?}"
    );
}
