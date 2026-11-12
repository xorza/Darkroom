use glam::Vec2;
use palantir::{Modifiers, PointerButton};

use crate::core::document::internals::DocumentFixture;
use crate::core::document::item_id::ItemId;
use crate::core::edit::graph_intent::GraphIntent;
use crate::gui::pane::graph_ui::gesture::selection_ui::border;
use crate::gui::pane::graph_ui::internals::CanvasHarness;
use crate::gui::theme::Theme;

/// Escape cancels a rubber band: no `SetSelection`, and the next band
/// starts from a clean base.
///
/// The cancel is resolved once per frame by the canvas and handed to each controller, so this
/// also covers the wiring: a band that kept running after Esc would commit on release. The
/// second band covers `base`, which lives inside `RubberBand`: a cancel that left it behind
/// would union the abandoned drag's selection into the next one.
#[test]
fn escape_cancels_a_rubber_band_and_leaves_no_residue() {
    use palantir::Key;

    // Placed by id, far enough apart that a band can hold one node without the other.
    let fixture = DocumentFixture::probes(2);
    let (a, b) = (fixture.node(0), fixture.node(1));
    let mut h = CanvasHarness::new(
        fixture
            .placed(a, Vec2::new(40.0, 40.0))
            .placed(b, Vec2::new(400.0, 40.0)),
    );
    h.prime(2);

    // Sweep bare canvas across both nodes, then cancel mid-drag.
    let empty = Vec2::new(20.0, 400.0);
    h.ui.press(empty, PointerButton::Left);
    h.frame();
    h.ui.drag_to(Vec2::new(700.0, 60.0));
    h.frame();
    h.ui.key(Key::Escape);
    let cancelled = h.frame();
    assert!(
        cancelled.is_empty(),
        "a cancelled band commits nothing: {cancelled:?}"
    );
    h.ui.release(PointerButton::Left);
    let after = h.frame();
    assert!(
        after.is_empty(),
        "and the release of a cancelled band commits nothing either: {after:?}"
    );

    // A fresh band over `a` alone must select exactly `a`; if the cancelled drag's `base`
    // survived, `b` would ride along. The nodes sit at x = 40 and x = 400, so a sweep stopping
    // at x = 150 reaches only the first; both bands start from the same empty patch below them.
    h.ui.press(empty, PointerButton::Left);
    h.frame();
    h.ui.drag_to(Vec2::new(150.0, 100.0));
    h.frame();
    h.ui.release(PointerButton::Left);
    let intents = h.frame();
    assert!(
        matches!(
            intents[..],
            [GraphIntent::SetSelection { ref to }] if to.len() == 1 && to.contains(&ItemId::Node(a)),
        ),
        "the next band selects only what it swept, with no residue from the \
         cancelled one: {intents:?} (b = {b:?})"
    );
}

/// A Shift-band commits the union of what it swept and what was already
/// selected, counting a node in both exactly once.
///
/// The overlap pins the sweep's shape: `base` arrives sorted off the document's `BTreeSet`, the
/// sweep appends in paint order, and a node in both lands twice. `Selection::swept`
/// binary-searches, so an unsorted or duplicated sweep answers wrong about what paints selected,
/// and its debug assertion fires on every frame of this drag.
#[test]
fn a_shift_band_unions_with_the_committed_selection_counting_overlap_once() {
    let fixture = DocumentFixture::probes(2);
    let (a, b) = (fixture.node(0), fixture.node(1));
    let mut h = CanvasHarness::new(
        fixture
            .placed(a, Vec2::new(40.0, 40.0))
            .placed(b, Vec2::new(260.0, 40.0)),
    );
    // `a` is already selected and the band below sweeps both, so `a` reaches the swept buffer
    // twice, once from the base and once from the sweep.
    h.document_mut().main_view.selected.insert(ItemId::Node(a));
    h.prime(2);

    h.ui.hold_modifiers(Modifiers::SHIFT);
    h.ui.press(Vec2::new(20.0, 400.0), PointerButton::Left);
    h.frame();
    // Two steps, as the band anchors where `drag.started()` first fires: a single jump would
    // sweep a zero-area rect.
    h.ui.drag_to(Vec2::new(20.0, 390.0));
    h.frame();
    h.ui.drag_to(Vec2::new(700.0, 60.0));
    h.frame();
    h.ui.release(PointerButton::Left);
    let intents = h.frame();

    // Both nodes, each once; the count says the duplicate was folded before the `BTreeSet`.
    assert!(
        matches!(
            intents[..],
            [GraphIntent::SetSelection { ref to }]
                if to.len() == 2
                    && to.contains(&ItemId::Node(a))
                    && to.contains(&ItemId::Node(b)),
        ),
        "the shift band commits both nodes exactly once: {intents:?} (a = {a:?}, b = {b:?})"
    );
    assert_eq!(
        h.graph_ui.gestures.selection_ui.preview(),
        None,
        "the release frame paints the committed set, which its drain just wrote"
    );
}

/// The band's border is one hairline on screen at every zoom: its world width times the zoom
/// is the hairline, 1 px, so 2 world px at 50 % and half a world px at 200 %.
#[test]
fn the_band_border_is_a_screen_hairline_at_every_zoom() {
    let theme = Theme::default();
    for (zoom, world) in [(0.5, 2.0), (1.0, 1.0), (2.0, 0.5)] {
        assert_eq!(border(&theme, zoom).width, world, "{zoom}");
        assert_eq!(world * zoom, theme.stroke.hairline);
    }
}
