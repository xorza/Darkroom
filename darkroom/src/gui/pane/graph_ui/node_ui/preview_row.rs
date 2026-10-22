//! A preview node's body content: the value wired into it.
//!
//! Takes the slot an ordinary node's memory footer occupies. A preview holds no
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
use palantir::prelude::*;
use palantir::widget::Shape;
use palantir::{CursorIcon, DockOperation, ImageFit, TextWrap};
use scenarium::NodeId;

use crate::core::document::tab_ref::TabRef;
use crate::gui::pane::graph_ui::node_ui::node_record::NodeRecord;
use crate::gui::pane::graph_ui::node_ui::wid;
use crate::gui::state::preview_store::{DrawablePreview, StoredContent};
use crate::gui::theme::Theme;
use crate::gui::widgets::format::bytes::Bytes;
use crate::gui::widgets::support;
use crate::gui::widgets::support::{CARD_FOOTER_PAD_X, CARD_FOOTER_PAD_Y, LabeledValue, ROW_GAP};

/// Minimum body width for a preview node, canvas-world units. Wider than a
/// normal node's so a thumbnail has somewhere to be; the body still hugs
/// upward if the header or port row needs more.
pub(super) const PREVIEW_MIN_WIDTH: f32 = 240.0;

/// Height of the value area itself, excluding the header, port row, and info
/// footer. Fixed, so a node does not resize as values arrive and vanish —
/// an image letterboxes inside via `Contain`, and a non-image centres its text
/// in the same frame.
const PREVIEW_CONTENT_HEIGHT: f32 = 150.0;

/// What a preview shows when it has never received a value. A run that produced
/// nothing and a preview wired to nothing are the same from here — both mean
/// "there is no value yet", and saying more would be guessing.
const EMPTY_LABEL: &str = "No value yet";

/// Stable id for a preview's value area. Reconstructible from the node, so the
/// canvas-level scan can read last frame's click without a cache.
pub(crate) fn preview_image_wid(node_id: NodeId) -> WidgetId {
    wid::node("preview_image", node_id)
}

/// Draw one preview node's value area, plus the image info footer when there is
/// an image to describe.
pub(super) fn show(ui: &mut Ui, rec: &mut NodeRecord<'_, '_>) {
    let ncx = rec.node;
    let theme = ncx.theme();
    let node = ncx;
    let stored = ncx.graph_ctx.run_state().previews.get(node.id);
    let has_image = stored.and_then(StoredContent::image).is_some();
    let content = Panel::vstack()
        .id(preview_image_wid(node.id))
        .size((Sizing::FILL, Sizing::fixed(PREVIEW_CONTENT_HEIGHT)))
        .padding(Spacing::all(6.0))
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
                // alike; `EMPTY_LABEL` covers having nothing at all.
                let text = match stored.and_then(StoredContent::message) {
                    Some(message) => fmt!(ui, "{message}"),
                    None => ui.intern(EMPTY_LABEL),
                };
                Text::new(text)
                    .style(&support::sized_text(ui, theme.text.label))
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
    if content.left.clicked() {
        rec.out.push_view(DockOperation::OpenTab {
            tab: TabRef::ImageViewer(node.id),
        });
    }
    if let Some(image) = stored.and_then(StoredContent::image) {
        info_row(ui, ncx.theme(), image);
    }
}

/// The image's info footer: resolution, pixel format (channel layout + bit depth), and original
/// source size — styled like a node body's memory footer
/// ([`memory_row`](crate::gui::pane::graph_ui::node_ui::memory_row)), so every read-only fact strip
/// in the app reads as the same kind of thing.
fn info_row(ui: &mut Ui, theme: &Theme, image: &DrawablePreview) {
    Panel::hstack()
        .id_salt("preview_info")
        .size((Sizing::FILL, Sizing::HUG))
        .padding(Spacing::xy(CARD_FOOTER_PAD_X, CARD_FOOTER_PAD_Y))
        .gap(10.0)
        .line_gap(4.0)
        .child_align(Align::v(VAlign::Center))
        // Round only the bottom corners so the strip seats into the body's
        // rounded bottom, the way the header rounds the top.
        .bg(support::footer_bg(theme, theme.card.inner_radius()))
        .show(ui, |ui| {
            let size = image.preview.native_size;
            support::bare_value(ui, theme, format_args!("{}\u{d7}{}", size.x, size.y));
            support::bare_value(ui, theme, FormatLabel(image.preview.native_format));
            Panel::hstack()
                .id_salt("preview_info_size")
                .size((Sizing::HUG, Sizing::HUG))
                .gap(ROW_GAP)
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
            FormatLabel(ColorFormat::L_F32).to_string(),
            "L \u{b7} 32-bit"
        );
    }
}
