//! Runtime presentation resources for the values preview nodes publish.
//!
//! The store is the sole owner of preview-card and viewer textures. An image
//! uploads a small thumbnail immediately, retains its source only until a
//! viewer first needs the full texture, then drops the source after that
//! upload. Non-image values are formatted on receipt and dropped immediately.

use std::collections::HashMap;

use glam::UVec2;
use imaginarium::{ColorFormat, Preview, ProcessingContext};
use lens::Image as LensImage;
use palantir::{Image as AptImage, ImageHandle, Ui};
use scenarium::{DynamicValue, NodeId};

use crate::core::document::Document;

const PREVIEW_TEXTURE_DIM: u32 = 256;
const FULL_TEXTURE_DIM: u32 = 8192;

#[derive(Default, Debug)]
pub(crate) struct PreviewStore {
    /// Each preview node's current value, keyed by the node that published it —
    /// a preview *is* the thing on screen, so its identity is the widget's.
    pub(crate) entries: HashMap<NodeId, StoredContent>,
}

#[derive(Debug)]
pub(crate) enum StoredContent {
    Text(String),
    Image(PreviewImage),
    Error(String),
}

impl StoredContent {
    /// The image behind this value, or `None` for a formatted non-image and
    /// for a value that failed to prepare. The single downcast both readers —
    /// the preview card and the image viewer — go through, so a new variant can't
    /// be handled by one and silently fall through the other.
    pub(crate) fn image(&self) -> Option<&PreviewImage> {
        match self {
            StoredContent::Image(image) => Some(image),
            StoredContent::Text(_) | StoredContent::Error(_) => None,
        }
    }

    /// The text this value shows *in place of* an image: the formatted value
    /// itself, or the reason it isn't renderable. `None` when [`Self::image`]
    /// answered — the two are complementary, so a caller that renders the
    /// image never also has a message to show.
    pub(crate) fn message(&self) -> Option<&str> {
        match self {
            StoredContent::Text(text) | StoredContent::Error(text) => Some(text.as_str()),
            StoredContent::Image(_) => None,
        }
    }
}

#[derive(Debug)]
pub(crate) struct PreviewImage {
    pub(crate) preview: ImageHandle,
    pub(crate) full: FullImage,
    pub(crate) native_size: UVec2,
    pub(crate) native_format: ColorFormat,
    pub(crate) source_bytes: usize,
}

#[derive(Debug)]
pub(crate) enum FullImage {
    Deferred(DynamicValue),
    Resident(ImageHandle),
    Failed(String),
}

#[derive(Debug)]
struct PreparedImage {
    raster: AptImage,
    native_size: UVec2,
    native_format: ColorFormat,
}

impl PreviewStore {
    /// Store one preview node's freshly published value, replacing whatever it
    /// was showing.
    ///
    /// No retention filter on the way in, unlike [`Self::ingest_preview`]: a value
    /// arrives keyed by the node that published it, and a node that published
    /// exists. `reconcile` still drops it once that stops being true.
    pub(crate) fn ingest_preview(&mut self, ui: &Ui, node_id: NodeId, value: DynamicValue) {
        self.entries.insert(node_id, prepare_content(ui, value));
    }

    /// Release every presentation resource the document no longer retains and
    /// upload the full-resolution texture each *visible* viewer needs.
    ///
    /// Run unconditionally once a frame. Both halves are already idempotent —
    /// `materialize_full` returns at once for anything not still deferred, and
    /// the retain is a lookup per stored value — so asking every frame costs a
    /// pass over the open viewer tabs (normally none) and the handful of nodes
    /// holding a value. That is cheaper than the bookkeeping a request flag
    /// needed: the store is written *outside* the frame too (`ingest_preview`
    /// runs from the worker drain in `App::update`), so a flag had to survive
    /// until the next frame rather than resetting with it, and every edit path
    /// that moved the retained set had to remember to raise it.
    pub(crate) fn reconcile(&mut self, ui: &Ui, document: &Document) {
        // Scoped to what the coming record pass draws: a full texture is up
        // to 8192² RGBA8, so uploading one for a viewer tab stacked behind
        // another in the same pane would cost hundreds of MB unseen.
        for node_id in document.visible_viewer_nodes() {
            self.materialize_full(ui, node_id);
        }
        // A preview's own node is its retention: delete the node and the value
        // it was showing has nothing left to draw it.
        self.entries
            .retain(|node_id, _| document.holds_preview_node(*node_id));
    }

    fn materialize_full(&mut self, ui: &Ui, node_id: NodeId) {
        if let Some(StoredContent::Image(image)) = self.entries.get_mut(&node_id) {
            image.materialize_full(ui);
        }
    }
}

impl PreviewImage {
    /// Upload the deferred source at full resolution and drop it, in place —
    /// a no-op once `full` is already `Resident` or `Failed`.
    ///
    /// The new `full` is built into a local before it's assigned, so the read
    /// of the deferred value is finished by then. That's what lets this take
    /// `&mut self`: no placeholder variant has to be parked in the slot while
    /// the old value is owned, and no frame can observe the entry mid-swap.
    fn materialize_full(&mut self, ui: &Ui) {
        let FullImage::Deferred(value) = &self.full else {
            return;
        };
        let resolved =
            match as_image(value).and_then(|image| prepare_image(image, FULL_TEXTURE_DIM)) {
                Ok(prepared) => match ui.register_image(prepared.raster) {
                    Ok(handle) => FullImage::Resident(handle),
                    Err(error) => FullImage::Failed(error.to_string()),
                },
                Err(message) => FullImage::Failed(message),
            };
        self.full = resolved;
    }
}

fn prepare_content(ui: &Ui, value: DynamicValue) -> StoredContent {
    // Scoped so the borrow ends before `value` moves into `Deferred` below.
    // One downcast serves both the "is this an image at all?" test and the
    // conversion — a non-image renders as text, an image that fails to
    // convert as an error, and those are different outcomes.
    let prepared = {
        let Some(image) = value.as_custom::<LensImage>() else {
            return StoredContent::Text(value.to_string());
        };
        prepare_image(image, PREVIEW_TEXTURE_DIM)
    };
    match prepared {
        Ok(prepared) => match ui.register_image(prepared.raster) {
            Ok(preview) => {
                let source_bytes = value.ram_usage().total();
                StoredContent::Image(PreviewImage {
                    preview,
                    full: FullImage::Deferred(value),
                    native_size: prepared.native_size,
                    native_format: prepared.native_format,
                    source_bytes,
                })
            }
            Err(error) => StoredContent::Error(error.to_string()),
        },
        Err(message) => StoredContent::Error(message),
    }
}

/// The image behind a published value, or the message to show in its place.
fn as_image(value: &DynamicValue) -> Result<&LensImage, String> {
    value
        .as_custom::<LensImage>()
        .ok_or_else(|| "value is not an image".to_owned())
}

fn prepare_image(image: &LensImage, max_dim: u32) -> Result<PreparedImage, String> {
    let cpu = image
        .buffer
        .make_cpu(&ProcessingContext::cpu_only())
        .map_err(|e| format!("could not read image pixels: {e}"))?;
    let native_size = UVec2::new(cpu.desc().width as u32, cpu.desc().height as u32);
    if native_size.x == 0 || native_size.y == 0 {
        return Err("image is empty".to_owned());
    }
    let native_format = cpu.desc().color_format;
    let target = capped_target(native_size, max_dim);
    let rgba = Preview::new(target.x as usize, target.y as usize).to_rgba8(&cpu);
    let desc = rgba.desc();
    assert_eq!(desc.color_format, ColorFormat::RGBA_U8);
    let pixels = rgba.into_bytes();
    assert_eq!(pixels.len(), desc.row_bytes() * desc.height);
    Ok(PreparedImage {
        raster: AptImage::from_rgba8(target.x, target.y, pixels),
        native_size,
        native_format,
    })
}

fn capped_target(native: UVec2, max_dim: u32) -> UVec2 {
    let scale = (max_dim as f32 / native.x.max(native.y) as f32).min(1.0);
    UVec2::new(
        (native.x as f32 * scale).round().max(1.0) as u32,
        (native.y as f32 * scale).round().max(1.0) as u32,
    )
}

#[cfg(test)]
mod tests;
