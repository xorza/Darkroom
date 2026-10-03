use std::time::Duration;

use glam::UVec2;
use scenarium::{FuncId, Graph, Library, testing};

use super::*;
use crate::core::document::harness::DocFixture;
use crate::gui::pane::graph::harness::CanvasHarness;

/// A much bigger search field and a much roomier popup than the defaults.
fn enlarge(t: &mut palantir::Theme) {
    t.text.font_size_px *= 3.0;
    t.context_menu.padding = palantir::Spacing::all(24.0);
    t.context_menu.gap = 12.0;
}

/// `n` stub funcs in one category — enough rows for a palette to overflow.
fn bulk_library(n: usize) -> Library {
    let mut library = Library::default();
    for i in 0..n {
        library.add(testing::stub_func(FuncId::unique(), format!("func{i:02}")).category("Bulk"));
    }
    library
}

/// The new-node palette keeps its search field and its results inside the
/// height cap, at whatever height the field actually measures.
///
/// The results `Scroll` has to carry an explicit cap of its own — a stack
/// hands every non-`Fill` child its full main extent, so a `Hug` scroll offered
/// the popup's cap takes all of it and shoves the search row past the bottom.
/// The cap is measured off the field's real height, so both cases below run
/// the same assertion, the second with the field's text and the popup's
/// padding scaled well up.
#[test]
fn the_palette_sizes_its_results_area_from_the_search_row_it_actually_has() {
    use palantir::Rect;

    /// The surface the palette opens against — 900 px tall, which is what the
    /// cap below is resolved from.
    const SURFACE: UVec2 = UVec2::new(1200, 900);

    /// The two rects the height cap divides between, and the cap itself.
    #[derive(Debug, Clone, Copy)]
    struct Palette {
        field: Rect,
        results: Rect,
        cap: f32,
    }

    /// Records one palette open, with `restyle` applied to the live
    /// `Ui::theme` first.
    fn open_palette(restyle: impl Fn(&mut palantir::Theme)) -> Palette {
        // Enough rows in one category to overflow any sane cap, so the scroll
        // is genuinely competing for the popup's height. No nodes placed: the
        // palette is what spawns them.
        let library = bulk_library(60);
        // Real shaping: the search field sizes to its text, which is the
        // measurement the cap has to divide around.
        let mut h = CanvasHarness::shaping_text(
            DocFixture::with_library(Graph::default(), library),
            SURFACE,
        );
        restyle(h.ui.ui().theme_mut());

        h.frame();
        // Right-click on empty canvas opens the palette; give it two frames so
        // the search field has measured and the cap reads its real height.
        h.ui.right_click_at(Vec2::new(500.0, 400.0));
        h.prime(2);

        // The same cap `NewNodeUi::apply` resolves, against this harness's
        // 900 px surface.
        let cap = popup_cap(h.ctx.theme.new_node_popup_max_height, SURFACE.y as f32);
        Palette {
            field: h.ui.rect(search_field_wid()).expect("field recorded"),
            results: h.ui.rect(results_wid()).expect("results recorded"),
            cap,
        }
    }

    /// The field sits above the results, and the two plus the popup's own
    /// chrome fit the cap with no slack the chrome doesn't account for —
    /// which catches an allowance that over-subtracts as well as one that
    /// under-subtracts.
    fn assert_fits(palette: Palette, menu: &palantir::ContextMenuTheme, label: &str) {
        let Palette {
            field,
            results,
            cap,
        } = palette;
        assert!(
            field.max().y <= results.min.y + 0.5,
            "{label}: the results overlap the search field ({field:?} vs {results:?})",
        );
        // Spanned from the field's top to the results' bottom rather than
        // rebuilt from the terms `chrome_above_results` adds up: a term missing
        // from both sides of a rebuilt sum cancels, and the test passes over
        // exactly the gap the popup then paints its rows outside.
        let used = menu.padding.vertical_sum() + (results.max().y - field.min.y);
        assert!(
            used <= cap + 0.5,
            "{label}: field {} and results {} overflow the {cap} cap (used {used})",
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

    let plain = palantir::Theme::default();
    let small = open_palette(|_| {});
    assert_fits(small, &plain.context_menu, "default theme");

    // Now restyle both terms a fixed allowance could never track: a much
    // bigger search field, and a much roomier popup. The results area has to
    // give up exactly what they took.
    let mut restyled = palantir::Theme::default();
    enlarge(&mut restyled);
    let big = open_palette(enlarge);
    assert_fits(big, &restyled.context_menu, "bigger field and popup");

    assert!(
        big.field.size.h > small.field.size.h,
        "the field really did grow: {} → {}",
        small.field.size.h,
        big.field.size.h,
    );
    assert!(
        big.results.size.h < small.results.size.h,
        "and the results gave up the difference: {} → {}",
        small.results.size.h,
        big.results.size.h,
    );
}

/// An idle frame that only *paints* must not read as the canvas having been
/// away — the open palette focuses its search field, whose caret blink wakes
/// the runtime with no input behind it, and that wake runs no record pass.
///
/// A stamp that counted painted frames would see a gap at every blink, and the
/// next real frame would reset every in-flight gesture, the palette with them.
/// The reset is `GraphUI`-wide, so the palette here stands in for every
/// gesture the same blink would drop.
#[test]
fn a_caret_blink_does_not_read_as_the_canvas_having_been_away() {
    /// Past the caret's blink half-period, so the wake has fired by the frame
    /// below and that frame has nothing else to do.
    const IDLE: Duration = Duration::from_millis(600);

    let mut h = CanvasHarness::new(DocFixture::with_library(Graph::default(), bulk_library(12)));
    h.frame();
    let anchor = Vec2::new(500.0, 400.0);
    h.ui.right_click_at(anchor);
    h.prime(2);
    let opened = h.ui.rect(results_wid()).expect("the palette opened");

    h.ui.advance(IDLE);
    assert!(
        !h.try_frame(),
        "the idle frame recorded a pass, so it is not the paint-only case \
         this test is about",
    );

    // The smallest input there is: the pointer moves within the row it was
    // already over. Nothing about the palette changed — only the frame the
    // blink slipped in between.
    h.ui.move_to(opened.center() + Vec2::new(1.0, 0.0));
    h.prime(1);
    assert_eq!(
        h.ui.rect(results_wid()),
        Some(opened),
        "the palette closed across a paint-only frame",
    );
}

/// The search reports a change only when its fold changed, so the palette
/// filters its rows once per edit of the query and not on every frame — and
/// not when the edit only changed the case.
#[test]
fn the_search_reports_only_a_changed_fold() {
    let mut search = Search::default();
    assert!(!search.fold(), "an empty query folds to what it was");

    search.text.push_str("Blur");
    assert!(search.fold());
    assert_eq!(search.folded, "blur");
    assert!(!search.fold(), "an unchanged query is no change");

    search.text.make_ascii_uppercase();
    assert!(!search.fold(), "a case change folds the same");
    assert_eq!(search.folded, "blur");

    search.text.push('x');
    assert!(search.fold());
    assert_eq!(search.folded, "blurx");
}

/// The cap is the theme's inside a tall window, the window less its margin
/// inside a short one, and the floor inside one shorter still.
#[test]
fn the_popup_cap_holds_the_palette_inside_the_window() {
    assert_eq!(popup_cap(400.0, 900.0), 400.0);
    assert_eq!(popup_cap(400.0, 300.0), 300.0 - POPUP_WINDOW_MARGIN);
    assert_eq!(popup_cap(400.0, 100.0), POPUP_MIN_HEIGHT);
}
