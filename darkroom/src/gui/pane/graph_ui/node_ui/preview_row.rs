//! A preview node's body content: the value wired into it.
//!
//! Takes the slot an ordinary node's footer occupies: a preview has no output, so no cached
//! output to report. It is a row inside a normal node body, so selection, dragging, the
//! breaker, and the input wire all come from the node machinery.

use palantir::{
    Align, Configure, CursorIcon, Fit, ImagePaint, Justify, Panel, Sense, Shape, SizeSpec, Sizing,
    Text, TextWrap, Ui, VAlign, Visibility, fmt,
};

use crate::gui::pane::graph_ui::node_ui::card;
use crate::gui::pane::graph_ui::node_ui::card::LabeledValue;
use crate::gui::pane::graph_ui::node_ui::node_record::NodeRecord;
use crate::gui::pane::graph_ui::wid;
use crate::gui::state::preview_store::{DrawablePreview, StoredContent};
use crate::gui::theme::Theme;
use crate::gui::widgets::format::bytes::Bytes;
use crate::gui::widgets::format::pixel_size::PixelSize;
use crate::gui::widgets::support;

/// Height of the value area itself, excluding the header, port row, and info footer. Fixed, so
/// a node does not resize as values arrive and vanish.
const PREVIEW_CONTENT_HEIGHT: f32 = 152.0;

/// What a preview shows when it has never received a value: a run that produced nothing and a
/// preview wired to nothing both mean "there is no value yet".
const EMPTY_LABEL: &str = "No value yet";

/// Draw one preview node's value area, plus the image info footer when there is
/// an image to describe.
pub(super) fn show(ui: &mut Ui, record: &mut NodeRecord<'_, '_>) {
    let node_ctx = record.node;
    let theme = node_ctx.theme();
    let node = node_ctx;
    let program_id = node.program_id();
    let stored = program_id.and_then(|id| node_ctx.graph_ctx.run_state().previews().get(id));
    let has_image = stored.and_then(StoredContent::image).is_some();
    // A picture reads at any zoom; the line that stands in for one is words.
    let words = record.stage.ctx.words();
    let content = Panel::vstack()
        .id(wid::preview_image(node.graph_ctx.graph(), node.node_id))
        .size((Sizing::FILL, Sizing::fixed(PREVIEW_CONTENT_HEIGHT)))
        // The header's and footer's inset, so the image's left edge lines up with the title.
        .padding(card::band_padding(theme))
        .child_align(Align::CENTER)
        .justify(Justify::Center)
        // Only an image opens a viewer, so only an image is clickable.
        .sense(if has_image { Sense::CLICK } else { Sense::NONE })
        .show(ui, |ui| {
            if let Some(image) = stored.and_then(StoredContent::image) {
                ui.add_shape(
                    Shape::rect(),
                    ImagePaint::new(image.preview.handle.clone()).fit(Fit::Contain),
                );
            } else {
                // `message` is complementary to `image`, so this covers a
                // formatted non-image value and a value that failed to prepare
                // alike, in the body ink; `EMPTY_LABEL` covers having nothing at all, in the muted
                // ink every empty state takes.
                let (text, style) = match stored.and_then(StoredContent::message) {
                    Some(message) => (
                        fmt!(ui, "{message}"),
                        support::sized_text(ui, theme.font.caption),
                    ),
                    None => (
                        ui.intern(EMPTY_LABEL),
                        support::muted_text(ui, theme, theme.font.caption),
                    ),
                };
                Text::new(text)
                    .style(&style)
                    .visibility(words)
                    .text_wrap(TextWrap::Wrap)
                    .show(ui);
            }
        })
        .response;
    if content.hovered() {
        ui.request_cursor(CursorIcon::Pointer);
    }
    // The card's one action: show this node's value at full resolution, in the viewer that
    // follows the active preview. Navigation rather than a graph edit, as the pane arrangement
    // is the dock's.
    if content.clicked() {
        let program_id = program_id.expect("an image is stored only under a program node");
        record.stage.out.push_show_preview(program_id);
    }
    if let Some(image) = stored.and_then(StoredContent::image) {
        info_row(ui, node_ctx.theme(), image, record.stage.ctx.words());
    }
}

/// The image's info footer: resolution, pixel format, and
/// original source size, styled like a node body's
/// [`node_footer`](crate::gui::pane::graph_ui::node_ui::node_footer).
fn info_row(ui: &mut Ui, theme: &Theme, image: &DrawablePreview, words: Visibility) {
    Panel::hstack()
        .id_salt("preview_info")
        .visibility(words)
        .size(SizeSpec::FILL_W)
        .padding(card::band_padding(theme))
        .gap(theme.space.md)
        .child_align(VAlign::Center)
        // Round only the bottom corners, to seat into the body's rounded bottom.
        .chrome(theme.footer_chrome(theme.card_inner_radius()))
        .show(ui, |ui| {
            card::bare_value(ui, theme, PixelSize(image.preview.native_size));
            card::bare_value(ui, theme, image.preview.native_format);
            Panel::hstack()
                .id_salt("preview_info_size")
                .size(SizeSpec::HUG)
                .gap(theme.space.xs)
                .child_align(VAlign::Center)
                .show(ui, |ui| {
                    LabeledValue {
                        label: "Source",
                        value: Bytes::new(image.source_bytes as u64),
                    }
                    .show(ui, theme);
                });
        });
}

#[cfg(test)]
mod tests {
    use scenarium::GraphId;

    /// Clicking a preview card's image asks the dock for that node's viewer
    /// tab — the canvas's one view-tier request.
    #[test]
    fn clicking_a_preview_card_asks_for_its_viewer_tab() {
        use std::sync::Arc;

        use lens::DisplayStretch;
        use scenarium::ProgramId;

        use crate::core::document::internals::DocumentFixture;
        use crate::core::preview_sink;
        use crate::gui::pane::graph_ui::internals::CanvasHarness;
        use crate::gui::pane::graph_ui::wid;
        use crate::gui::state::preview_store;

        let mut fixture = DocumentFixture::default();
        let node = fixture.add(&preview_sink::func(Arc::default()));
        let mut h = CanvasHarness::new(fixture);
        // The run projection `App` would have filled from a completed run:
        // without a value the card records `Sense::NONE` and swallows the click.
        h.ctx.run_state.previews_mut().publish(
            ProgramId::from(node),
            preview_store::internals::opaque_image_value(),
            DisplayStretch::Linear,
        );
        h.ctx.run_state.previews_mut().settle(h.ui.ui());
        h.prime(2);
        assert!(
            h.shown_previews.is_empty(),
            "nothing asked for before the click"
        );

        h.ui.click(wid::preview_image(GraphId::Main, node));
        let intents = h.frame();

        assert_eq!(h.shown_previews, [ProgramId::from(node)]);
        assert!(
            intents.is_empty(),
            "opening a viewer is navigation, not a graph edit: {intents:?}"
        );
    }
}
