//! A preview node's body content: the value wired into it.
//!
//! Takes the slot an ordinary node's memory footer occupies. A preview holds no
//! cached output — it has no output at all — so there is nothing for that
//! readout to report, and the value is the only thing worth the space.
//!
//! Unlike the pinned-output card ([`crate::gui::canvas::pin_preview`]) this is
//! not a floating widget with wiring of its own: it is a row inside a normal
//! node body, so selection, dragging, the breaker, and the input wire all come
//! from the node machinery unchanged.

use palantir::{
    Align, Configure, ImageFit, Justify, Panel, Shape, Sizing, Spacing, Text, TextWrap, Ui,
};

use crate::gui::canvas::pin_preview::info_row;
use crate::gui::node::RecordCtx;
use crate::gui::pinned_output::StoredContent;
use crate::gui::scene::SceneNode;
use crate::gui::widgets::support::sized_text;

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

/// Draw one preview node's value area, plus the image info footer when there is
/// an image to describe.
pub(super) fn preview_row(ui: &mut Ui, rcx: RecordCtx<'_>, node: &SceneNode) {
    let stored = rcx.run_state.pinned_outputs.previews.get(&node.id);
    Panel::vstack()
        .id_salt("preview_content")
        .size((Sizing::FILL, Sizing::fixed(PREVIEW_CONTENT_HEIGHT)))
        .padding(Spacing::all(6.0))
        .child_align(Align::CENTER)
        .justify(Justify::Center)
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
        });
    if let Some(image) = stored.and_then(StoredContent::image) {
        info_row(ui, rcx.theme, rcx.theme.card_inner_radius(), image);
    }
}
