use glam::{UVec2, Vec2};
use palantir::internals::UiHarness;
use palantir::{Configure, Panel, Sizing, Ui};
use scenarium::{Func, FuncId, Library, OutputTypes, testing};

use crate::core::document::Document;
use crate::core::edit::intent::sink::Intents;
use crate::gui::pane::graph::GraphUI;
use crate::gui::pane::graph::harness::*;
use crate::gui::state::run_state::RunState;
use crate::gui::theme::Theme;

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

    use crate::gui::pane::graph::gesture::new_node::{results_wid, search_field_wid};

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
            let ctx = app(theme, &library, &run_state);
            let mut intents = Intents::default();
            let mut types = OutputTypes::default();
            let graph_ctx = graph_ctx_for(ctx, &doc, &mut types);
            graph_ui.prepass(ui, graph_ctx, &mut intents);
            Panel::vstack()
                .id_salt("pane")
                .size((Sizing::FILL, Sizing::FILL))
                .show(ui, |ui| {
                    graph_ui.draw(ui, graph_ctx, &mut intents);
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

use super::*;

#[test]
fn name_matches_is_case_insensitive_substring_with_empty_query_wildcard() {
    // Empty query is the "show everything" wildcard.
    assert!(name_matches("Gaussian Blur", ""));
    assert!(name_matches("", ""));
    // Case-insensitive substring anywhere in the name. Caller passes an
    // already-lowercased query, so only the name is folded here.
    assert!(name_matches("Gaussian Blur", "blur"));
    assert!(name_matches("Gaussian Blur", "gauss"));
    assert!(name_matches("Gaussian Blur", "an bl"));
    // Non-substring and wrong-fragment queries reject.
    assert!(!name_matches("Gaussian Blur", "sharpen"));
    assert!(!name_matches("Blur", "blurry"));
    // A non-lowercased query never matches a lowercased name — the
    // contract is "query already lowercased", so this documents that a
    // caller who forgets to fold gets no false positives. It holds on
    // both sides of the ASCII fast path.
    assert!(!name_matches("blur", "BLUR"));
    assert!(!name_matches("Grün", "GRÜN"));
    // Non-ASCII names fold by the Unicode rules, not byte-wise.
    assert!(name_matches("Grün", "grün"));
    assert!(name_matches("Ölfilter", "ölfil"));
    assert!(!name_matches("Grün", "grun"));
    // An ASCII name never matches a non-ASCII query.
    assert!(!name_matches("Blur", "blür"));
}
