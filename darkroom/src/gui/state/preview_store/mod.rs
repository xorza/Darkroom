//! Runtime presentation resources for the values preview nodes publish.
//!
//! The store is the sole owner of preview-card and viewer textures. An image
//! uploads a small thumbnail immediately and holds its source until a viewer
//! first asks for the full texture — [`PreviewImage::full`] uploads it there
//! and then, in the pass that draws it, and drops the source as it goes.
//! Non-image values are formatted on receipt and dropped immediately.

pub(crate) mod error;

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;

use glam::UVec2;
use imaginarium::{ColorFormat, Preview, ProcessingContext};
use lens::Image as LensImage;
use palantir::{Image as AptImage, ImageHandle, Ui};
use scenarium::{DynamicValue, NodeId};

use crate::core::document::Document;
use crate::gui::state::preview_store::error::PreviewImageError;

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
    Error(PreviewImageError),
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
    /// `Cow` because the two sources differ: a formatted value is already the
    /// string it shows, while a failure is a typed error rendered on the way
    /// out — and only a card sitting on a broken value pays for that.
    pub(crate) fn message(&self) -> Option<Cow<'_, str>> {
        match self {
            StoredContent::Text(text) => Some(Cow::Borrowed(text.as_str())),
            StoredContent::Error(error) => Some(Cow::Owned(error.to_string())),
            StoredContent::Image(_) => None,
        }
    }
}

#[derive(Debug)]
pub(crate) struct PreviewImage {
    pub(crate) preview: ImageHandle,
    /// The full-resolution texture, or the source still waiting to become one.
    /// Read only through [`Self::full`], which is what resolves the wait.
    full: RefCell<FullImage>,
    pub(crate) native_size: UVec2,
    pub(crate) native_format: ColorFormat,
    pub(crate) source_bytes: usize,
}

/// What a stored image holds for a viewer: the published value until one asks
/// for it at full resolution, the texture from then on, or why there will
/// never be one.
#[derive(Debug)]
enum FullImage {
    Pending(DynamicValue),
    Resident(ImageHandle),
    Failed(PreviewImageError),
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

    /// Release every presentation resource the document no longer retains.
    ///
    /// Run unconditionally once a frame: the retain is a lookup per stored
    /// value, over the handful of nodes holding one. That is cheaper than the
    /// bookkeeping a dirty flag needed — the store is written *outside* the
    /// frame too (`ingest_preview` runs from the worker drain), so a flag had
    /// to survive until the next frame rather than resetting with it, and
    /// every edit path that moved the retained set had to remember to raise
    /// it.
    ///
    /// A preview's own node is its retention: delete the node and the value it
    /// was showing has nothing left to draw it. The shared liveness rule
    /// narrowed to preview nodes — see `Document::holds_preview_node` for why
    /// this cache is stricter than the others.
    pub(crate) fn reconcile(&mut self, document: &Document) {
        self.entries
            .retain(|node_id, _| document.holds_preview_node(*node_id));
    }
}

impl PreviewImage {
    /// The full-resolution texture, uploading the published value on the first
    /// ask and reusing the result from then on. The source is dropped as it
    /// becomes a texture, so an image is never held twice.
    ///
    /// **Lazy, not deferred.** The upload is up to 8192² RGBA8 — hundreds of MB
    /// — and only a viewer pane ever wants one; a preview card draws the
    /// thumbnail. Asking is therefore the whole scoping rule: a pane records
    /// only when its tab is the visible one, so a viewer stacked behind
    /// another uploads nothing, and one that becomes visible mid-record gets
    /// its texture in the pass that draws it rather than a frame later.
    ///
    /// `&self` because that ask comes from the record pass, which holds the
    /// run projection immutably (see [`AppCtx`](crate::gui::app::ctx::AppCtx)).
    /// The borrow is taken, resolved and dropped inside this call, so nothing
    /// can observe the cell mid-upload or hold it across a frame.
    ///
    /// Both arms are read *out* rather than borrowed from, since what they
    /// name lives behind that cell: the handle clone is a refcount bump on the
    /// texture the store still owns, and only a broken image copies its
    /// reason.
    pub(crate) fn full(&self, ui: &Ui) -> Result<ImageHandle, PreviewImageError> {
        let mut full = self.full.borrow_mut();
        if let FullImage::Pending(value) = &*full {
            *full = Self::upload(ui, value);
        }
        match &*full {
            FullImage::Resident(handle) => Ok(handle.clone()),
            FullImage::Failed(error) => Err(error.clone()),
            FullImage::Pending(_) => unreachable!("the pending source was just uploaded"),
        }
    }

    /// Register the value's pixels at full resolution. Every failure — a value
    /// that isn't an image, pixels that won't convert, a registration the
    /// renderer refuses — lands as `Failed`, which is terminal: it is stored,
    /// so a broken image is diagnosed once rather than retried every frame.
    fn upload(ui: &Ui, value: &DynamicValue) -> FullImage {
        match as_image(value).and_then(|image| prepare_image(image, FULL_TEXTURE_DIM)) {
            Ok(prepared) => match ui.register_image(prepared.raster) {
                Ok(handle) => FullImage::Resident(handle),
                Err(error) => FullImage::Failed(error.into()),
            },
            Err(error) => FullImage::Failed(error),
        }
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
                    full: RefCell::new(FullImage::Pending(value)),
                    native_size: prepared.native_size,
                    native_format: prepared.native_format,
                    source_bytes,
                })
            }
            Err(error) => StoredContent::Error(error.into()),
        },
        Err(error) => StoredContent::Error(error),
    }
}

/// The image behind a published value, or why there isn't one.
fn as_image(value: &DynamicValue) -> Result<&LensImage, PreviewImageError> {
    value
        .as_custom::<LensImage>()
        .ok_or(PreviewImageError::NotAnImage)
}

fn prepare_image(image: &LensImage, max_dim: u32) -> Result<PreparedImage, PreviewImageError> {
    let cpu = image
        .buffer
        .make_cpu(&ProcessingContext::cpu_only())
        .map_err(|e| PreviewImageError::Pixels(e.to_string()))?;
    let native_size = UVec2::new(cpu.desc().width as u32, cpu.desc().height as u32);
    if native_size.x == 0 || native_size.y == 0 {
        return Err(PreviewImageError::Empty);
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
pub(crate) mod internals {
    use imaginarium::{Image as RawImage, ImageBuffer, ImageDesc};

    use super::*;

    impl PreviewImage {
        /// Whether the full-resolution texture has been uploaded yet.
        ///
        /// The one question a test can't ask through [`PreviewImage::full`],
        /// since asking that *is* what uploads it.
        pub(crate) fn is_full_resident(&self) -> bool {
            matches!(&*self.full.borrow(), FullImage::Resident(_))
        }
    }

    /// The smallest opaque image a preview card will render — 2×1 RGBA8.
    ///
    /// Publishing a value is what makes a card clickable at all (it records
    /// `Sense::NONE` without one), so every test about that chip, or about the
    /// viewer tab it opens, starts by ingesting this.
    pub(crate) fn opaque_image_value() -> DynamicValue {
        let desc = ImageDesc::new(2, 1, ColorFormat::RGBA_U8);
        let raw = RawImage::new_with_data(desc, vec![255; desc.row_bytes()]).unwrap();
        DynamicValue::from_custom(LensImage::from(ImageBuffer::from_cpu(raw)))
    }
}

#[cfg(test)]
mod tests;
