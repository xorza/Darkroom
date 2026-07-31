use glam::{UVec2, Vec2};
use palantir::internals::UiHarness;
use palantir::{Configure, Panel, Sizing, Ui};
use scenarium::{Binding, InputPort, OutputTypes};

use crate::core::document::{Document, GraphView, PortKind, PortRef};
use crate::core::edit::intent::sink::Intents;
use crate::gui::pane::graph::GraphUI;
use crate::gui::pane::graph::harness::*;
use crate::gui::state::run_state::RunState;
use crate::gui::theme::Theme;

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
        let ctx = app(&theme, &library, &run_state);
        let mut intents = Intents::default();
        let mut types = OutputTypes::default();
        let graph_ctx = graph_ctx_for(ctx, doc, &mut types);
        graph_ui.prepass(ui, graph_ctx, &mut intents);
        Panel::vstack()
            .id_salt("pane")
            .size((Sizing::FILL, Sizing::FILL))
            .show(ui, |ui| {
                graph_ui.draw(ui, graph_ctx, &mut intents);
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
    let live = graph_ctx_for(app(&theme, &library, &run_state), &doc, &mut live_types);
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
