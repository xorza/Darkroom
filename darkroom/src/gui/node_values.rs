//! Render-ready views of a node's runtime input/output values, plus the
//! conversion from a worker [`ArgumentValues`] reply.
//!
//! Trivial values (numbers/strings/enums/paths) format straight to text;
//! image values are reduced to the preview the worker resolved before
//! replying and uploaded here as a GUI texture, so the full buffers never
//! cross into the editor. The owning per-node store + fetch coordination
//! lives in [`crate::gui::run_state::RunState`].

use imaginarium::{ColorFormat, Image as RawImage};
use lens::Image as LensImage;
use palantir::{Image as PalImage, ImageHandle, Ui};
use scenarium::data::DynamicValue;
use scenarium::execution::ArgumentValues;

/// One port's runtime value, ready to render: a formatted text cell plus
/// an optional preview thumbnail (already uploaded as a texture).
#[derive(Debug, Default)]
pub(crate) struct PortValueView {
    pub(crate) text: String,
    pub(crate) preview: Option<ImageHandle>,
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
                preview: None,
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
    let preview = value
        .as_custom::<LensImage>()
        .and_then(LensImage::take_preview)
        .and_then(|preview| upload_preview(ui, preview));
    PortValueView {
        text: value.to_string(),
        preview,
    }
}

/// Upload an `RGBA_U8` preview as a palantir texture. Repacks rows only if
/// a padded stride ever shows up (the preview path produces a packed
/// stride, so this is normally a straight move). `None` for an unexpected
/// format or a degenerate buffer.
fn upload_preview(ui: &Ui, image: RawImage) -> Option<ImageHandle> {
    let desc = image.desc;
    if desc.color_format != ColorFormat::RGBA_U8 {
        return None;
    }
    let row = desc.width * 4;
    // A sub-row stride would overrun on the per-row slice below — reject
    // rather than panic on a malformed image from the worker.
    if desc.stride < row {
        return None;
    }
    let bytes = image.into_bytes();
    let pixels = if desc.stride == row {
        bytes
    } else {
        let mut packed = Vec::with_capacity(row * desc.height);
        for y in 0..desc.height {
            packed.extend_from_slice(&bytes[y * desc.stride..y * desc.stride + row]);
        }
        packed
    };
    if pixels.len() != row * desc.height {
        return None;
    }
    let handle = ui.register_image(PalImage::from_rgba8(
        desc.width as u32,
        desc.height as u32,
        pixels,
    ));
    Some(handle)
}
