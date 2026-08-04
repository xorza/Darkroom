use glam::Vec2;
use scenarium::{Binding, InputPort};

use crate::core::document::PortRef;
use crate::core::document::harness::DocFixture;
use crate::gui::pane::graph::harness::CanvasHarness;

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
    let mut fixture = DocFixture::probes(2);
    let (stays, leaves) = (fixture.node(0), fixture.node(1));
    fixture
        .doc
        .graph
        .set_input_binding(InputPort::new(leaves, 0), Binding::bind(stays, 0));

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
    let before = h.doc().main_view.item_placements[&leaves].pos;
    let shift = Vec2::new(6000.0, 4000.0);
    h.doc_mut()
        .main_view
        .item_placements
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
    // centre travelled exactly as far as the node did.
    assert!(
        (culled - anchored - shift).length() < 0.01,
        "expected the centre to move by {shift:?}, got {:?}",
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
    h.doc_mut().remove_node(leaves);
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
