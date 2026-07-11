//! Render-ready views of a node's runtime input/output values, plus the
//! conversion from a worker [`ArgumentValues`] reply.
//!
//! Trivial values (numbers/strings/enums/paths) format straight to text;
//! image values upload the thumbnail the worker resolved before replying
//! as a GUI texture, and additionally keep the full value (an `Arc` clone
//! sharing the worker cache's buffer) so the image-viewer tab can show it
//! at full resolution on demand. The owning per-node store + fetch
//! coordination lives in [`crate::gui::run_state::RunState`].

use aperture::{Image as PalImage, ImageHandle, Ui};
use imaginarium::{ColorFormat, Image as RawImage};
use lens::Image as LensImage;
use scenarium::data::DynamicValue;
use scenarium::execution::ArgumentValues;

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
