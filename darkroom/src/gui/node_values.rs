//! Render-ready views of a node's runtime input/output values, plus the
//! conversion from a worker [`ArgumentValues`] reply.
//!
//! Trivial values (numbers/strings/enums/paths) format straight to text;
//! image values upload the thumbnail the worker resolved before replying
//! as a GUI texture, and additionally keep the full value (an `Arc` clone
//! sharing the worker cache's buffer) so the image-viewer tab can show it
//! at full resolution on demand. The owning per-node store + fetch
//! coordination lives in [`crate::gui::run_state::RunState`].

use aperture::{
    Background, Configure, Corners, Image as PalImage, ImageFilter, ImageFit, ImageHandle, Panel,
    Sense, Shape, Sizing, Spacing, Stroke, Text, TextWrap, Ui, WidgetId,
};
use imaginarium::{ColorFormat, Image as RawImage};
use lens::Image as LensImage;
use scenarium::data::DynamicValue;
use scenarium::execution::ArgumentValues;

use crate::gui::theme::Theme;
use crate::gui::widgets::support::sized_text;

/// One port's runtime value, ready to render: a formatted text cell plus
/// an optional preview thumbnail (already uploaded as a texture).
#[derive(Debug, Default)]
pub(crate) struct PortValueView {
    pub(crate) text: String,
    pub(crate) preview: Option<ImageHandle>,
    /// The port's full runtime value, kept (a cheap `Arc` clone of the
    /// worker's cached value) so clicking the thumbnail can show the image
    /// at full resolution without a worker round-trip. `Some` only for
    /// image values; dropped with the view on the next run.
    pub(crate) full: Option<DynamicValue>,
}

/// A node's runtime values, indexed by port. `inputs[i]` / `outputs[i]`
/// line up with the scene's port arrays by index.
#[derive(Debug, Default)]
pub(crate) struct NodeValueView {
    pub(crate) inputs: Vec<PortValueView>,
    pub(crate) outputs: Vec<PortValueView>,
}

/// Convert a worker reply into a render-ready view: format every value to
/// text and upload any image previews as GPU textures. Each preview's
/// [`ImageHandle`] is owned by the returned view, so replacing the view on
/// a re-run drops the old handles and frees their textures.
pub(crate) fn build_view(ui: &Ui, values: ArgumentValues) -> NodeValueView {
    let inputs = values
        .inputs
        .iter()
        .map(|value| match value {
            Some(value) => port_view(ui, value),
            None => PortValueView {
                text: "-".to_string(),
                ..Default::default()
            },
        })
        .collect();
    let outputs = values
        .outputs
        .iter()
        .map(|value| port_view(ui, value))
        .collect();
    NodeValueView { inputs, outputs }
}

fn port_view(ui: &Ui, value: &DynamicValue) -> PortValueView {
    // The imaginarium-backed `Image` (the astro nodes' currency too) parks its
    // preview as an RGBA_U8 image; downcast and upload it.
    let image = value.as_custom::<LensImage>();
    let preview = image
        .and_then(LensImage::take_preview)
        .and_then(|preview| upload_preview(ui, preview));
    PortValueView {
        text: value.to_string(),
        preview,
        full: image.is_some().then(|| value.clone()),
    }
}

/// Upload an `RGBA_U8` preview as an aperture texture. `None` for an
/// unexpected format or a degenerate buffer.
fn upload_preview(ui: &Ui, image: RawImage) -> Option<ImageHandle> {
    Some(ui.register_image(rgba8_image(image)?))
}

/// Draw an image `handle` as a framed, aspect-preserving thumbnail capped at
/// `max_width` (never upscaled past the image's own width). A dark image on a
/// dark surface needs the hairline edge to read as a framed object; the frame
/// brightens on hover as a click affordance. The panel senses clicks so a
/// caller can poll `wid` from last frame's responses to open the full-resolution
/// viewer. Shared by the inspector panel and the Preview node's inline view.
///
/// The rounded corners come from a `Shape::WindowedRect` over the image (wedges
/// filled with the surface, stroke on the boundary), not `clip_rounded` — same
/// look without the stencil-mask pass. No-op for a degenerate (zero-dimension)
/// handle. `margin` insets the thumbnail from its neighbors (the inspector
/// passes none; the Preview node's inline view gives it breathing room).
pub(crate) fn image_preview(
    ui: &mut Ui,
    theme: &Theme,
    wid: WidgetId,
    handle: &ImageHandle,
    max_width: f32,
    margin: Spacing,
) {
    let size = handle.size();
    if size.x == 0 || size.y == 0 {
        return;
    }
    let aspect = size.x as f32 / size.y as f32;
    let w = max_width.min(size.x as f32);
    let h = w / aspect;
    let stroke_alpha = if ui.response_for(wid).hovered {
        0.6
    } else {
        0.18
    };
    Panel::vstack()
        .id(wid)
        .size((Sizing::Fixed(w), Sizing::Fixed(h)))
        .margin(margin)
        .sense(Sense::CLICK)
        .show(ui, |ui| {
            ui.add_shape(
                Shape::image(handle.clone())
                    .fit(ImageFit::Contain)
                    .filter(ImageFilter::Linear),
            );
            ui.add_shape(Shape::WindowedRect {
                local_rect: None,
                corners: Corners::all(4.0),
                fill: theme.colors.node_fill.into(),
                stroke: Stroke::solid(theme.colors.text_muted.with_alpha(stroke_alpha), 1.0),
            });
        });
}

/// Draw the Preview node's image slot: fixed width `w`, height derived from
/// the held image's aspect ratio and capped at `max_h` — a landscape image
/// renders near its natural shape, a tall one is capped rather than growing
/// the node without bound. Absent a `handle` (nothing fetched yet), the slot
/// is an empty `w`×`max_h` placeholder. A present `handle` is drawn
/// `Contain`ed (aspect-preserving, letterboxed on the recessed backdrop) —
/// only pillarboxes once its natural height would exceed `max_h`. Clickable,
/// and brightens its border on hover, like [`image_preview`].
pub(crate) fn preview_slot(
    ui: &mut Ui,
    theme: &Theme,
    wid: WidgetId,
    handle: Option<&ImageHandle>,
    w: f32,
    max_h: f32,
    margin: Spacing,
) {
    let h = match handle.map(|h| h.size()) {
        Some(size) if size.x > 0 && size.y > 0 => (w * size.y as f32 / size.x as f32).min(max_h),
        _ => max_h,
    };
    let stroke_alpha = if ui.response_for(wid).hovered {
        0.6
    } else {
        0.18
    };
    Panel::vstack()
        .id(wid)
        .size((Sizing::Fixed(w), Sizing::Fixed(h)))
        .margin(margin)
        .sense(Sense::CLICK)
        // Recessed fill so the (possibly letterboxed / empty) slot reads as an
        // inset image area, distinct from the node body.
        .background(Background::rounded(
            theme.colors.canvas_bg,
            Corners::all(4.0),
        ))
        .show(ui, |ui| {
            if let Some(handle) = handle {
                ui.add_shape(
                    Shape::image(handle.clone())
                        .fit(ImageFit::Contain)
                        .filter(ImageFilter::Linear),
                );
            }
            // Rounds the (square) image corners into the recessed backdrop and
            // draws the border; wedge fill matches the backdrop so it blends.
            ui.add_shape(Shape::WindowedRect {
                local_rect: None,
                corners: Corners::all(4.0),
                fill: theme.colors.canvas_bg.into(),
                stroke: Stroke::solid(theme.colors.text_muted.with_alpha(stroke_alpha), 1.0),
            });
        });
}

/// Draw the Preview node's fallback for a fetched value that isn't an image:
/// its formatted text in the same recessed-panel family as [`preview_slot`],
/// but sized to content (`Sizing::Hug` height) rather than the image box's
/// fixed footprint — a scalar/string value shouldn't reserve unused
/// image-sized space. Not a click target: there's no full-resolution view to
/// open for a non-image value.
pub(crate) fn preview_text(
    ui: &mut Ui,
    theme: &Theme,
    wid: WidgetId,
    text: &str,
    w: f32,
    margin: Spacing,
) {
    Panel::vstack()
        .id(wid)
        .size((Sizing::Fixed(w), Sizing::Hug))
        .margin(margin)
        .padding(Spacing::all(6.0))
        .background(Background::rounded(
            theme.colors.canvas_bg,
            Corners::all(4.0),
        ))
        .show(ui, |ui| {
            Text::new(text.to_owned())
                .style(sized_text(ui, 12.0))
                .text_wrap(TextWrap::Wrap)
                .show(ui);
        });
}

/// Reinterpret a packed `RGBA_U8` imaginarium image as an uploadable
/// aperture image — the shared tail of every image→texture path (the
/// preview thumbnails here, the viewer's full-resolution render).
/// `None` for another format or a padded stride; imaginarium images are
/// tightly packed, so for RGBA_U8 input `Some` is the norm and the
/// bytes move without a repack.
pub(crate) fn rgba8_image(image: RawImage) -> Option<PalImage> {
    let desc = image.desc;
    if desc.color_format != ColorFormat::RGBA_U8 {
        return None;
    }
    let pixels = image.into_bytes();
    if pixels.len() != desc.row_bytes() * desc.height {
        return None;
    }
    Some(PalImage::from_rgba8(
        desc.width as u32,
        desc.height as u32,
        pixels,
    ))
}
