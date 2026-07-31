use glam::{UVec2, Vec2};
use palantir::internals::UiHarness;
use palantir::{Configure, Panel, Sizing, Ui};
use scenarium::{
    Binding, DataType, Func, FuncId, FuncInput, FuncOutput, InputPort, Library, NodeKind,
    OutputType, OutputTypes, testing,
};

use crate::core::document::{Document, GraphView, PortKind, PortRef};
use crate::core::edit::intent::sink::{Intents, Queued};
use crate::core::edit::intent::types::GraphIntent;
use crate::gui::app::AppContext;
use crate::gui::canvas::GraphUI;
use crate::gui::graph_scope::GraphScope;
use crate::gui::node::node_widget_id;
use crate::gui::node::port_row::port_circle_wid;
use crate::gui::run_state::RunState;
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

/// [`one_func_library`] plus a passthrough: one input and one wildcard output
/// mirroring it, so the projected type of its output is whatever the *graph*
/// wires in rather than anything its declaration states.
fn wildcard_library() -> Library {
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

/// The scope a test reads a canvas back through, over the same document the
/// canvas drew.
///
/// `output_types` is scratch the composition fills — a fresh one per call is
/// correct, since several tests below edit their document between frames and
/// the scope resolves against whichever one it is handed.
fn scope<'a>(
    doc: &'a Document,
    library: &'a Library,
    run_state: &'a RunState,
    output_types: &'a mut OutputTypes,
) -> GraphScope<'a> {
    GraphScope::for_document(doc, library, run_state, output_types)
        .expect("the fixture's document shows the graph")
}

/// Spread the placements so no node lands off-viewport and gets culled —
/// `GraphView::for_graph` seeds every item at the origin.
fn spread(view: &mut GraphView) {
    for (i, pos) in view.item_placements.values_mut().enumerate() {
        *pos = Vec2::new(40.0 + i as f32 * 220.0, 40.0);
    }
}

/// The graph intent behind each queued item. The assertion belongs to every
/// canvas test: none of these widgets can reach the dock.
fn graph_intents(queued: &[Queued]) -> Vec<&GraphIntent> {
    queued
        .iter()
        .map(|item| match item {
            Queued::Graph(intent) => intent,
            Queued::Dock(intent) => panic!("a canvas widget raised {intent:?}"),
        })
        .collect()
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

    let mut doc = Document::default();
    let stays = doc.graph.add_func_node(&probe);
    let leaves = doc.graph.add_func_node(&probe);
    doc.graph
        .set_input_binding(InputPort::new(leaves, 0), Binding::bind(stays, 0));
    doc.main_view = GraphView::for_graph(&doc.graph);
    spread(&mut doc.main_view);

    let theme = Theme::default();
    let run_state = RunState::default();
    let mut harness = UiHarness::new(UVec2::new(1200, 800));
    let mut graph_ui = GraphUI::default();

    let draw = |ui: &mut Ui, graph_ui: &mut GraphUI, doc: &Document| {
        let ctx = AppContext {
            theme: &theme,
            library: &library,
            run_state: &run_state,
            status_error: None,
            process_memory: 0,
        };
        let mut intents = Intents::default();
        let mut types = OutputTypes::default();
        let graph_scope = scope(doc, &library, &run_state, &mut types);
        graph_ui.prepass(ui, graph_scope, &mut intents);
        Panel::vstack()
            .id_salt("pane")
            .size((Sizing::FILL, Sizing::FILL))
            .show(ui, |ui| {
                graph_ui.draw(ui, &ctx, graph_scope, &mut intents);
            });
    };
    let frame = |harness: &mut UiHarness, graph_ui: &mut GraphUI, doc: &_| {
        harness.frame(|ui| draw(ui, graph_ui, doc));
    };

    // Both on screen and recorded, so every glyph has a fresh offset cached.
    frame(&mut harness, &mut graph_ui, &doc);
    frame(&mut harness, &mut graph_ui, &doc);
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
    let before = doc.main_view.item_placements[&leaves];
    let shift = Vec2::new(6000.0, 4000.0);
    doc.main_view.item_placements[&leaves] = before + shift;
    frame(&mut harness, &mut graph_ui, &doc);
    frame(&mut harness, &mut graph_ui, &doc);

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
    let mut live_types = OutputTypes::default();
    let live = scope(&doc, &library, &run_state, &mut live_types);
    for id in [stays, leaves] {
        assert!(
            graph_ui
                .geometry
                .node_world_rect(live.node(id).expect("in the graph"))
                .is_some(),
            "an off-screen node is not a deleted one",
        );
    }

    // Now say the document dropped it. Its entries go; its neighbour's stay.
    graph_ui.retain_nodes(|id| id == stays);
    assert!(
        graph_ui
            .geometry
            .node_world_rect(live.node(leaves).expect("in the graph"))
            .is_none(),
        "a node the document stopped holding releases its cached size",
    );
    assert!(
        graph_ui
            .geometry
            .node_world_rect(live.node(stays).expect("in the graph"))
            .is_some(),
        "and its neighbour keeps its own",
    );
    // The port offsets went with it, so the next culled rebuild has nothing
    // left to reconstruct from.
    frame(&mut harness, &mut graph_ui, &doc);
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
        let doc = Document::default();
        let run_state = RunState::default();
        let mut harness = UiHarness::with_text(UVec2::new(1200, 900));
        restyle(&mut harness.ui().theme);
        let mut graph_ui = GraphUI::default();

        let draw = |ui: &mut Ui, graph_ui: &mut GraphUI| {
            let ctx = AppContext {
                theme,
                library: &library,
                run_state: &run_state,
                status_error: None,
                process_memory: 0,
            };
            let mut intents = Intents::default();
            let mut types = OutputTypes::default();
            let graph_scope = scope(&doc, &library, &run_state, &mut types);
            graph_ui.prepass(ui, graph_scope, &mut intents);
            Panel::vstack()
                .id_salt("pane")
                .size((Sizing::FILL, Sizing::FILL))
                .show(ui, |ui| {
                    graph_ui.draw(ui, &ctx, graph_scope, &mut intents);
                });
        };

        harness.frame(|ui| draw(ui, &mut graph_ui));
        // Right-click on empty canvas opens the palette; give it two frames so
        // the search field has measured and the cap reads its real height.
        harness.right_click_at(Vec2::new(500.0, 400.0));
        harness.frame(|ui| draw(ui, &mut graph_ui));
        harness.frame(|ui| draw(ui, &mut graph_ui));

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
    let mut doc = Document::default();
    let a = doc.graph.add_func_node(&probe);
    let b = doc.graph.add_func_node(&probe);
    doc.main_view = GraphView::for_graph(&doc.graph);
    // Placed by id, not by `spread`: this case cares *which* node the
    // second band reaches, and `spread` assigns by map iteration order.
    doc.main_view
        .item_placements
        .insert(a, Vec2::new(40.0, 40.0));
    doc.main_view
        .item_placements
        .insert(b, Vec2::new(400.0, 40.0));

    let theme = Theme::default();
    let run_state = RunState::default();
    let mut harness = UiHarness::new(UVec2::new(1200, 800));
    let mut graph_ui = GraphUI::default();

    let draw = |ui: &mut Ui, graph_ui: &mut GraphUI| {
        let ctx = AppContext {
            theme: &theme,
            library: &library,
            run_state: &run_state,
            status_error: None,
            process_memory: 0,
        };
        let mut intents = Intents::default();
        let mut types = OutputTypes::default();
        let graph_scope = scope(&doc, &library, &run_state, &mut types);
        graph_ui.scan_hits(ui, Some(graph_scope));
        graph_ui.prepass(ui, graph_scope, &mut intents);
        Panel::vstack()
            .id_salt("pane")
            .size((Sizing::FILL, Sizing::FILL))
            .show(ui, |ui| {
                graph_ui.draw(ui, &ctx, graph_scope, &mut intents);
            });
        intents.drain().collect::<Vec<_>>()
    };

    for _ in 0..2 {
        harness.frame(|ui| {
            draw(ui, &mut graph_ui);
        });
    }

    // Sweep bare canvas across both nodes, then cancel mid-drag.
    let empty = Vec2::new(20.0, 400.0);
    harness.press_at(empty);
    harness.frame(|ui| {
        draw(ui, &mut graph_ui);
    });
    harness.drag_to(Vec2::new(700.0, 60.0));
    harness.frame(|ui| {
        draw(ui, &mut graph_ui);
    });
    harness.key(Key::Escape);
    let cancelled = harness.frame_value(|ui| draw(ui, &mut graph_ui));
    assert!(
        cancelled.is_empty(),
        "a cancelled band commits nothing: {cancelled:?}"
    );
    harness.release_button(palantir::PointerButton::Left);
    let after = harness.frame_value(|ui| draw(ui, &mut graph_ui));
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
        draw(ui, &mut graph_ui);
    });
    harness.drag_to(Vec2::new(150.0, 100.0));
    harness.frame(|ui| {
        draw(ui, &mut graph_ui);
    });
    harness.release_button(palantir::PointerButton::Left);
    let emitted = harness.frame_value(|ui| draw(ui, &mut graph_ui));
    let intents = graph_intents(&emitted);
    assert!(
        matches!(
            intents[..],
            [GraphIntent::SetSelection { to }] if to.len() == 1 && to.contains(&a),
        ),
        "the next band selects only what it swept, with no residue from the \
         cancelled one: {intents:?} (b = {b:?})"
    );
}

/// The breaker cuts a node where the *document* says it is, not where it last
/// painted.
///
/// Both rects are one frame apart whenever something moves a node out from
/// under a live gesture — an undo, say — because `SceneNode::pos`
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

    let mut doc = Document::default();
    let node = doc.graph.add_func_node(&probe);
    doc.main_view = GraphView::for_graph(&doc.graph);
    spread(&mut doc.main_view);

    let theme = Theme::default();
    let run_state = RunState::default();
    let mut harness = UiHarness::new(UVec2::new(1200, 800));
    let mut graph_ui = GraphUI::default();

    // `doc` is a parameter, not a capture, so it can be edited between frames
    // the way an undo would edit the document under a running gesture.
    let draw = |ui: &mut Ui, graph_ui: &mut GraphUI, doc: &Document| {
        let ctx = AppContext {
            theme: &theme,
            library: &library,
            run_state: &run_state,
            status_error: None,
            process_memory: 0,
        };
        let mut intents = Intents::default();
        let mut types = OutputTypes::default();
        let graph_scope = scope(doc, &library, &run_state, &mut types);
        graph_ui.prepass(ui, graph_scope, &mut intents);
        Panel::vstack()
            .id_salt("pane")
            .size((Sizing::FILL, Sizing::FILL))
            .show(ui, |ui| {
                graph_ui.draw(ui, &ctx, graph_scope, &mut intents);
            });
        intents.drain().collect::<Vec<_>>()
    };

    harness.frame(|ui| {
        draw(ui, &mut graph_ui, &doc);
    });
    harness.frame(|ui| {
        draw(ui, &mut graph_ui, &doc);
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
    let scribbling = harness.frame_value(|ui| draw(ui, &mut graph_ui, &doc));
    assert!(
        scribbling.is_empty(),
        "a scribble in flight severs nothing until release: {scribbling:?}"
    );

    // Now move the node onto the scribble, centred on its far end. This frame
    // the document says the node is here while its arranged rect still says it
    // is back there — the divergence the probe has to resolve the new way.
    doc.main_view.item_placements[&node] = SCRIBBLE_TO - Vec2::new(body.size.w, body.size.h) * 0.5;
    harness.frame(|ui| {
        draw(ui, &mut graph_ui, &doc);
    });

    harness.release_button(PointerButton::Right);
    let released = harness.frame_value(|ui| draw(ui, &mut graph_ui, &doc));
    // The helper carries the pane assertion: a cut commits against the pane
    // the scribble ran on.
    let released = graph_intents(&released);
    assert!(
        matches!(released[..], [GraphIntent::RemoveNode { node_id }] if *node_id == node),
        "the release cuts the node the scribble now crosses: {released:?}"
    );
}

/// A node-body right-click selects the node it landed on before the menu
/// opens, so whatever the user picks next acts on a coherent set.
///
/// It comes out of the closure the shared trigger scan takes — "which of this
/// node's widgets opens the menu" — so it's checked through a real click
/// rather than by calling it.
#[test]
fn a_node_body_right_click_selects_the_node_it_landed_on() {
    let library = one_func_library();
    let probe = library.by_name("probe").expect("just added").clone();

    // Two nodes, so the assertion that exactly one ends up selected has
    // something to exclude.
    let mut doc = Document::default();
    let func = doc.graph.add_func_node(&probe);
    doc.graph.add_func_node(&probe);
    doc.main_view = GraphView::for_graph(&doc.graph);
    spread(&mut doc.main_view);

    let theme = Theme::default();
    let run_state = RunState::default();
    let mut harness = UiHarness::new(UVec2::new(1600, 900));
    let mut graph_ui = GraphUI::default();

    let draw = |ui: &mut Ui, graph_ui: &mut GraphUI| {
        let ctx = AppContext {
            theme: &theme,
            library: &library,
            run_state: &run_state,
            status_error: None,
            process_memory: 0,
        };
        let mut intents = Intents::default();
        // Navigation phase first — the sweep runs before the tab set settles.
        let mut types = OutputTypes::default();
        let graph_scope = scope(&doc, &library, &run_state, &mut types);
        graph_ui.scan_hits(ui, Some(graph_scope));
        graph_ui.prepass(ui, graph_scope, &mut intents);
        Panel::vstack()
            .id_salt("pane")
            .size((Sizing::FILL, Sizing::FILL))
            .show(ui, |ui| {
                graph_ui.draw(ui, &ctx, graph_scope, &mut intents);
            });
        intents.drain().collect::<Vec<_>>()
    };

    // Two frames so every node body has recorded and carries a hit-testable
    // rect for the clicks below.
    harness.frame(|ui| {
        draw(ui, &mut graph_ui);
    });
    harness.frame(|ui| {
        draw(ui, &mut graph_ui);
    });

    let on_func = harness.center_of(node_widget_id(func));
    harness.right_click_at(on_func);
    let emitted = harness.frame_value(|ui| draw(ui, &mut graph_ui));
    let intents = graph_intents(&emitted);
    assert!(
        matches!(
            intents[..],
            [GraphIntent::SetSelection { to }] if to.len() == 1 && to.contains(&func),
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

    let mut doc = Document::default();
    let dragged = doc.graph.add_func_node(&probe);
    let bystander = doc.graph.add_func_node(&probe);
    doc.main_view = GraphView::for_graph(&doc.graph);
    spread(&mut doc.main_view);
    let start = doc.main_view.item_placements[&dragged];

    let theme = Theme::default();
    let run_state = RunState::default();
    let mut harness = UiHarness::new(UVec2::new(1200, 800));
    let mut graph_ui = GraphUI::default();

    let draw = |ui: &mut Ui, graph_ui: &mut GraphUI| {
        let ctx = AppContext {
            theme: &theme,
            library: &library,
            run_state: &run_state,
            status_error: None,
            process_memory: 0,
        };
        let mut intents = Intents::default();
        let mut types = OutputTypes::default();
        let graph_scope = scope(&doc, &library, &run_state, &mut types);
        graph_ui.scan_hits(ui, Some(graph_scope));
        graph_ui.prepass(ui, graph_scope, &mut intents);
        Panel::vstack()
            .id_salt("pane")
            .size((Sizing::FILL, Sizing::FILL))
            .show(ui, |ui| {
                graph_ui.draw(ui, &ctx, graph_scope, &mut intents);
            });
        intents.drain().collect::<Vec<_>>()
    };

    for _ in 0..2 {
        harness.frame(|ui| {
            draw(ui, &mut graph_ui);
        });
    }

    // Press the body, then travel past the drag threshold. The sweep sees
    // the latch on the frame after the travel; the record consumes it.
    let grab = harness.center_of(node_widget_id(dragged));
    harness.press_at(grab);
    harness.frame(|ui| {
        draw(ui, &mut graph_ui);
    });
    let travel = Vec2::new(37.0, -21.0);
    harness.drag_to(grab + travel);
    harness.frame(|ui| {
        draw(ui, &mut graph_ui);
    });

    // Next frame, `NodeUI::prepass` advances the anchor the record latched.
    let emitted = harness.frame_value(|ui| draw(ui, &mut graph_ui));
    let intents = graph_intents(&emitted);
    let moves = intents
        .iter()
        .find_map(|intent| match intent {
            GraphIntent::MoveSelection { grabbed, moves } => Some((*grabbed, moves)),
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

    let library = one_func_library();
    let probe = library.by_name("probe").expect("just added").clone();

    // Two unwired nodes, so the drag has a producer to leave and a consumer
    // to land on and nothing to trip the cycle check.
    let mut doc = Document::default();
    let producer = doc.graph.add_func_node(&probe);
    let consumer = doc.graph.add_func_node(&probe);
    doc.main_view = GraphView::for_graph(&doc.graph);
    spread(&mut doc.main_view);

    let theme = Theme::default();
    let run_state = RunState::default();
    let mut harness = UiHarness::new(UVec2::new(1200, 800));
    let mut graph_ui = GraphUI::default();

    // `doc` is a parameter, not a capture, so the graph can gain the edge the
    // first drag commits before the second one runs against it.
    let draw = |ui: &mut Ui, graph_ui: &mut GraphUI, doc: &Document| {
        let ctx = AppContext {
            theme: &theme,
            library: &library,
            run_state: &run_state,
            status_error: None,
            process_memory: 0,
        };
        let mut intents = Intents::default();
        let mut types = OutputTypes::default();
        let graph_scope = scope(doc, &library, &run_state, &mut types);
        graph_ui.prepass(ui, graph_scope, &mut intents);
        Panel::vstack()
            .id_salt("pane")
            .size((Sizing::FILL, Sizing::FILL))
            .show(ui, |ui| {
                graph_ui.draw(ui, &ctx, graph_scope, &mut intents);
            });
        intents.drain().collect::<Vec<_>>()
    };

    // Two frames to record both nodes, so their port circles have widget ids
    // and `CanvasGeometry` measured centers to hit-test against.
    harness.frame(|ui| {
        draw(ui, &mut graph_ui, &doc);
    });
    harness.frame(|ui| {
        draw(ui, &mut graph_ui, &doc);
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
        draw(ui, &mut graph_ui, &doc);
    });
    harness.drag_to(drop_at);
    let held = harness.frame_value(|ui| draw(ui, &mut graph_ui, &doc));
    assert!(
        held.is_empty(),
        "a wire still held commits nothing: {held:?}"
    );

    harness.release_button(PointerButton::Left);
    let released = harness.frame_value(|ui| draw(ui, &mut graph_ui, &doc));

    // The helper carries the pane assertion: a wire commits against the pane
    // holding its start node, never the focused one.
    let released = graph_intents(&released);
    assert!(
        matches!(
            released[..],
            [GraphIntent::SetInput { input, to: Some(Binding::Bind(src)) }]
                if *input == InputPort::new(consumer, 0)
                    && src.node_id == producer
                    && src.port_idx == 0
        ),
        "the release binds the port it snapped to, and nothing else: {released:?}"
    );

    // Now the same gesture backwards, with the document holding the edge that
    // release just described: consumer.out0 → producer.in0 would close the
    // loop, so the wire must not snap and the release must commit nothing.
    // The filter answers that off `Document::graph_for` — a prepass handed the
    // wrong graph would go on snapping cycles with every other assertion here
    // still green.
    doc.graph
        .set_input_binding(InputPort::new(consumer, 0), Binding::bind(producer, 0));
    let back_source = port_circle_wid(PortRef {
        node_id: consumer,
        kind: PortKind::Output,
        port_idx: 0,
    });
    let back_sink = port_circle_wid(PortRef {
        node_id: producer,
        kind: PortKind::Input,
        port_idx: 0,
    });
    let back_drop_at = harness.center_of(back_sink);
    harness.advance_past_double_click(|ui| {
        draw(ui, &mut graph_ui, &doc);
    });
    harness.press_on(back_source);
    harness.frame(|ui| {
        draw(ui, &mut graph_ui, &doc);
    });
    harness.drag_to(back_drop_at);
    harness.frame(|ui| {
        draw(ui, &mut graph_ui, &doc);
    });
    harness.release_button(PointerButton::Left);
    let refused = harness.frame_value(|ui| draw(ui, &mut graph_ui, &doc));
    assert!(
        refused.is_empty(),
        "a drop that would close a cycle never snaps, so it binds nothing: {refused:?}"
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
    use crate::core::edit::intent::types::GraphIntent;
    use crate::core::preview::{PreviewSink, preview_func};

    let mut library = one_func_library();
    library.add(preview_func(std::sync::Arc::<PreviewSink>::default()));
    let probe = library.by_name("probe").expect("just added").clone();

    let mut doc = Document::default();
    let producer = doc.graph.add_func_node(&probe);
    doc.main_view = GraphView::for_graph(&doc.graph);
    spread(&mut doc.main_view);

    let mut harness = UiHarness::new(UVec2::new(1200, 800));
    let theme = Theme::default();
    let run_state = RunState::default();
    let mut graph_ui = GraphUI::default();
    let out_port = PortRef {
        node_id: producer,
        kind: PortKind::Output,
        port_idx: 0,
    };

    // One frame to record the node so its output circle has a widget id and
    // `CanvasGeometry` a measured center; the gesture refuses an unmeasured
    // port outright.
    let draw = |ui: &mut Ui, graph_ui: &mut GraphUI| {
        let ctx = AppContext {
            theme: &theme,
            library: &library,
            run_state: &run_state,
            status_error: None,
            process_memory: 0,
        };
        let mut intents = Intents::default();
        let mut types = OutputTypes::default();
        let graph_scope = scope(&doc, &library, &run_state, &mut types);
        graph_ui.prepass(ui, graph_scope, &mut intents);
        Panel::vstack()
            .id_salt("pane")
            .size((Sizing::FILL, Sizing::FILL))
            .show(ui, |ui| {
                graph_ui.draw(ui, &ctx, graph_scope, &mut intents);
            });
        intents.drain().collect::<Vec<_>>()
    };

    harness.frame(|ui| {
        draw(ui, &mut graph_ui);
    });
    harness.frame(|ui| {
        draw(ui, &mut graph_ui);
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
        draw(ui, &mut graph_ui);
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
    let spawned = harness.frame_value(|ui| draw(ui, &mut graph_ui));
    harness.set_modifiers(Modifiers::default());
    harness.release_button(PointerButton::Left);

    // The helper carries the pane assertion: the spawn is raised against the
    // port's own pane, not whichever one happens to be focused.
    let spawned = graph_intents(&spawned);
    let adds: Vec<_> = spawned
        .iter()
        .filter(|intent| matches!(intent, GraphIntent::AddNode { .. }))
        .collect();
    assert_eq!(adds.len(), 1, "one preview spawned: {spawned:?}");
    let GraphIntent::AddNode { node_id, node, .. } = adds[0] else {
        unreachable!("filtered to AddNode");
    };
    assert!(
        matches!(node.kind, NodeKind::Func(id) if crate::core::preview::is_preview(id)),
        "the spawned node is a preview"
    );
    assert!(
        spawned.iter().any(|intent| matches!(
            intent,
            GraphIntent::SetInput { input, to: Some(Binding::Bind(src)) }
                if input.node_id == *node_id
                    && src.node_id == producer
                    && src.port_idx == 0
        )),
        "and it is already reading the port the drag came off: {spawned:?}"
    );
}

/// A graph edit reaches the very next read, with nothing announced.
///
/// The canvas holds no derived state about the graph, so there is no
/// invalidation step between wiring a port and the canvas reporting its new
/// type — which is the whole reason the wildcard resolution is safe to do per
/// read. Wired through the same `scope` the record passes build.
#[test]
fn a_wire_edit_reaches_the_next_read_with_nothing_announced() {
    let library = wildcard_library();
    let probe = library.by_name("probe").expect("just added").clone();
    let passthrough = library.by_name("passthrough").expect("just added").clone();

    let mut doc = Document::default();
    // `probe` declares a fixed `Int` output; the passthrough mirrors whatever
    // reaches its input, so unwired it resolves to `Any`.
    let producer = doc.graph.add_func_node(&probe);
    let consumer = doc.graph.add_func_node(&passthrough);
    doc.main_view = GraphView::for_graph(&doc.graph);

    let run_state = RunState::default();
    let resolved_output = |doc: &Document| {
        let mut types = OutputTypes::default();
        scope(doc, &library, &run_state, &mut types)
            .node(consumer)
            .expect("the passthrough resolves")
            .output(0)
            .expect("it declares one output")
            .ty()
    };

    assert_eq!(
        resolved_output(&doc),
        DataType::Any,
        "an unwired passthrough has nothing to mirror"
    );

    doc.graph
        .set_input_binding(InputPort::new(consumer, 0), Binding::bind(producer, 0));
    assert_eq!(
        resolved_output(&doc),
        DataType::Int,
        "the next read follows the new wire — nothing was invalidated in between"
    );
}
