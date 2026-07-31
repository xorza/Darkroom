use glam::Vec2;
use palantir::Modifiers;

use crate::core::document::harness::DocFixture;
use crate::core::edit::intent::types::GraphIntent;
use crate::gui::pane::graph::harness::CanvasHarness;

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

    // Placed by id rather than left on the fixture's row: this case cares
    // *which* node the second band reaches, and the row assigns by map
    // iteration order.
    let fixture = DocFixture::probes(2);
    let (a, b) = (fixture.node(0), fixture.node(1));
    let mut h = CanvasHarness::new(
        fixture
            .placed(a, Vec2::new(40.0, 40.0))
            .placed(b, Vec2::new(400.0, 40.0)),
    );
    h.prime(2);

    // Sweep bare canvas across both nodes, then cancel mid-drag.
    let empty = Vec2::new(20.0, 400.0);
    h.ui.press_at(empty);
    h.frame();
    h.ui.drag_to(Vec2::new(700.0, 60.0));
    h.frame();
    h.ui.key(Key::Escape);
    let cancelled = h.frame();
    assert!(
        cancelled.is_empty(),
        "a cancelled band commits nothing: {cancelled:?}"
    );
    h.ui.release_button(palantir::PointerButton::Left);
    let after = h.frame();
    assert!(
        after.is_empty(),
        "and the release of a cancelled band commits nothing either: {after:?}"
    );

    // A fresh band over `a` alone must select exactly `a` — if the
    // cancelled drag's `base` survived, `b` would ride along. The nodes sit
    // at x = 40 and x = 400, so a sweep stopping at x = 150 reaches the
    // first and not the second; both bands start from the same empty patch
    // well below them.
    h.ui.press_at(empty);
    h.frame();
    h.ui.drag_to(Vec2::new(150.0, 100.0));
    h.frame();
    h.ui.release_button(palantir::PointerButton::Left);
    let intents = h.frame();
    assert!(
        matches!(
            intents[..],
            [GraphIntent::SetSelection { ref to }] if to.len() == 1 && to.contains(&a),
        ),
        "the next band selects only what it swept, with no residue from the \
         cancelled one: {intents:?} (b = {b:?})"
    );
}

/// A Shift-band commits the union of what it swept and what was already
/// selected, counting a node in both exactly once.
///
/// The overlap is the case that pins the sweep's shape: `base` arrives sorted
/// off the document's `BTreeSet`, the sweep appends in paint order, and a node
/// in both lands twice. `Selection::swept` binary-searches, so a sweep left
/// unsorted or undeduplicated answers *wrong* about what paints selected —
/// its debug assertion fires on every frame of this drag.
#[test]
fn a_shift_band_unions_with_the_committed_selection_counting_overlap_once() {
    let fixture = DocFixture::probes(2);
    let (a, b) = (fixture.node(0), fixture.node(1));
    let mut h = CanvasHarness::new(
        fixture
            .placed(a, Vec2::new(40.0, 40.0))
            .placed(b, Vec2::new(260.0, 40.0)),
    );
    // `a` is already selected, and the band below sweeps *both* — so `a`
    // reaches the swept buffer twice, once from the base and once from the
    // sweep.
    h.doc_mut().main_view.selected.insert(a);
    h.prime(2);

    h.ui.set_modifiers(Modifiers {
        shift: true,
        ..Modifiers::default()
    });
    h.ui.press_at(Vec2::new(20.0, 400.0));
    h.frame();
    // Two steps, not one: the band anchors where `drag.started()` first
    // fires, so a single jump would latch and finish at the same point and
    // sweep a zero-area rect.
    h.ui.drag_to(Vec2::new(20.0, 390.0));
    h.frame();
    h.ui.drag_to(Vec2::new(700.0, 60.0));
    h.frame();
    h.ui.release_button(palantir::PointerButton::Left);
    let intents = h.frame();

    // Both nodes, each once. `SetSelection` carries a `BTreeSet`, so the
    // count is what says the duplicate was folded before it got there.
    assert!(
        matches!(
            intents[..],
            [GraphIntent::SetSelection { ref to }]
                if to.len() == 2 && to.contains(&a) && to.contains(&b),
        ),
        "the shift band commits both nodes exactly once: {intents:?} (a = {a:?}, b = {b:?})"
    );
}
