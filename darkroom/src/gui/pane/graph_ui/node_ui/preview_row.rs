//! A preview node's body content: the value wired into it.
//!
//! Takes the slot an ordinary node's footer occupies. A preview holds no
//! cached output — it has no output at all — so there is nothing for that
//! readout to report, and the value is the only thing worth the space.
//!
//! Not a floating widget with wiring of its own: it is a row inside a normal
//! node body, so selection, dragging, the breaker, and the input wire all come
//! from the node machinery unchanged.

use std::fmt;
use std::fmt::Display;
use std::fmt::Formatter;

use imaginarium::ColorFormat;
use palantir::widget::Shape;
use palantir::{
    Align, Configure, CursorIcon, DockOperation, ImageFit, Justify, Panel, Sense, Sizing, Text,
    TextWrap, Ui, VAlign, fmt,
};

use crate::core::document::tab_ref::TabRef;
use crate::gui::pane::graph_ui::node_ui::card;
use crate::gui::pane::graph_ui::node_ui::card::LabeledValue;
use crate::gui::pane::graph_ui::node_ui::node_record::NodeRecord;
use crate::gui::pane::graph_ui::wid;
use crate::gui::state::preview_store::{DrawablePreview, StoredContent};
use crate::gui::theme::Theme;
use crate::gui::widgets::format::bytes::Bytes;
use crate::gui::widgets::support;

/// Height of the value area itself, excluding the header, port row, and info
/// footer. Fixed, so a node does not resize as values arrive and vanish —
/// an image letterboxes inside via `Contain`, and a non-image centers its text
/// in the same frame.
const PREVIEW_CONTENT_HEIGHT: f32 = 152.0;

/// What a preview shows when it has never received a value. A run that produced
/// nothing and a preview wired to nothing are the same from here — both mean
/// "there is no value yet", and saying more would be guessing.
const EMPTY_LABEL: &str = "No value yet";

/// Draw one preview node's value area, plus the image info footer when there is
/// an image to describe.
pub(super) fn show(ui: &mut Ui, record: &mut NodeRecord<'_, '_>) {
    let node_ctx = record.node;
    let theme = node_ctx.theme();
    let node = node_ctx;
    let stored = node_ctx.graph_ctx.run_state().previews().get(node.node_id);
    let has_image = stored.and_then(StoredContent::image).is_some();
    let content = Panel::vstack()
        .id(wid::preview_image(node.node_id))
        .size((Sizing::FILL, Sizing::fixed(PREVIEW_CONTENT_HEIGHT)))
        // The header's and footer's inset, so the image's left edge lines up with the title.
        .padding(card::band_padding(theme))
        .child_align(Align::CENTER)
        .justify(Justify::Center)
        // Only an image opens a viewer, so only an image is clickable.
        .sense(if has_image { Sense::CLICK } else { Sense::NONE })
        .show(ui, |ui| {
            if let Some(image) = stored.and_then(StoredContent::image) {
                ui.add_shape(Shape::image(image.preview.handle.clone()).fit(ImageFit::Contain));
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
                    .text_wrap(TextWrap::Wrap)
                    .show(ui);
            }
        })
        .response
        .snapshot();
    if content.hovered() {
        ui.set_cursor(CursorIcon::Pointer);
    }
    // The card's one action: open this node's value at full resolution. Asked
    // for here, where the clickable area is built, so the id it senses on and
    // the id its click is read from are the same one.
    //
    // A view request rather than a graph edit — the pane arrangement is the
    // dock's, and `Requests` routes by what is being asked for, not by who
    // asks. Deduped by `OpenTab`, so clicking a card whose viewer is already
    // open just activates it.
    if content.clicked() {
        record.out.push_view(DockOperation::OpenTab {
            tab: TabRef::ImageViewer(node.node_id),
        });
    }
    if let Some(image) = stored.and_then(StoredContent::image) {
        info_row(ui, node_ctx.theme(), image);
    }
}

/// The image's info footer: resolution, pixel format (channel layout + bit depth), and original
/// source size — styled like a node body's footer
/// ([`node_footer`](crate::gui::pane::graph_ui::node_ui::node_footer)), so every read-only fact strip
/// in the app reads as the same kind of thing.
fn info_row(ui: &mut Ui, theme: &Theme, image: &DrawablePreview) {
    Panel::hstack()
        .id_salt("preview_info")
        .size((Sizing::FILL, Sizing::HUG))
        .padding(card::band_padding(theme))
        .gap(theme.space.md)
        .child_align(Align::v(VAlign::Center))
        // Round only the bottom corners so the strip seats into the body's
        // rounded bottom, the way the header rounds the top.
        .chrome(card::footer_chrome(theme))
        .show(ui, |ui| {
            let size = image.preview.native_size;
            card::bare_value(ui, theme, format_args!("{}\u{d7}{}", size.x, size.y));
            card::bare_value(ui, theme, FormatLabel(image.preview.native_format));
            Panel::hstack()
                .id_salt("preview_info_size")
                .size((Sizing::HUG, Sizing::HUG))
                .gap(theme.space.xs)
                .child_align(Align::v(VAlign::Center))
                .show(ui, |ui| {
                    LabeledValue {
                        label: "Source",
                        value: Bytes::new(image.source_bytes as u64),
                    }
                    .show(ui, theme);
                });
        });
}

/// `"RGBA \u{b7} 8-bit"`-style shorthand for a pixel format: channel layout,
/// then per-channel bit depth.
///
/// Rendered on demand — the strip it feeds records once per preview node per
/// frame, so there is nothing for a `String` to be worth.
#[derive(Clone, Copy, Debug)]
struct FormatLabel(ColorFormat);

impl Display for FormatLabel {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let bits = self.0.sample_type.bits();
        write!(f, "{} \u{b7} {bits}-bit", self.0.channel_count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixel_format_shorthand_reports_channel_layout_and_bit_depth() {
        assert_eq!(
            FormatLabel(ColorFormat::RGBA_U8).to_string(),
            "RGBA \u{b7} 8-bit"
        );
        assert_eq!(
            FormatLabel(ColorFormat::RGB_U16).to_string(),
            "RGB \u{b7} 16-bit"
        );
        assert_eq!(
            FormatLabel(ColorFormat::MONO_F32).to_string(),
            "Mono \u{b7} 32-bit"
        );
    }

    /// Clicking a preview card's image asks the dock for that node's viewer
    /// tab — the canvas's one view-tier request.
    #[test]
    fn clicking_a_preview_card_asks_for_its_viewer_tab() {
        use std::sync::Arc;

        use palantir::DockOperation;

        use crate::core::document::internals::DocumentFixture;
        use crate::core::document::tab_ref::TabRef;
        use crate::core::preview_sink;
        use crate::gui::pane::graph_ui::internals::CanvasHarness;
        use crate::gui::pane::graph_ui::wid;
        use crate::gui::state::preview_store;

        let mut fixture = DocumentFixture::default();
        let node = fixture.add(&preview_sink::func(Arc::default()));
        let mut h = CanvasHarness::new(fixture);
        // The run projection `App` would have filled from a completed run:
        // without a value the card records `Sense::NONE` and swallows the click.
        h.ctx
            .run_state
            .previews_mut()
            .publish(node, preview_store::internals::opaque_image_value());
        h.ctx.run_state.previews_mut().settle(h.ui.ui());
        h.prime(2);
        assert!(h.view_ops.is_empty(), "nothing asked for before the click");

        h.ui.click(wid::preview_image(node));
        let intents = h.frame();

        assert!(
            matches!(
                h.view_ops[..],
                [DockOperation::OpenTab {
                    tab: TabRef::ImageViewer(clicked)
                }] if clicked == node
            ),
            "expected one OpenTab for {node:?}, got {:?}",
            h.view_ops
        );
        assert!(
            intents.is_empty(),
            "opening a viewer is navigation, not a graph edit: {intents:?}"
        );
    }
}
