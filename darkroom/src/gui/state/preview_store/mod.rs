//! Runtime presentation resources for the values preview nodes publish.
//!
//! The store is the sole owner of preview-card and viewer textures. An image
//! uploads a small thumbnail immediately and holds its source until a viewer
//! first asks for the full texture — [`PreviewImage::full`] uploads it there
//! and then, in the pass that draws it, and drops the source as it goes.
//! Non-image values are formatted on receipt and dropped immediately.

pub(crate) mod error;

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::num::NonZeroU32;

use glam::UVec2;
use imaginarium::{ColorFormat, Image as CpuImage, Preview};
use lens::Image as LensImage;
use palantir::{Image as Raster, ImageHandle, Ui};
use scenarium::{DynamicValue, NodeId};

use crate::core::document::Document;
use crate::gui::state::preview_store::error::PreviewImageError;

/// Longest edge of a preview card's thumbnail — the one size darkroom picks
/// for itself. A viewer's texture names no figure at all; see
/// [`prepare_drawable`].
const PREVIEW_TEXTURE_DIM: NonZeroU32 = NonZeroU32::new(256).unwrap();

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
    pub(crate) fn message(&self) -> Option<PreviewMessage<'_>> {
        match self {
            StoredContent::Text(text) => Some(PreviewMessage::Text(text.as_str())),
            StoredContent::Error(error) => Some(PreviewMessage::Failure(error)),
            StoredContent::Image(_) => None,
        }
    }
}

/// A line drawn where a preview image would be. Both readers show one — a
/// node's preview card and a viewer pane — so both draw it from here.
///
/// `Display` rather than an assembled `String`: both record every frame, and
/// the line goes straight into the record pass's text arena. A failure is a
/// typed error rendered on the way out, so only a reader actually sitting on
/// one pays for that rendering.
#[derive(Clone, Copy, Debug)]
pub(crate) enum PreviewMessage<'a> {
    /// Text standing in for the picture: a formatted non-image value, or a
    /// reader's own invitation before any value has arrived.
    Text(&'a str),
    /// Why the value on hand could not be prepared as an image.
    Failure(&'a PreviewImageError),
}

impl fmt::Display for PreviewMessage<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text(text) => f.write_str(text),
            Self::Failure(error) => write!(f, "{error}"),
        }
    }
}

#[derive(Debug)]
pub(crate) struct PreviewImage {
    /// The card's thumbnail, registered the moment the value arrives — and
    /// where the native metadata lives, since the full-resolution texture is
    /// prepared from the same source and so is described by the same figures.
    pub(crate) preview: DrawableImage,
    /// The full-resolution texture, or the source still waiting to become one.
    /// Read only through [`Self::full`], which is what resolves the wait.
    full: RefCell<FullImage>,
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

/// A registered texture plus what its source looked like before the
/// downscale: everything a pane needs to draw one image and label it.
///
/// The one shape the store hands out — the card's thumbnail and the viewer's
/// full-resolution texture are the same thing at two sizes, so neither reader
/// needs a form of its own.
#[derive(Clone, Debug)]
pub(crate) struct DrawableImage {
    pub(crate) handle: ImageHandle,
    /// Source dimensions before the downscale that sized the texture — the
    /// thumbnail's own cap, or the device's texture limit for a viewer's.
    pub(crate) native_size: UVec2,
    /// Source pixel format before the RGBA8 view conversion.
    pub(crate) native_format: ColorFormat,
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
    /// **Lazy, not deferred.** The upload is the source's own resolution — a
    /// full-sensor frame is hundreds of MB of RGBA8 — and only a viewer pane
    /// ever wants one; a preview card draws the
    /// thumbnail. Asking is therefore the whole scoping rule: a pane records
    /// only when its tab is the visible one, so a viewer stacked behind
    /// another uploads nothing, and one that becomes visible mid-record gets
    /// its texture in the pass that draws it rather than a frame later.
    ///
    /// `&self` because that ask comes from the record pass, which holds the
    /// run projection immutably (see [`AppCtx`](crate::gui::app::ctx::AppCtx)).
    /// The borrow is taken, resolved and dropped inside this call, so nothing
    /// can observe the cell mid-upload or hold it across a frame — and both
    /// answers are read *out* of it rather than borrowed from, the handle as a
    /// refcount bump on the texture the store still owns.
    ///
    /// A failure is stored, not just returned: it is terminal, so a broken
    /// image is diagnosed once rather than retried on every frame that draws
    /// it.
    pub(crate) fn full(&self, ui: &Ui) -> Result<DrawableImage, PreviewImageError> {
        let mut full = self.full.borrow_mut();
        if let FullImage::Pending(value) = &*full {
            *full = match Self::upload(ui, value) {
                Ok(handle) => FullImage::Resident(handle),
                Err(error) => FullImage::Failed(error),
            };
        }
        match &*full {
            // The thumbnail's metadata, because both textures come off the
            // same source — only the handle differs between the two sizes.
            FullImage::Resident(handle) => Ok(DrawableImage {
                handle: handle.clone(),
                ..self.preview.clone()
            }),
            FullImage::Failed(error) => Err(error.clone()),
            FullImage::Pending(_) => unreachable!("the pending source was just uploaded"),
        }
    }

    /// Register the value's pixels at full resolution.
    fn upload(ui: &Ui, value: &DynamicValue) -> Result<ImageHandle, PreviewImageError> {
        // Not a fallible step here, unlike in `prepare_content`: a
        // `PreviewImage` is only ever built over a value that already
        // downcast, and `Pending` holds that same value.
        let image = value
            .as_custom::<LensImage>()
            .expect("a stored preview image is only built over an image value");
        Ok(prepare_drawable(ui, image, None)?.handle)
    }
}

fn prepare_content(ui: &Ui, value: DynamicValue) -> StoredContent {
    // One downcast serves both the "is this an image at all?" test and the
    // conversion — a non-image renders as text, an image that fails to convert
    // as an error, and those are different outcomes. The borrow it hands out
    // is dead by the time `value` moves into `Pending` below.
    let Some(image) = value.as_custom::<LensImage>() else {
        return StoredContent::Text(value.to_string());
    };
    match prepare_drawable(ui, image, Some(PREVIEW_TEXTURE_DIM)) {
        Ok(preview) => StoredContent::Image(PreviewImage {
            preview,
            source_bytes: value.ram_usage().total(),
            full: RefCell::new(FullImage::Pending(value)),
        }),
        Err(error) => StoredContent::Error(error),
    }
}

/// Read a published image back to the CPU, fit it to `max_dim`, and hand
/// the pixels to the renderer — the one path from a pipeline value to
/// something a pane can draw, at whichever of the two sizes is asked for.
///
/// `max_dim` is the caller's *own* ceiling on the longest edge, or `None` to
/// take as many texels as the machine will hold — which is what a viewer
/// wants. The device's limit is read here and always binds on top of it, since
/// registration rejects an over-limit image rather than shrinking it, and a
/// mosaic wider than the GPU allows would otherwise have nothing to show at
/// all.
fn prepare_drawable(
    ui: &Ui,
    image: &LensImage,
    max_dim: Option<NonZeroU32>,
) -> Result<DrawableImage, PreviewImageError> {
    let cpu = image.interleaved();
    let native_size = UVec2::new(cpu.desc().width as u32, cpu.desc().height as u32);
    if native_size.x == 0 || native_size.y == 0 {
        return Err(PreviewImageError::Empty);
    }
    // Whichever ceiling is lower, and no ceiling at all only when neither the
    // caller nor the device names one.
    let ceiling = [max_dim, ui.max_image_dimension()]
        .into_iter()
        .flatten()
        .min();
    let raster = rgba8_raster(&cpu, capped_target(native_size, ceiling));
    Ok(DrawableImage {
        handle: ui.register_image(raster)?,
        native_size,
        native_format: cpu.desc().color_format,
    })
}

/// Downscale to `target` and convert to RGBA8 — the one place pixels are
/// produced, and so the seam a test asserts exact output on.
fn rgba8_raster(cpu: &CpuImage, target: UVec2) -> Raster {
    let rgba = Preview::new(target.x as usize, target.y as usize).to_rgba8(cpu);
    let desc = rgba.desc();
    assert_eq!(desc.color_format, ColorFormat::RGBA_U8);
    let pixels = rgba.into_bytes();
    assert_eq!(pixels.len(), desc.row_bytes() * desc.height);
    Raster::from_rgba8(target.x, target.y, pixels)
}

/// `native` scaled to fit `max_dim` on its longest edge — aspect preserved,
/// never upscaled. `None` is no ceiling, and answers the source's own
/// dimensions.
fn capped_target(native: UVec2, max_dim: Option<NonZeroU32>) -> UVec2 {
    let Some(max_dim) = max_dim else {
        return native;
    };
    let scale = (max_dim.get() as f32 / native.x.max(native.y) as f32).min(1.0);
    UVec2::new(
        (native.x as f32 * scale).round().max(1.0) as u32,
        (native.y as f32 * scale).round().max(1.0) as u32,
    )
}

#[cfg(test)]
pub(crate) mod internals {
    use imaginarium::{Image as RawImage, ImageDesc};

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
        DynamicValue::from_custom(LensImage::from(raw))
    }
}

#[cfg(test)]
mod tests;
