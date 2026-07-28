use glam::{UVec2, Vec2};
use palantir::internals::UiHarness;
use palantir::{Configure, Panel, Sizing, Ui};
use scenarium::{
    Binding, CacheMode, DataType, Func, FuncId, FuncInput, FuncOutput, Graph, GraphDef, GraphId,
    InputPort, Library, Node, NodeKind, testing,
};

use crate::core::document::{GraphRef, GraphView, PortKind, PortRef};
use crate::core::edit::intent::sink::{Intents, Queued};
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

/// The intent behind each queued item, checking on the way that every one
/// named `target`. Both assertions belong to every canvas test: a widget
/// edits the pane it drew from, and none of them can reach the dock.
fn scoped_intents(queued: &[Queued], target: GraphRef) -> Vec<&Intent> {
    queued
        .iter()
        .map(|item| match item {
            Queued::Scoped {
                target: named,
                intent,
            } => {
                assert_eq!(*named, target, "a canvas widget edits its own pane");
                intent
            }
            Queued::Global(intent) => panic!("a canvas widget raised {intent:?}"),
        })
        .collect()
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
        process_memory: 0,
    };

    let mut graph_ui = GraphUI::default();
    let mut scene = Scene::default();
    let mut intents = Intents::default();
    let mut command = None;
    let mut harness = UiHarness::new(UVec2::new(1600, 900));

    // Returns what the frame queued, so the target assertions below read the
    // same sink the real pipeline drains.
    let mut draw = |ui: &mut Ui| -> Vec<Queued> {
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
    let intents = scoped_intents(&emitted, local);
    assert!(
        matches!(
            intents[..],
            [Intent::SetNodeProperty {
                node_id,
                to: NodeProperty::RuntimeCache(CacheMode::Ram),
            }] if *node_id == def_func,
        ),
        "the cache chip must flip the RAM bit of the node it sits on: {intents:?}"
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
    let intents = scoped_intents(&emitted, local);
    assert!(
        matches!(
            intents[..],
            [Intent::SetInput { input, to: None }] if *input == InputPort::new(def_func, 0),
        ),
        "the double-click must clear the binding of the port it landed on: {intents:?}"
    );
}

/// The breaker cuts a node where the *document* says it is, not where it last
/// painted.
///
/// Both rects are one frame apart whenever something moves a node out from
/// under a live gesture — an undo, a scripted edit — because `SceneNode::pos`
/// is mirrored pre-record while the body's own arranged rect is still last
/// frame's. Driving that here: scribble over empty canvas, move the node onto
/// the scribble mid-gesture, release. The cut has to land, which it only does
/// if the probe reads the same `node_world_rect` the cull and the rubber band
/// do rather than re-deriving one off `response_for(node_widget_id(..))`.
#[test]
fn the_breaker_cuts_a_node_at_its_current_position_not_its_last_painted_one() {
    use palantir::PointerButton;

    use crate::core::document::GraphView;

    // Where the scribble runs: a short vertical stroke over empty canvas, well
    // clear of where the node starts.
    const SCRIBBLE_FROM: Vec2 = Vec2::new(600.0, 600.0);
    const SCRIBBLE_TO: Vec2 = Vec2::new(600.0, 420.0);

    let library = one_func_library();
    let probe = library.by_name("probe").expect("just added").clone();

    let mut root = Graph::default();
    let node = root.add_func_node(&probe);
    let mut view = GraphView::for_graph(&root);
    spread(&mut view);

    let theme = Theme::default();
    let run_state = RunState::default();
    let mut harness = UiHarness::new(UVec2::new(1200, 800));
    let mut graph_ui = GraphUI::default();
    let mut scene = Scene::default();

    // `view` is a parameter, not a capture, so it can be edited between frames
    // the way an undo would edit the document under a running gesture.
    let draw = |ui: &mut Ui, graph_ui: &mut GraphUI, scene: &mut Scene, view: &GraphView| {
        let ctx = AppContext {
            theme: &theme,
            library: &library,
            run_state: &run_state,
            status_error: None,
            process_memory: 0,
        };
        let mut intents = Intents::default();
        scene.rebuild(
            ui,
            &library,
            &run_state,
            [GraphProjection {
                target: GraphRef::Main,
                source: SceneSource::Entry(&root),
                view,
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
        draw(ui, &mut graph_ui, &mut scene, &view);
    });
    harness.frame(|ui| {
        draw(ui, &mut graph_ui, &mut scene, &view);
    });
    let body = harness
        .rect(node_widget_id(node))
        .expect("the node recorded a body");
    assert!(
        !body.contains(SCRIBBLE_TO),
        "the scribble must start out clear of the node, else the move proves nothing"
    );

    // Right-drag over empty canvas: the gesture latches and paints a polyline
    // nowhere near the node, so nothing is marked.
    harness.press_button_at(PointerButton::Right, SCRIBBLE_FROM);
    harness.drag_to(SCRIBBLE_TO);
    let scribbling = harness.frame_value(|ui| draw(ui, &mut graph_ui, &mut scene, &view));
    assert!(
        scribbling.is_empty(),
        "a scribble in flight severs nothing until release: {scribbling:?}"
    );

    // Now move the node onto the scribble, centred on its far end. This frame
    // the document says the node is here while its arranged rect still says it
    // is back there — the divergence the probe has to resolve the new way.
    view.item_placements[&node] = SCRIBBLE_TO - Vec2::new(body.size.w, body.size.h) * 0.5;
    harness.frame(|ui| {
        draw(ui, &mut graph_ui, &mut scene, &view);
    });

    harness.release_button(PointerButton::Right);
    let released = harness.frame_value(|ui| draw(ui, &mut graph_ui, &mut scene, &view));
    // The helper carries the pane assertion: a cut commits against the pane
    // the scribble ran on.
    let released = scoped_intents(&released, GraphRef::Main);
    assert!(
        matches!(released[..], [Intent::RemoveNode { node_id }] if *node_id == node),
        "the release cuts the node the scribble now crosses: {released:?}"
    );
}

/// A node-body right-click selects the node it landed on before the menu
/// opens, so whatever the user picks next acts on a coherent set. A boundary
/// interface node carries no structural identity to duplicate or remove, so it
/// offers no menu at all and a right-click on one is inert.
///
/// Both halves come out of the one closure the shared trigger scan takes —
/// "which of this node's widgets opens the menu, or `None` if it offers no
/// menu" — so they're checked through real clicks rather than by calling it.
#[test]
fn a_node_body_right_click_selects_that_node_and_boundary_nodes_offer_nothing() {
    let library = one_func_library();
    let probe = library.by_name("probe").expect("just added").clone();

    // A definition body, so the pane holds an ordinary node beside the two
    // boundary nodes that have to stay inert.
    let mut def = GraphDef::new("Adder")
        .inputs([FuncInput::optional("a", DataType::Int)])
        .output(FuncOutput::new("sum", DataType::Int));
    let boundary = def.body.add(Node::new(NodeKind::GraphInput));
    def.body.add(Node::new(NodeKind::GraphOutput));
    let func = def.body.add_func_node(&probe);

    let target = GraphRef::Local(GraphId::unique());
    let mut view = GraphView::for_graph(&def.body);
    spread(&mut view);

    let theme = Theme::default();
    let run_state = RunState::default();
    let mut harness = UiHarness::new(UVec2::new(1600, 900));
    let mut graph_ui = GraphUI::default();
    let mut scene = Scene::default();

    let draw = |ui: &mut Ui, graph_ui: &mut GraphUI, scene: &mut Scene| {
        let ctx = AppContext {
            theme: &theme,
            library: &library,
            run_state: &run_state,
            status_error: None,
            process_memory: 0,
        };
        let mut intents = Intents::default();
        scene.rebuild(
            ui,
            &library,
            &run_state,
            [GraphProjection {
                target,
                source: SceneSource::Def(&def),
                view: &view,
            }],
        );
        graph_ui.prepass(ui, scene, &library, &mut intents);
        let graph = scene.graph(target).expect("projected");
        Panel::vstack()
            .id_salt("pane")
            .size((Sizing::FILL, Sizing::FILL))
            .show(ui, |ui| {
                graph_ui.draw(ui, &ctx, graph, &mut intents, &mut None);
            });
        intents.drain().collect::<Vec<_>>()
    };

    // Two frames so every node body has recorded and carries a hit-testable
    // rect for the clicks below.
    harness.frame(|ui| {
        draw(ui, &mut graph_ui, &mut scene);
    });
    harness.frame(|ui| {
        draw(ui, &mut graph_ui, &mut scene);
    });

    // The boundary node first, while no popup is up to intercept the press.
    let on_boundary = harness.center_of(node_widget_id(boundary));
    harness.right_click_at(on_boundary);
    let inert = harness.frame_value(|ui| draw(ui, &mut graph_ui, &mut scene));
    assert!(
        inert.is_empty(),
        "a boundary node offers no menu, so its right-click raises nothing: {inert:?}"
    );

    let on_func = harness.center_of(node_widget_id(func));
    harness.right_click_at(on_func);
    let emitted = harness.frame_value(|ui| draw(ui, &mut graph_ui, &mut scene));
    // The helper carries the pane assertion: the selection lands on the pane
    // the menu opened over, not the focused one.
    let intents = scoped_intents(&emitted, target);
    assert!(
        matches!(
            intents[..],
            [Intent::SetSelection { to }] if to.len() == 1 && to.contains(&func),
        ),
        "the right-click selects exactly the node it opened on: {intents:?}"
    );
}

/// A bare drag off an output port onto a compatible input commits the bind.
///
/// Drives the whole `GlyphDrag` lifecycle through real input rather than
/// poking the controller: the latch off the port layer's drag edge, the
/// per-frame snap scan (which is a geometry hit test precisely *because*
/// palantir hides `hovered` from every widget but the drag-capture owner), and
/// the release edge — the layer's `dragging` flag going false, which is the
/// only thing that says a held wire is done.
#[test]
fn a_port_drag_released_over_a_compatible_port_commits_the_binding() {
    use palantir::PointerButton;

    use crate::gui::node::port_row::port_circle_wid;

    let library = one_func_library();
    let probe = library.by_name("probe").expect("just added").clone();

    // Two unwired nodes, so the drag has a producer to leave and a consumer
    // to land on and nothing to trip the cycle check.
    let mut root = Graph::default();
    let producer = root.add_func_node(&probe);
    let consumer = root.add_func_node(&probe);
    let mut view = GraphView::for_graph(&root);
    spread(&mut view);

    let theme = Theme::default();
    let run_state = RunState::default();
    let mut harness = UiHarness::new(UVec2::new(1200, 800));
    let mut graph_ui = GraphUI::default();
    let mut scene = Scene::default();

    let draw = |ui: &mut Ui, graph_ui: &mut GraphUI, scene: &mut Scene| {
        let ctx = AppContext {
            theme: &theme,
            library: &library,
            run_state: &run_state,
            status_error: None,
            process_memory: 0,
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

    // Two frames to record both nodes, so their port circles have widget ids
    // and `CanvasGeometry` measured centers to hit-test against.
    harness.frame(|ui| {
        draw(ui, &mut graph_ui, &mut scene);
    });
    harness.frame(|ui| {
        draw(ui, &mut graph_ui, &mut scene);
    });

    let source = port_circle_wid(PortRef {
        node_id: producer,
        kind: PortKind::Output,
        port_idx: 0,
    });
    let sink = port_circle_wid(PortRef {
        node_id: consumer,
        kind: PortKind::Input,
        port_idx: 0,
    });
    // The snap scan tests the post-transform rect, so aim at that one.
    let drop_at = harness.center_of(sink);

    harness.press_on(source);
    harness.frame(|ui| {
        draw(ui, &mut graph_ui, &mut scene);
    });
    harness.drag_to(drop_at);
    let held = harness.frame_value(|ui| draw(ui, &mut graph_ui, &mut scene));
    assert!(
        held.is_empty(),
        "a wire still held commits nothing: {held:?}"
    );

    harness.release_button(PointerButton::Left);
    let released = harness.frame_value(|ui| draw(ui, &mut graph_ui, &mut scene));

    // The helper carries the pane assertion: a wire commits against the pane
    // holding its start node, never the focused one.
    let released = scoped_intents(&released, GraphRef::Main);
    assert!(
        matches!(
            released[..],
            [Intent::SetInput { input, to: Some(Binding::Bind(src)) }]
                if *input == InputPort::new(consumer, 0)
                    && src.node_id == producer
                    && src.port_idx == 0
        ),
        "the release binds the port it snapped to, and nothing else: {released:?}"
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
            process_memory: 0,
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

    // The helper carries the pane assertion: the spawn is raised against the
    // port's own pane, not whichever one happens to be focused.
    let spawned = scoped_intents(&spawned, GraphRef::Main);
    let adds: Vec<_> = spawned
        .iter()
        .filter(|intent| matches!(intent, Intent::AddNode { .. }))
        .collect();
    assert_eq!(adds.len(), 1, "one preview spawned: {spawned:?}");
    let Intent::AddNode { node_id, node, .. } = adds[0] else {
        unreachable!("filtered to AddNode");
    };
    assert!(
        matches!(node.kind, NodeKind::Func(id) if crate::core::preview::is_preview(id)),
        "the spawned node is a preview"
    );
    assert!(
        spawned.iter().any(|intent| matches!(
            intent,
            Intent::SetInput { input, to: Some(Binding::Bind(src)) }
                if input.node_id == *node_id
                    && src.node_id == producer
                    && src.port_idx == 0
        )),
        "and it is already reading the port the drag came off: {spawned:?}"
    );
}
