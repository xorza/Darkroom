//! Render-ready views of a node's runtime input/output values, plus the
//! conversion from a worker [`ArgumentValues`] reply.
//!
//! Trivial values (numbers/strings/enums/paths) format straight to text;
//! image values are reduced to the preview the worker resolved before
//! replying and uploaded here as a GUI texture, so the full buffers never
//! cross into the editor. The owning per-node store + fetch coordination
//! lives in [`crate::run_state::RunState`].

use imaginarium::{ColorFormat, Image as RawImage};
use lens::Image as LensImage;
use palantir::{Image as PalImage, ImageHandle, Ui};
use scenarium::data::DynamicValue;
use scenarium::execution::ArgumentValues;
use scenarium::prelude::NodeId;

use crate::run_state::{RunId, ValueRequest};

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
/// text and upload any image previews as textures (keyed by `run_id` so a
/// re-run replaces rather than collides with the prior thumbnail).
pub(crate) fn build_view(ui: &Ui, request: ValueRequest, values: ArgumentValues) -> NodeValueView {
    let ValueRequest { node_id, run_id } = request;
    let inputs = values
        .inputs
        .iter()
        .enumerate()
        .map(|(idx, value)| match value {
            Some(value) => port_view(ui, node_id, run_id, "in", idx, value),
            None => PortValueView {
                text: "-".to_string(),
                preview: None,
            },
        })
        .collect();
    let outputs = values
        .outputs
        .iter()
        .enumerate()
        .map(|(idx, value)| port_view(ui, node_id, run_id, "out", idx, value))
        .collect();
    NodeValueView { inputs, outputs }
}

fn port_view(
    ui: &Ui,
    node_id: NodeId,
    run_id: RunId,
    side: &str,
    idx: usize,
    value: &DynamicValue,
) -> PortValueView {
    let preview = value
        .as_custom::<LensImage>()
        .and_then(LensImage::take_preview)
        .and_then(|preview| upload_preview(ui, node_id, run_id, side, idx, preview));
    PortValueView {
        text: value.to_string(),
        preview,
    }
}

/// Upload an `RGBA_U8` preview as a palantir texture. Repacks rows only if
/// a padded stride ever shows up (the preview path produces a packed
/// stride, so this is normally a straight move). `None` for an unexpected
/// format or a degenerate buffer.
fn upload_preview(
    ui: &Ui,
    node_id: NodeId,
    run_id: RunId,
    side: &str,
    idx: usize,
    image: RawImage,
) -> Option<ImageHandle> {
    let desc = image.desc;
    if desc.color_format != ColorFormat::RGBA_U8 {
        return None;
    }
    let row = desc.width * 4;
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
    let key = ("darkroom.inspector.preview", node_id, side, idx, run_id);
    let handle = ui.register_image(
        key,
        PalImage::from_rgba8(desc.width as u32, desc.height as u32, pixels),
    );
    Some(handle)
}
