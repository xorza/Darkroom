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
use crate::gui::app::commands::AppCommand;
use crate::gui::app::commands::run::RunCommand;
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
    let mut harness = UiHarness::new(UVec2::new(1600, 900));

    // Returns what the frame queued *and* the command it surfaced, so the
    // assertions below read the same two channels the real pipeline drains.
    let mut draw = |ui: &mut Ui| -> FrameOut {
        // Navigation phase: sweep last frame's node responses before the
        // rebuild replaces the projection they were recorded from — the
        // order `Editor::frame` runs, and the one every reader below
        // depends on.
        graph_ui.hits.scan(ui, &scene);
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
        let mut command = None;
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
                            command =
                                command
                                    .take()
                                    .or(graph_ui.draw(ui, &ctx, graph, &mut intents));
                            graph_toolbar::show(ui, &ctx, graph, &graph_ui.geometry, &mut intents);
                        });
                }
            });
        FrameOut {
            queued: intents.drain().collect(),
            command,
        }
    };

    // Two frames: the first fills `CanvasGeometry` and the response cache
    // the second draws against, which is when the node subtrees record in
    // full. A collision on any frame fails the assert below.
    let mut seen_collisions = Vec::new();
    for _ in 0..2 {
        harness.frame_value(&mut draw);
        seen_collisions.extend(harness.collisions());
    }

    // The chip scans behind `emit_chip_command` read one swept hit and then
    // look for the node wearing it. Both halves need checking: the click has
    // to reach the one node whose play chip took it — out of four on screen,
    // in the pane that offers the chip at all — and an otherwise identical
    // frame with no click has to surface nothing. Before the inspector opens,
    // since its panel would sit over the neighbouring node's header.
    harness.click_on(node_wid("play_badge", downstream));
    let ran = harness.frame_value(&mut draw);
    seen_collisions.extend(harness.collisions());
    assert!(
        matches!(ran.command, Some(AppCommand::Run(RunCommand::Node(id))) if id == downstream),
        "the play chip runs the node it sits on: {:?}",
        ran.command
    );
    assert!(
        harness.frame_value(&mut draw).command.is_none(),
        "a frame with no click surfaces no command"
    );

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
    let emitted = harness.frame_value(&mut draw).queued;
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
    let emitted = harness.frame_value(&mut draw).queued;
    let intents = scoped_intents(&emitted, local);
    assert!(
        matches!(
            intents[..],
            [Intent::SetInput { input, to: None }] if *input == InputPort::new(def_func, 0),
        ),
        "the double-click must clear the binding of the port it landed on: {intents:?}"
    );

    // The chip scans behind `emit_chip_command` open on a gate — the widget
    // holding the left button's click — and only then look for the node
    // wearing it. Both halves need checking: the click has to reach the one
    // node whose play chip took it (out of four on screen, in the pane that
    // offers the chip at all), and an otherwise identical frame with no
    // click has to surface nothing.
    // Every click above landed on a chip or a port, and a widget that
    // captures its own press is not an action *outside* the panel — so the
    // transient inspector opened back at the top is still open.
    assert!(
        harness.rect(inspect_panel_wid(upstream)).is_some(),
        "a chip or port click must not dismiss an unpinned inspection panel"
    );
}

/// The two channels one canvas frame writes to: the intent queue and the
/// single `AppCommand` the pane claims.
#[derive(Debug)]
struct FrameOut {
    queued: Vec<Queued>,
    command: Option<AppCommand>,
}

/// A node scrolled off-screen keeps resolvable port centers — and loses them
/// once the *document* stops holding it.
///
/// Two halves of the same cache. A culled node records nothing, so its glyphs'
/// responses are all empty and `rebuild` reconstructs their centers from the
/// persistent intra-node offsets instead of polling each one; that's what keeps
/// a wire anchored to the off-screen end it runs to. Which is also why absence
/// from the scene can't be grounds for eviction — a closed tab looks the same —
/// so eviction is driven from outside, by whether the document still holds the
/// node.
#[test]
fn a_culled_nodes_ports_stay_anchored_until_its_node_leaves_the_document() {
    let library = one_func_library();
    let probe = library.by_name("probe").expect("just added").clone();

    let mut root = Graph::default();
    let stays = root.add_func_node(&probe);
    let leaves = root.add_func_node(&probe);
    root.set_input_binding(InputPort::new(leaves, 0), Binding::bind(stays, 0));
    let mut view = GraphView::for_graph(&root);
    spread(&mut view);

    let theme = Theme::default();
    let run_state = RunState::default();
    let mut harness = UiHarness::new(UVec2::new(1200, 800));
    let mut graph_ui = GraphUI::default();
    let mut scene = Scene::default();

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
                graph_ui.draw(ui, &ctx, graph, &mut intents);
            });
    };
    let frame = |harness: &mut UiHarness, graph_ui: &mut GraphUI, scene: &mut Scene, view: &_| {
        harness.frame(|ui| draw(ui, graph_ui, scene, view));
    };

    // Both on screen and recorded, so every glyph has a fresh offset cached.
    frame(&mut harness, &mut graph_ui, &mut scene, &view);
    frame(&mut harness, &mut graph_ui, &mut scene, &view);
    let out_port = PortRef {
        node_id: leaves,
        kind: PortKind::Output,
        port_idx: 0,
    };
    let anchored = graph_ui
        .geometry
        .ports
        .center(out_port)
        .expect("a recorded port resolves its center");

    // Scroll it far past the viewport. Two frames: the first still reads the
    // on-screen record, the second is the culled one that has to reconstruct.
    let before = view.item_placements[&leaves];
    let shift = Vec2::new(6000.0, 4000.0);
    view.item_placements[&leaves] = before + shift;
    frame(&mut harness, &mut graph_ui, &mut scene, &view);
    frame(&mut harness, &mut graph_ui, &mut scene, &view);

    let culled = graph_ui
        .geometry
        .ports
        .center(out_port)
        .expect("a culled port still resolves, off the cached offset");
    // And it tracks the move rather than sticking where it last painted: the
    // centre travelled exactly as far as the node did.
    assert!(
        (culled - anchored - shift).length() < 0.01,
        "expected the centre to move by {shift:?}, got {:?}",
        culled - anchored,
    );

    // The node the document keeps holds its cached size; the other one is
    // still cached too, because being off-screen is not being deleted.
    let live = scene.graph(GraphRef::Main).expect("projected");
    for id in [stays, leaves] {
        assert!(
            graph_ui
                .geometry
                .node_world_rect(live.node(id).expect("in scene"))
                .is_some(),
            "an off-screen node is not a deleted one",
        );
    }

    // Now say the document dropped it. Its entries go; its neighbour's stay.
    graph_ui.retain_nodes(|id| id == stays);
    assert!(
        graph_ui
            .geometry
            .node_world_rect(live.node(leaves).expect("in scene"))
            .is_none(),
        "a node the document stopped holding releases its cached size",
    );
    assert!(
        graph_ui
            .geometry
            .node_world_rect(live.node(stays).expect("in scene"))
            .is_some(),
        "and its neighbour keeps its own",
    );
    // The port offsets went with it, so the next culled rebuild has nothing
    // left to reconstruct from.
    frame(&mut harness, &mut graph_ui, &mut scene, &view);
    assert_eq!(
        graph_ui.geometry.ports.center(out_port),
        None,
        "an evicted node's ports stop resolving",
    );
}

/// The new-node palette keeps its search field and its results inside the
/// height cap, at whatever height the field actually measures.
///
/// The results `Scroll` has to carry an explicit cap of its own — a stack
/// hands every non-`Fill` child its full main extent, so a `Hug` scroll offered
/// the popup's cap takes all of it and shoves the search row past the bottom.
/// That cap used to be `cap - 48.0`, a hand-tuned stand-in for a height nothing
/// read; restyling the field's text or the menu's padding would have gone on
/// subtracting 48. Both cases below run the same assertion, the second with the
/// field's text scaled well past the old constant.
#[test]
fn the_palette_sizes_its_results_area_from_the_search_row_it_actually_has() {
    use palantir::{Rect, Spacing};

    use crate::gui::canvas::new_node_ui::{results_wid, search_field_wid};

    /// Records one palette open, with `restyle` applied to the live
    /// `Ui::theme` first, and returns the two rects the height cap divides
    /// between plus the cap itself.
    fn open_palette(theme: &Theme, restyle: impl Fn(&mut palantir::Theme)) -> (Rect, Rect, f32) {
        // Enough rows in one category to overflow any sane cap, so the scroll
        // is genuinely competing for the popup's height.
        let mut library = Library::default();
        for i in 0..60 {
            library.add(testing::with_stub_lambda(
                Func::new(FuncId::unique(), format!("func{i:02}")).category("Bulk"),
            ));
        }
        let root = Graph::default();
        let view = GraphView::for_graph(&root);
        let run_state = RunState::default();
        let mut harness = UiHarness::with_text(UVec2::new(1200, 900));
        restyle(&mut harness.ui().theme);
        let mut graph_ui = GraphUI::default();
        let mut scene = Scene::default();

        let draw = |ui: &mut Ui, graph_ui: &mut GraphUI, scene: &mut Scene| {
            let ctx = AppContext {
                theme,
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
                    graph_ui.draw(ui, &ctx, graph, &mut intents);
                });
        };

        harness.frame(|ui| draw(ui, &mut graph_ui, &mut scene));
        // Right-click on empty canvas opens the palette; give it two frames so
        // the search field has measured and the cap reads its real height.
        harness.right_click_at(Vec2::new(500.0, 400.0));
        harness.frame(|ui| draw(ui, &mut graph_ui, &mut scene));
        harness.frame(|ui| draw(ui, &mut graph_ui, &mut scene));

        // The same cap `NewNodeUi::apply` resolves, against this harness's
        // 900 px surface.
        let cap = theme
            .new_node_popup_max_height
            .clamp(120.0, (900.0f32 - 16.0).max(120.0));
        (
            harness.rect(search_field_wid()).expect("field recorded"),
            harness.rect(results_wid()).expect("results recorded"),
            cap,
        )
    }

    /// The field sits above the results, and the two plus the popup's own
    /// chrome fit the cap — with no more slack than the chrome accounts for,
    /// which is what catches an allowance that over-subtracts as well as one
    /// that under-subtracts.
    /// The field sits above the results, and the two plus the popup's own
    /// chrome fit the cap with no slack the chrome doesn't account for —
    /// which catches an allowance that over-subtracts as well as one that
    /// under-subtracts.
    fn assert_fits(rects: (Rect, Rect, f32), menu: &palantir::ContextMenuTheme, label: &str) {
        let (field, results, cap) = rects;
        assert!(
            field.max().y <= results.min.y + 0.5,
            "{label}: the results overlap the search field ({field:?} vs {results:?})",
        );
        let used = menu.padding.vert() + field.size.h + menu.gap + results.size.h;
        assert!(
            used <= cap + 0.5,
            "{label}: field {} + results {} + chrome overflows the {cap} cap (used {used})",
            field.size.h,
            results.size.h,
        );
        // With 60 rows the results always want more room than they get, so the
        // area has to claim everything the chrome didn't — an allowance that
        // over-subtracts would leave a visible dead band here.
        assert!(
            used >= cap - 1.0,
            "{label}: {} px of the {cap} cap went unused (used {used})",
            cap - used,
        );
    }

    let theme = Theme::default();
    let plain = palantir::Theme::default();
    let small = open_palette(&theme, |_| {});
    assert_fits(small, &plain.context_menu, "default theme");

    // Now restyle both terms the retired 48 px constant could never have
    // tracked: a much bigger search field, and a much roomier popup. The
    // results area has to give up exactly what they took.
    let mut restyled = palantir::Theme::default();
    restyled.text.font_size_px *= 3.0;
    restyled.context_menu.padding = Spacing::all(24.0);
    restyled.context_menu.gap = 12.0;
    let big = open_palette(&theme, |t| {
        t.text.font_size_px *= 3.0;
        t.context_menu.padding = Spacing::all(24.0);
        t.context_menu.gap = 12.0;
    });
    assert_fits(big, &restyled.context_menu, "bigger field and popup");

    assert!(
        big.0.size.h > small.0.size.h,
        "the field really did grow: {} → {}",
        small.0.size.h,
        big.0.size.h,
    );
    assert!(
        big.1.size.h < small.1.size.h,
        "and the results gave up the difference: {} → {}",
        small.1.size.h,
        big.1.size.h,
    );
}

/// Escape cancels a rubber band: no `SetSelection`, and the next band
/// starts from a clean base.
///
/// The cancel is resolved once per frame by the canvas and handed to each
/// controller, so this also covers the wiring — a band that kept running
/// after Esc would commit on release. The second band is the half that
/// would break silently: `base` lives inside `RubberBand` now, and a
/// cancel that left it behind would union the abandoned drag's selection
/// into the next one.
#[test]
fn escape_cancels_a_rubber_band_and_leaves_no_residue() {
    use palantir::Key;

    let library = one_func_library();
    let probe = library.by_name("probe").expect("just added").clone();
    let mut root = Graph::default();
    let a = root.add_func_node(&probe);
    let b = root.add_func_node(&probe);
    let mut view = GraphView::for_graph(&root);
    // Placed by id, not by `spread`: this case cares *which* node the
    // second band reaches, and `spread` assigns by map iteration order.
    view.item_placements.insert(a, Vec2::new(40.0, 40.0));
    view.item_placements.insert(b, Vec2::new(400.0, 40.0));

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
        graph_ui.hits.scan(ui, scene);
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
                graph_ui.draw(ui, &ctx, graph, &mut intents);
            });
        intents.drain().collect::<Vec<_>>()
    };

    for _ in 0..2 {
        harness.frame(|ui| {
            draw(ui, &mut graph_ui, &mut scene);
        });
    }

    // Sweep bare canvas across both nodes, then cancel mid-drag.
    let empty = Vec2::new(20.0, 400.0);
    harness.press_at(empty);
    harness.frame(|ui| {
        draw(ui, &mut graph_ui, &mut scene);
    });
    harness.drag_to(Vec2::new(700.0, 60.0));
    harness.frame(|ui| {
        draw(ui, &mut graph_ui, &mut scene);
    });
    harness.key(Key::Escape);
    let cancelled = harness.frame_value(|ui| draw(ui, &mut graph_ui, &mut scene));
    assert!(
        cancelled.is_empty(),
        "a cancelled band commits nothing: {cancelled:?}"
    );
    harness.release_button(palantir::PointerButton::Left);
    let after = harness.frame_value(|ui| draw(ui, &mut graph_ui, &mut scene));
    assert!(
        after.is_empty(),
        "and the release of a cancelled band commits nothing either: {after:?}"
    );

    // A fresh band over `a` alone must select exactly `a` — if the
    // cancelled drag's `base` survived, `b` would ride along. `spread`
    // puts the nodes at x = 40 and x = 260, so a sweep stopping at x = 150
    // reaches the first and not the second; both bands start from the same
    // empty patch well below them.
    harness.press_at(empty);
    harness.frame(|ui| {
        draw(ui, &mut graph_ui, &mut scene);
    });
    harness.drag_to(Vec2::new(150.0, 100.0));
    harness.frame(|ui| {
        draw(ui, &mut graph_ui, &mut scene);
    });
    harness.release_button(palantir::PointerButton::Left);
    let emitted = harness.frame_value(|ui| draw(ui, &mut graph_ui, &mut scene));
    let intents = scoped_intents(&emitted, GraphRef::Main);
    assert!(
        matches!(
            intents[..],
            [Intent::SetSelection { to }] if to.len() == 1 && to.contains(&a),
        ),
        "the next band selects only what it swept, with no residue from the \
         cancelled one: {intents:?} (b = {b:?})"
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
                graph_ui.draw(ui, &ctx, graph, &mut intents);
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
        // Navigation phase first — see the two-pane test for why the sweep
        // reads the pre-rebuild scene.
        graph_ui.hits.scan(ui, scene);
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
                graph_ui.draw(ui, &ctx, graph, &mut intents);
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

/// A drag on a node body moves that node, by the pointer's travel.
///
/// The drag *latch* is the one thing `CanvasHits` resolves for the record
/// rather than for an input pass: `NodeUI::draw_one` no longer polls the
/// node's own handles, it reads the handle the sweep found. So this drives
/// a real press-and-travel through the harness — nothing else in the suite
/// latches a body drag, and a latch that silently stopped firing would
/// leave every node unmovable with the whole rest of the canvas green.
#[test]
fn a_body_drag_moves_the_node_by_the_pointers_travel() {
    let library = one_func_library();
    let probe = library.by_name("probe").expect("just added").clone();

    let mut root = Graph::default();
    let dragged = root.add_func_node(&probe);
    let bystander = root.add_func_node(&probe);
    let mut view = GraphView::for_graph(&root);
    spread(&mut view);
    let start = view.item_placements[&dragged];

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
        graph_ui.hits.scan(ui, scene);
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
                graph_ui.draw(ui, &ctx, graph, &mut intents);
            });
        intents.drain().collect::<Vec<_>>()
    };

    for _ in 0..2 {
        harness.frame(|ui| {
            draw(ui, &mut graph_ui, &mut scene);
        });
    }

    // Press the body, then travel past the drag threshold. The sweep sees
    // the latch on the frame after the travel; the record consumes it.
    let grab = harness.center_of(node_widget_id(dragged));
    harness.press_at(grab);
    harness.frame(|ui| {
        draw(ui, &mut graph_ui, &mut scene);
    });
    let travel = Vec2::new(37.0, -21.0);
    harness.drag_to(grab + travel);
    harness.frame(|ui| {
        draw(ui, &mut graph_ui, &mut scene);
    });

    // Next frame, `NodeUI::prepass` advances the anchor the record latched.
    let emitted = harness.frame_value(|ui| draw(ui, &mut graph_ui, &mut scene));
    let intents = scoped_intents(&emitted, GraphRef::Main);
    let moves = intents
        .iter()
        .find_map(|intent| match intent {
            Intent::MoveSelection { grabbed, moves } => Some((*grabbed, moves)),
            _ => None,
        })
        .unwrap_or_else(|| panic!("a body drag must emit a MoveSelection: {intents:?}"));
    assert_eq!(moves.0, dragged, "the grabbed node is the one pressed");
    // Target = press-frame position + cumulative travel, so the node lands
    // exactly where the pointer took it — and the untouched node stays out
    // of the batch, since the grab selected only the node under it.
    assert_eq!(
        moves.1.as_slice(),
        &[(dragged, start + travel)],
        "the drag moves only the grabbed node, to its start plus the travel"
    );
    assert!(
        !moves.1.iter().any(|(id, _)| *id == bystander),
        "an unselected neighbour is not dragged along"
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
                graph_ui.draw(ui, &ctx, graph, &mut intents);
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
                graph_ui.draw(ui, &ctx, graph, &mut intents);
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

/// The palette reads the open graph's own definitions **once per open**, so
/// a definition added between two opens has to show up in the second one.
///
/// Those rows have to be owned (`LocalDefRow` copies each name out of the
/// scene's interned handles), and rebuilding them on every frame the
/// palette was up was pure waste — nothing can add a definition while the
/// popup holds the pointer. Caching them at open is what makes that read
/// cheap, and serving a *closed* palette's stale list on the next open is
/// the one way that can go wrong.
#[test]
fn the_palette_re_reads_the_graphs_definitions_on_every_open() {
    use palantir::Key;

    use crate::gui::canvas::new_node_ui::search_field_wid;

    let library = Library::default();
    let theme = Theme::default();
    let run_state = RunState::default();
    let mut root = Graph::default();
    root.insert_graph(GraphId::unique(), GraphDef::new("First").category("Local"));

    let mut harness = UiHarness::with_text(UVec2::new(1200, 900));
    let mut graph_ui = GraphUI::default();
    let mut scene = Scene::default();

    let draw = |ui: &mut Ui, graph_ui: &mut GraphUI, scene: &mut Scene, root: &Graph| {
        let ctx = AppContext {
            theme: &theme,
            library: &library,
            run_state: &run_state,
            status_error: None,
            process_memory: 0,
        };
        let view = GraphView::for_graph(root);
        let mut intents = Intents::default();
        scene.rebuild(
            ui,
            &library,
            &run_state,
            [GraphProjection {
                target: GraphRef::Main,
                source: SceneSource::Entry(root),
                view: &view,
            }],
        );
        graph_ui.prepass(ui, scene, &library, &mut intents);
        let graph = scene.graph(GraphRef::Main).expect("projected");
        Panel::vstack()
            .id_salt("pane")
            .size((Sizing::FILL, Sizing::FILL))
            .show(ui, |ui| {
                graph_ui.draw(ui, &ctx, graph, &mut intents);
            });
    };

    harness.frame(|ui| draw(ui, &mut graph_ui, &mut scene, &root));
    assert!(
        graph_ui.gestures.new_node_ui.cached_local_defs().is_empty(),
        "a palette that never opened caches nothing",
    );

    // First open: the one definition the graph holds.
    harness.right_click_at(Vec2::new(500.0, 400.0));
    harness.frame(|ui| draw(ui, &mut graph_ui, &mut scene, &root));
    assert_eq!(
        graph_ui.gestures.new_node_ui.cached_local_defs(),
        ["First"],
        "the open reads the graph's definitions",
    );

    // One Esc dismisses, which clears the anchor — so the next right-click
    // is a fresh open rather than a re-anchor. One, not two: the search
    // field yields `ESCAPE` (`TextEdit::escape_falls_through`) precisely so
    // it can't blur itself and strand the palette open around a box the
    // user can no longer type in.
    harness.key(Key::Escape);
    harness.frame(|ui| draw(ui, &mut graph_ui, &mut scene, &root));
    assert!(
        harness.rect(search_field_wid()).is_none(),
        "one Esc dismissed the palette",
    );

    // The document gains a definition while the palette is down.
    root.insert_graph(GraphId::unique(), GraphDef::new("Second").category("Local"));
    harness.frame(|ui| draw(ui, &mut graph_ui, &mut scene, &root));
    harness.advance_past_double_click(|ui| draw(ui, &mut graph_ui, &mut scene, &root));

    // Second open: both, not the first open's list.
    harness.right_click_at(Vec2::new(500.0, 400.0));
    harness.frame(|ui| draw(ui, &mut graph_ui, &mut scene, &root));
    let mut cached = graph_ui.gestures.new_node_ui.cached_local_defs();
    cached.sort_unstable();
    assert_eq!(
        cached,
        ["First", "Second"],
        "every open re-reads, so a definition added since the last one lists",
    );
}
