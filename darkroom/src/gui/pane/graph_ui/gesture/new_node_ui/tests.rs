use std::time::Duration;

use glam::UVec2;
use palantir::RgbaF32;
use scenarium::{FuncId, Graph, Library, internals};

use super::*;
use crate::core::document::internals::DocumentFixture;
use crate::gui::pane::graph_ui::internals::CanvasHarness;

/// A much bigger search field and a much roomier popup than the defaults.
fn enlarge(t: &mut palantir::Theme) {
    t.text.font_size *= 3.0;
    t.context_menu.padding = palantir::Spacing::all(24.0);
    t.context_menu.gap = 12.0;
}

/// `n` stub funcs in one category — enough rows for a palette to overflow.
fn bulk_library(n: usize) -> Library {
    let mut library = Library::default();
    for i in 0..n {
        library.add(
            internals::stub_func(FuncId::unique(), format!("func{i:02}"))
                .category("Bulk")
                .build(),
        );
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
            DocumentFixture::with_library(Graph::default(), library),
            SURFACE,
        );
        restyle(h.ui.ui().theme_mut());

        h.frame();
        // Right-click on empty canvas opens the palette; give it two frames so
        // the search field has measured and the cap reads its real height.
        h.ui.right_click(Vec2::new(500.0, 400.0));
        h.prime(2);

        // The same cap `NewNodeUi::apply` resolves, against this harness's
        // 900 px surface.
        let cap = popup_cap(&h.ctx.theme, SURFACE.y as f32);
        Palette {
            field: h.ui.rect(search_row_wid()).expect("search row recorded"),
            results: h.ui.rect(results_wid()).expect("results recorded"),
            cap,
        }
    }

    /// The field sits above the results, and the two plus the popup's own
    /// inset fit the cap with no slack the inset doesn't account for —
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
        // rebuilt from the terms `space_above_results` adds up: a term missing
        // from both sides of a rebuilt sum cancels, and the test passes over
        // exactly the gap the popup then paints its rows outside. The inset
        // around the span is the theme's padding plus the panel's border, which
        // palantir folds into the padding on every side.
        let inset = menu.padding.sums().h + 2.0 * menu.panel.border.width;
        let used = inset + (results.max().y - field.min.y);
        assert!(
            used <= cap + 0.5,
            "{label}: field {} and results {} overflow the {cap} cap (used {used})",
            field.size.h,
            results.size.h,
        );
        // With 60 rows the results always want more room than they get, so the
        // area has to claim everything the inset didn't — an allowance that
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
/// The reset is `GraphUi`-wide, so the palette here stands in for every
/// gesture the same blink would drop.
#[test]
fn a_caret_blink_does_not_read_as_the_canvas_having_been_away() {
    /// Past the caret's blink half-period, so the wake has fired by the frame
    /// below and that frame has nothing else to do.
    const IDLE: Duration = Duration::from_millis(600);

    let mut h = CanvasHarness::new(DocumentFixture::with_library(
        Graph::default(),
        bulk_library(12),
    ));
    h.frame();
    let anchor = Vec2::new(500.0, 400.0);
    h.ui.right_click(anchor);
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
    h.ui.hover(opened.center() + Vec2::new(1.0, 0.0));
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

/// The cap is [`POPUP_MAX_HEIGHT`] inside a tall window, the window less the theme's `lg`
/// (16) inside a short one, and the floor inside one shorter still.
#[test]
fn the_popup_cap_holds_the_palette_inside_the_window() {
    let theme = Theme::default();
    assert_eq!(popup_cap(&theme, 900.0), 400.0);
    assert_eq!(popup_cap(&theme, 300.0), 300.0 - 16.0);
    assert_eq!(popup_cap(&theme, 100.0), POPUP_MIN_HEIGHT);
}

/// The palette keeps the place its first frame took: opened near the window's bottom edge, the
/// window lifts the whole library above the point, and the first letter, which leaves a few
/// rows, must not drop the field back to the point.
#[test]
fn the_search_field_stays_where_the_palette_opened() {
    const SURFACE: UVec2 = UVec2::new(1200, 900);
    let mut h = CanvasHarness::sized(
        DocumentFixture::with_library(Graph::default(), bulk_library(60)),
        SURFACE,
    );
    h.frame();
    h.ui.right_click(Vec2::new(500.0, 860.0));
    h.prime(3);
    let opened = h.ui.rect(search_row_wid()).expect("the palette opened");
    assert!(
        opened.min.y < 860.0 - 100.0,
        "premise: the window lifted the palette above the point ({opened:?})",
    );

    h.ui.type_text("func0");
    h.prime(3);
    let results = h.ui.rect(results_wid()).expect("rows match");
    assert!(
        results.size.h < 400.0 - 100.0,
        "premise: the query shrank the list ({results:?})",
    );
    assert_eq!(h.ui.rect(search_row_wid()), Some(opened), "the field moved");
}

/// Down walks the focus past the scroll's bottom edge, and the list follows it: the focused
/// row is inside the results' rect, and the first row has scrolled out.
#[test]
fn the_list_follows_the_focus_the_keys_move() {
    let mut h = CanvasHarness::new(DocumentFixture::with_library(
        Graph::default(),
        bulk_library(60),
    ));
    h.frame();
    h.ui.right_click(Vec2::new(500.0, 100.0));
    h.prime(3);
    for _ in 0..30 {
        h.ui.key(palantir::Key::ArrowDown);
        h.prime(1);
    }
    h.prime(2);
    let results = h.ui.rect(results_wid()).expect("the palette opened");
    let focused =
        h.ui.rect(node_palette::row_wid(30))
            .expect("row 30 recorded");
    assert!(
        results.contains_rect(focused),
        "row 30 ({focused:?}) is outside the results ({results:?})",
    );
    // A visible rect is clipped to the viewport, so a row scrolled out has none left.
    let first = h.ui.rect(node_palette::row_wid(0));
    assert!(
        first.is_none_or(|first| first.size.h == 0.0),
        "the first row is still in view: {first:?}",
    );
}

/// The focused row's ink keeps 4.5:1 on its tint (WCAG 2.2 SC 1.4.3), and a match's accent on a
/// resting row keeps it on the popup's fill, which is a node's. The tint is an opaque wash, so
/// the GPU paints it as it is.
#[test]
fn the_focused_rows_ink_keeps_its_contrast_on_the_tint() {
    let theme = Theme::default();
    let tint = node_palette::focus_fill(&theme);
    assert_eq!(tint.a, 1.0, "a wash is opaque");
    let luminance = |c: RgbaF32| 0.2126 * c.r + 0.7152 * c.g + 0.0722 * c.b;
    let contrast = |ink: RgbaF32, fill: RgbaF32| (luminance(ink) + 0.05) / (luminance(fill) + 0.05);
    let text = theme.ink.text;
    let ratio = contrast(text, tint);
    assert!(
        ratio >= 4.5,
        "the focused row's ink is {ratio:.2}:1 on its tint"
    );
    let ratio = contrast(theme.colors.accent, theme.card.fill);
    assert!(ratio >= 4.5, "a resting match is {ratio:.2}:1 on the popup");
}
