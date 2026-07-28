//! A preview node's body content: the value wired into it.
//!
//! Takes the slot an ordinary node's memory footer occupies. A preview holds no
//! cached output — it has no output at all — so there is nothing for that
//! readout to report, and the value is the only thing worth the space.
//!
//! Not a floating widget with wiring of its own: it is a row inside a normal
//! node body, so selection, dragging, the breaker, and the input wire all come
//! from the node machinery unchanged.

use imaginarium::ColorFormat;
use palantir::{
    Align, Configure, CursorIcon, ImageFit, Justify, Panel, Sense, Shape, Sizing, Spacing, Text,
    TextWrap, Ui, VAlign, WidgetId,
};
use scenarium::NodeId;

use crate::gui::format::fmt_bytes;
use crate::gui::node::RecordCtx;
use crate::gui::preview_store::{PreviewImage, StoredContent};
use crate::gui::scene::SceneNode;
use crate::gui::theme::Theme;
use crate::gui::widgets::support::{
    CARD_FOOTER_PAD_X, CARD_FOOTER_PAD_Y, footer_background, labeled_value, mono_text, sized_text,
};

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
    WidgetId::from_hash(("graph.node.preview_image", node_id))
}

/// Draw one preview node's value area, plus the image info footer when there is
/// an image to describe.
pub(super) fn preview_row(ui: &mut Ui, rcx: RecordCtx<'_>, node: &SceneNode) {
    let stored = rcx.run_state.previews.entries.get(&node.id);
    let has_image = stored.and_then(StoredContent::image).is_some();
    let content = Panel::vstack()
        .id(preview_image_wid(node.id))
        .size((Sizing::FILL, Sizing::fixed(PREVIEW_CONTENT_HEIGHT)))
        .padding(Spacing::all(6.0))
        .child_align(Align::CENTER)
        .justify(Justify::Center)
        // Only an image opens a viewer, so only an image is clickable.
        .sense(if has_image { Sense::CLICK } else { Sense::NONE })
        .show(ui, |ui| match stored.and_then(StoredContent::image) {
            Some(image) => {
                ui.add_shape(Shape::image(image.preview.clone()).fit(ImageFit::Contain));
            }
            None => {
                // `message` is complementary to `image`, so this covers a
                // formatted non-image value and a value that failed to prepare
                // alike; `EMPTY_LABEL` covers having nothing at all.
                let text = stored
                    .and_then(StoredContent::message)
                    .unwrap_or(EMPTY_LABEL);
                Text::new(text)
                    .style(&sized_text(ui, 11.0))
                    .text_wrap(TextWrap::Wrap)
                    .show(ui);
            }
        })
        .response
        .snapshot();
    if content.hovered {
        ui.set_cursor(CursorIcon::Pointer);
    }
    if let Some(image) = stored.and_then(StoredContent::image) {
        info_row(ui, rcx.theme, image);
    }
}

/// The image's info footer: resolution, pixel format (channel layout + bit
/// depth), and original source size — styled like a node body's memory footer
/// ([`crate::gui::node::memory_row`]), so every read-only fact strip in the app
/// reads as the same kind of thing.
fn info_row(ui: &mut Ui, theme: &Theme, image: &PreviewImage) {
    Panel::hstack()
        .id_salt("preview_info")
        .size((Sizing::FILL, Sizing::HUG))
        .padding(Spacing::xy(CARD_FOOTER_PAD_X, CARD_FOOTER_PAD_Y))
        .gap(10.0)
        .line_gap(4.0)
        .child_align(Align::v(VAlign::Center))
        // Round only the bottom corners so the strip seats into the body's
        // rounded bottom, the way the header rounds the top.
        .background(footer_background(theme, theme.card_inner_radius()))
        .show(ui, |ui| {
            bare_value(
                ui,
                format!("{}\u{d7}{}", image.native_size.x, image.native_size.y),
            );
            bare_value(ui, format_label(image.native_format));
            Panel::hstack()
                .id_salt("preview_info_size")
                .size((Sizing::HUG, Sizing::HUG))
                .gap(4.0)
                .child_align(Align::v(VAlign::Center))
                .show(ui, |ui| {
                    labeled_value(ui, theme, "Source", fmt_bytes(image.source_bytes as u64));
                });
        });
}

/// `"RGBA \u{b7} 8-bit"`-style shorthand for a pixel format: channel layout,
/// then per-channel bit depth.
fn format_label(format: ColorFormat) -> String {
    let bits = format.channel_size.byte_count() as u32 * 8;
    format!("{} \u{b7} {bits}-bit", format.channel_count)
}

/// A bare mono-styled value with no label — used where the value's own shape
/// (`1920×1080`, `RGBA · 8-bit`) already says what it is.
fn bare_value(ui: &mut Ui, text: String) {
    Text::new(text).style(&mono_text(ui, 10.5)).show(ui);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_label_reports_channel_layout_and_bit_depth() {
        assert_eq!(format_label(ColorFormat::RGBA_U8), "RGBA \u{b7} 8-bit");
        assert_eq!(format_label(ColorFormat::RGB_U16), "RGB \u{b7} 16-bit");
        assert_eq!(format_label(ColorFormat::L_F32), "L \u{b7} 32-bit");
    }
}
