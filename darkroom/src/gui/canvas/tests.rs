use glam::{UVec2, Vec2};
use palantir::internals::UiHarness;
use palantir::{Configure, Panel, Sizing, Ui};
use scenarium::{
    Binding, CacheMode, DataType, Func, FuncId, FuncInput, FuncOutput, Graph, GraphDef, GraphId,
    InputPort, Library, Node, NodeKind, testing,
};

use crate::core::document::{GraphRef, GraphView, PortKind, PortRef};
use crate::core::edit::intent::sink::Intents;
use crate::core::edit::intent::types::{Intent, NodeProperty};
use crate::gui::app::AppContext;
use crate::gui::canvas::GraphUI;
use crate::gui::canvas::inspector::{inspect_badge_wid, inspect_panel_wid};
use crate::gui::graph_toolbar;
use crate::gui::node::port_row::port_circle_wid;
use crate::gui::node::{node_wid, node_widget_id};
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

/// Two graph panes drawn in one frame must not record a widget id twice,
/// and every intent either raises must name the pane it came from.
///
/// Every pane runs the *same* draw code, so anything keyed by a constant
/// rather than by the pane (`GraphRef`) or by a document-unique domain id
/// (`NodeId` / `PortRef`) collides the moment a second pane opens — and a
/// collision silently re-keys cross-frame widget state, so the panes fight
/// over one scroll offset / text cursor / animation row. Palantir reports
/// explicit-id collisions through `Forest.collisions`, which is what this
/// asserts on: a root pane and a local-definition pane, side by side.
///
/// The target half is the same hazard one layer down: nothing in a node-body
/// signature says which graph it edits, so a site reaching for the wrong
/// `GraphRef` commits into the *other* pane's graph — silently dropped where
/// the payload names a node the target doesn't hold, silently applied where
/// it doesn't (`SetViewport`). Both halves are checked here because both need
/// two panes on screen to show up at all.
#[test]
fn two_graph_panes_record_no_duplicate_widget_ids_and_edit_only_themselves() {
    let library = one_func_library();
    let probe = library.by_name("probe").expect("just added");

    // Root pane: two func nodes, the second bound to the first, so wires,
    // both port kinds, and a const editor all record.
    let mut root = Graph::default();
    let upstream = root.add_func_node(probe);
    let downstream = root.add_func_node(probe);
    root.set_input_binding(InputPort::new(downstream, 0), Binding::bind(upstream, 0));

    // Local-definition pane: boundary nodes, which draw the port-rename
    // widgets the root pane never records, plus a func node — the boundary
    // pair carries none of the header chips the record-phase intents come
    // from, and its input is wired so a double-click has a binding to clear.
    let mut def = GraphDef::new("Adder")
        .inputs([FuncInput::optional("a", DataType::Int)])
        .output(FuncOutput::new("sum", DataType::Int));
    let def_in = def.body.add(Node::new(NodeKind::GraphInput));
    let def_out = def.body.add(Node::new(NodeKind::GraphOutput));
    let def_func = def.body.add_func_node(probe);
    def.body
        .set_input_binding(InputPort::new(def_out, 0), Binding::bind(def_in, 0));
    def.body
        .set_input_binding(InputPort::new(def_func, 0), Binding::bind(def_in, 0));

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

    // Returns what the frame queued, so the target assertions below read the
    // same sink the real pipeline drains.
    let mut draw = |ui: &mut Ui| -> Vec<(GraphRef, Intent)> {
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
        intents.drain().collect()
    };

    // Two frames: the first fills `CanvasGeometry` and the response cache
    // the second draws against, which is when the node subtrees record in
    // full. A collision on any frame fails the assert below.
    let mut seen_collisions = Vec::new();
    for _ in 0..2 {
        harness.frame_value(&mut draw);
        seen_collisions.extend(harness.collisions());
    }

    // Open an inspector and draw again. `Inspectors::modes` is a
    // document-wide `NodeId` map that *every* pane iterates, so the panel
    // ids are the one place where a per-pane draw walks a set it doesn't
    // own — it stays single-pane only because `GraphScene::node` filters
    // on `owner`. Exercise it rather than trusting the read.
    harness.click_on(inspect_badge_wid(upstream));
    harness.frame_value(&mut draw);
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

    // Record phase: a header chip on the definition pane's node. The target
    // has to come off `SceneNode::owner` — the widget has no other way to
    // know which of the two graphs it is drawing.
    harness.click_on(node_wid("ram_badge", def_func));
    let emitted = harness.frame_value(&mut draw);
    assert!(
        matches!(
            emitted[..],
            [(
                target,
                Intent::SetNodeProperty {
                    node_id,
                    to: NodeProperty::RuntimeCache(CacheMode::Ram),
                },
            )] if target == local && node_id == def_func,
        ),
        "the cache chip must flip the RAM bit of the node it sits on, in its \
         own pane: {emitted:?}"
    );

    // Prepass phase: a port double-click on the same node clears its binding.
    // Scanned per pane rather than per node, so it reads its target off the
    // `GraphScene` being swept.
    let port = PortRef {
        node_id: def_func,
        kind: PortKind::Input,
        port_idx: 0,
    };
    harness.click_on(port_circle_wid(port));
    harness.frame_value(&mut draw);
    harness.click_on(port_circle_wid(port));
    let emitted = harness.frame_value(&mut draw);
    assert!(
        matches!(
            emitted[..],
            [(target, Intent::SetInput { input, to: None })]
                if target == local && input == InputPort::new(def_func, 0),
        ),
        "the double-click must clear the binding of the port it landed on, in \
         its own pane: {emitted:?}"
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
