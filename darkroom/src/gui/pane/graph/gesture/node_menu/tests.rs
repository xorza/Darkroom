use glam::UVec2;

use crate::core::document::harness::DocFixture;
use crate::core::edit::intent::types::GraphIntent;
use crate::gui::pane::graph::harness::CanvasHarness;

/// A node-body right-click selects the node it landed on before the menu
/// opens, so whatever the user picks next acts on a coherent set.
///
/// It comes out of the closure the shared trigger scan takes — "which of this
/// node's widgets opens the menu" — so it's checked through a real click
/// rather than by calling it.
#[test]
fn a_node_body_right_click_selects_the_node_it_landed_on() {
    // Two nodes, so the assertion that exactly one ends up selected has
    // something to exclude, on a surface wide enough for the opened menu.
    let mut h = CanvasHarness::sized(DocFixture::probes(2), UVec2::new(1600, 900));
    let func = h.node(0);
    // Two frames so every node body has recorded and carries a hit-testable
    // rect for the click below.
    h.prime(2);

    let on_func = h.node_center(func);
    h.ui.right_click_at(on_func);
    let intents = h.frame();
    assert!(
        matches!(
            intents[..],
            [GraphIntent::SetSelection { ref to }] if to.len() == 1 && to.contains(&func),
        ),
        "the right-click selects exactly the node it opened on: {intents:?}"
    );
}
