//! Render-ready views of a node's runtime input/output values, plus the
//! conversion from a worker [`ArgumentValues`] reply.
//!
//! Every value formats straight to text; image values additionally keep the
//! full value (an `Arc` clone sharing the worker cache's buffer) so the
//! image-viewer tab can show it at full resolution on demand. The owning
//! per-node store + fetch coordination lives in
//! [`crate::gui::run_state::RunState`].

use aperture::Image as PalImage;
use imaginarium::{ColorFormat, Image as RawImage};
use lens::Image as LensImage;
use scenarium::data::DynamicValue;
use scenarium::execution::ArgumentValues;

/// One port's runtime value, ready to render: a formatted text cell.
#[derive(Debug, Default)]
pub(crate) struct PortValueView {
    pub(crate) text: String,
    /// The port's full runtime value, kept (a cheap `Arc` clone of the
    /// worker's cached value) so the image-viewer tab can show it at full
    /// resolution without a worker round-trip. `Some` only for image values;
    /// dropped with the view on the next run.
    pub(crate) full: Option<DynamicValue>,
}

/// A node's runtime values, indexed by port. `inputs[i]` / `outputs[i]`
/// line up with the scene's port arrays by index.
#[derive(Debug, Default)]
pub(crate) struct NodeValueView {
    pub(crate) inputs: Vec<PortValueView>,
    pub(crate) outputs: Vec<PortValueView>,
}

/// Convert a worker reply into a render-ready view: format every value to text.
pub(crate) fn build_view(values: ArgumentValues) -> NodeValueView {
    let inputs = values
        .inputs
        .iter()
        .map(|value| match value {
            Some(value) => port_view(value),
            None => PortValueView {
                text: "-".to_string(),
                ..Default::default()
            },
        })
        .collect();
    let outputs = values.outputs.iter().map(port_view).collect();
    NodeValueView { inputs, outputs }
}

fn port_view(value: &DynamicValue) -> PortValueView {
    let image = value.as_custom::<LensImage>();
    PortValueView {
        text: value.to_string(),
        full: image.is_some().then(|| value.clone()),
    }
}

/// Reinterpret a packed `RGBA_U8` imaginarium image as an uploadable
/// aperture image — the image-viewer's full-resolution render path.
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
