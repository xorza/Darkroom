use glam::Vec2;
use scenarium::{Binding, InputPort};

use crate::core::document::internals::DocumentFixture;
use crate::gui::pane::graph_ui::internals::CanvasHarness;
use crate::gui::port_ref::PortRef;

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
    let mut fixture = DocumentFixture::probes(2);
    let (stays, leaves) = (fixture.node(0), fixture.node(1));
    fixture
        .document
        .graph
        .set_input_binding(InputPort::new(leaves, 0), Binding::wire(stays, 0));

    // Both on screen and recorded, so every glyph has a fresh offset cached.
    let mut h = CanvasHarness::new(fixture);
    h.prime(2);

    let out_port = PortRef::output(leaves, 0);
    let anchored = h
        .graph_ui
        .geometry()
        .ports
        .center(out_port)
        .expect("a recorded port resolves its center");

    // Scroll it far past the viewport. Two frames: the first still reads the
    // on-screen record, the second is the culled one that has to reconstruct.
    let before = h.document().main_view.node_placements[&leaves].pos;
    let shift = Vec2::new(6000.0, 4000.0);
    h.document_mut()
        .main_view
        .node_placements
        .get_mut(&leaves)
        .unwrap()
        .pos = before + shift;
    h.prime(2);

    let culled = h
        .graph_ui
        .geometry()
        .ports
        .center(out_port)
        .expect("a culled port still resolves, off the cached offset");
    // And it tracks the move rather than sticking where it last painted: the
    // center travelled as far as the node did. Within 0.01: both centers are a
    // position plus the cached offset in f32, and near 6000 one rounding is
    // about 0.0005, so a few of them stay far under the bound while a stale
    // center would miss by thousands.
    assert!(
        (culled - anchored - shift).length() < 0.01,
        "expected the center to move by {shift:?}, got {:?}",
        culled - anchored,
    );

    // The node the document keeps holds its cached size; the other one is
    // still cached too, because being off-screen is not being deleted.
    for id in [stays, leaves] {
        assert!(
            h.node_world_rect(id).is_some(),
            "an off-screen node is not a deleted one",
        );
    }

    // Now the document drops it for real, and the sweep runs against that
    // document — the shape production takes, where a deleted node is gone from
    // the graph before anything asks whether to keep its cache entries.
    // Read off `node_sizes` directly: `node_world_rect` resolves through a
    // `NodeCtx`, which a deleted node no longer has.
    h.document_mut().remove_node(leaves);
    h.graph_ui.retain_nodes(&h.ctx.open.document);
    assert!(
        !h.graph_ui.geometry().node_sizes.contains_key(&leaves),
        "a node the document stopped holding releases its cached size",
    );
    assert!(
        h.graph_ui.geometry().node_sizes.contains_key(&stays),
        "and its neighbour keeps its own",
    );
    assert!(
        h.node_world_rect(stays).is_some(),
        "which still resolves through the live node",
    );
    // The port offsets went with it, so the next culled rebuild has nothing
    // left to reconstruct from.
    h.prime(1);
    assert_eq!(
        h.graph_ui.geometry().ports.center(out_port),
        None,
        "an evicted node's ports stop resolving",
    );
}

/// A drag frame moves its node in the drain between the prepass and the record, and the ports
/// the record draws wires to sit on the moved node: each center keeps its offset from the node's
/// position in the document the record reads, not the position before the move.
#[test]
fn a_dragged_nodes_ports_follow_it_in_the_frame_that_moves_it() {
    let mut h = CanvasHarness::new(DocumentFixture::probes(2));
    let node = h.node(0);
    let port = PortRef::output(node, 0);
    h.prime(2);
    let pos = |h: &CanvasHarness| h.document().main_view.node_placements[&node].pos;
    let offset = |h: &CanvasHarness| h.graph_ui.geometry().ports.center(port).unwrap() - pos(h);
    let rest = offset(&h);

    let grab = h.node_center(node);
    h.ui.press_at(grab);
    h.frame_applied();
    let mut moves = 0;
    for step in 1..=3 {
        h.ui.drag_to(grab + Vec2::new(37.0, -21.0) * step as f32);
        for _ in 0..2 {
            let before = pos(&h);
            h.frame_applied();
            moves += usize::from(pos(&h) != before);
            // Within f32 rounding: each frame measures the offset again off rects laid out at
            // the moved position, whose sums round a few ulps apart; a frame behind is the
            // whole step of about 43 px.
            let drift = (offset(&h) - rest).abs().max_element();
            assert!(
                drift < 1e-3,
                "step {step}: {:?} against {rest:?}",
                offset(&h)
            );
        }
    }
    assert_eq!(moves, 3, "each step moved the node in one frame");
}
