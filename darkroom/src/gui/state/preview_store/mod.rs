//! Runtime presentation resources for the values preview nodes publish.
//!
//! The store is the sole owner of preview-card and viewer textures. A
//! published value goes to the [`PreviewConverter`]'s thread, and what comes
//! back lands on a later frame: an image as a small thumbnail, with its source
//! held until a viewer first asks for the full texture; a non-image as its
//! formatted text. Until then the card keeps showing what it showed before.
//!
//! [`PreviewConverter`]: preview_converter::PreviewConverter

use common::Size2us;
pub(crate) mod error;
pub(crate) mod preview_converter;

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::num::NonZeroU32;

use glam::UVec2;
use imaginarium::{ColorFormat, Image as CpuImage, ImageDesc, Preview};
use palantir::{Image as Raster, ImageHandle, Ui};
use scenarium::{DynamicValue, NodeId};

use crate::core::document::Document;
use crate::core::wake::Wake;
use crate::gui::state::preview_store::error::DrawablePreviewError;
use crate::gui::state::preview_store::preview_converter::{
    Card, Converted, Job, Jobs, Outcome, PreviewConverter, Thumbnail, Ticket, Work,
};

/// Longest edge of a preview card's thumbnail — the one size darkroom picks
/// for itself. A viewer's texture names no figure at all: it takes the
/// source's own size, capped only by the device's texture limit.
const PREVIEW_TEXTURE_DIM: NonZeroU32 = NonZeroU32::new(256).unwrap();

#[derive(Debug)]
pub(crate) struct PreviewStore {
    /// Each preview node's current value, keyed by the node that published it —
    /// a preview *is* the thing on screen, so its identity is the widget's.
    entries: HashMap<NodeId, StoredContent>,
    /// The publishes of each node still converting for its card.
    converting: HashMap<NodeId, Converting>,
    last_ticket: Ticket,
    converter: PreviewConverter,
}

#[derive(Debug)]
pub(crate) enum StoredContent {
    Text(String),
    Image(DrawablePreview),
    Error(DrawablePreviewError),
}

impl StoredContent {
    /// The image behind this value, or `None` for a formatted non-image and
    /// for a value that failed to prepare. The single downcast both readers —
    /// the preview card and the image viewer — go through, so a new variant can't
    /// be handled by one and silently fall through the other.
    pub(crate) const fn image(&self) -> Option<&DrawablePreview> {
        match self {
            Self::Image(image) => Some(image),
            Self::Text(_) | Self::Error(_) => None,
        }
    }

    /// The text this value shows *in place of* an image: the formatted value
    /// itself, or the reason it isn't renderable. `None` when [`Self::image`]
    /// answered — the two are complementary, so a caller that renders the
    /// image never also has a message to show.
    pub(crate) const fn message(&self) -> Option<PreviewMessage<'_>> {
        match self {
            Self::Text(text) => Some(PreviewMessage::Text(text.as_str())),
            Self::Error(error) => Some(PreviewMessage::Failure(error)),
            Self::Image(_) => None,
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
    Failure(&'a DrawablePreviewError),
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
pub(crate) struct DrawablePreview {
    /// The card's thumbnail, registered the moment the value arrives — and
    /// where the native metadata lives, since the full-resolution texture is
    /// prepared from the same source and so is described by the same figures.
    pub(crate) preview: DrawableImage,
    /// The full-resolution texture, or the source still waiting to become one.
    /// Read only through [`Self::full`], which is what starts the conversion.
    full: RefCell<FullImage>,
    pub(crate) source_bytes: usize,
    /// The node and the publish this image came from — what a full raster
    /// converted from it is addressed to.
    node_id: NodeId,
    ticket: Ticket,
    /// Where [`Self::full`] queues the full-resolution conversion. The record
    /// pass asks with `&self`, so the image carries its own sender rather than
    /// reaching for the store's.
    jobs: Jobs,
}

/// The publishes of one node still converting: every ticket from `oldest` to
/// `newest`.
///
/// A node's results arrive in the order it published — one thread, one
/// queue — so a result in this span is newer than what the card shows, and
/// lands even while a newer publish converts. Waiting for the newest alone
/// would starve a node that publishes faster than its frames convert: each
/// publish would void the one before, and the card would never change.
#[derive(Debug, Clone, Copy)]
struct Converting {
    oldest: Ticket,
    newest: Ticket,
}

/// What a stored image holds for a viewer: the published value until one asks
/// for it at full resolution, the raster being converted at `texels`, the
/// texture from then on, or why there will never be one.
#[derive(Debug)]
enum FullImage {
    Pending(DynamicValue),
    Converting { texels: UVec2 },
    Resident(ImageHandle),
    Failed(DrawablePreviewError),
}

/// What a viewer draws for a stored image: the full-resolution texture, or,
/// while that converts, the thumbnail standing in for it at the same `texels`,
/// so the framing does not move when the texture arrives.
#[derive(Clone, Debug)]
pub(crate) enum FullView {
    Ready(DrawableImage),
    Converting(DrawableImage),
}

impl FullView {
    pub(crate) const fn image(&self) -> &DrawableImage {
        match self {
            Self::Ready(image) | Self::Converting(image) => image,
        }
    }
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
    /// The texture size this image is framed at: the handle's own, except for a
    /// thumbnail standing in for a full texture still converting.
    pub(crate) texels: UVec2,
    /// Source dimensions before the downscale that sized the texture — the
    /// thumbnail's own cap, or the device's texture limit for a viewer's.
    pub(crate) native_size: UVec2,
    /// Source pixel format before the RGBA8 view conversion.
    pub(crate) native_format: ColorFormat,
}

impl PreviewStore {
    pub(crate) fn new(wake: Wake) -> Self {
        Self {
            entries: HashMap::new(),
            converting: HashMap::new(),
            last_ticket: Ticket::default(),
            converter: PreviewConverter::new(wake),
        }
    }

    /// What `node_id` shows now, or `None` when nothing it published has
    /// landed.
    pub(crate) fn get(&self, node_id: NodeId) -> Option<&StoredContent> {
        self.entries.get(&node_id)
    }

    /// Queue one preview node's freshly published value for conversion. The
    /// card keeps what it shows until the result lands, in a later
    /// [`Self::land`].
    ///
    /// No retention filter on the way in: a value arrives keyed by the node
    /// that published it, and a node that published exists. `reconcile`
    /// drops it once that stops being true.
    pub(crate) fn publish(&mut self, node_id: NodeId, value: DynamicValue) {
        let ticket = self.last_ticket.next();
        self.last_ticket = ticket;
        self.converting
            .entry(node_id)
            .and_modify(|span| span.newest = ticket)
            .or_insert(Converting {
                oldest: ticket,
                newest: ticket,
            });
        self.converter.jobs().send(Job {
            node_id,
            ticket,
            work: Work::Thumbnail(value),
        });
    }

    /// Register every conversion finished since the last frame. A card
    /// replaces what the node showed, unless its value was removed since it
    /// was published; a full raster goes to the image that asked for it,
    /// unless that image was replaced meanwhile.
    pub(crate) fn land(&mut self, ui: &Ui) {
        let Self {
            entries,
            converting,
            converter,
            ..
        } = self;
        for converted in converter.ready() {
            accept(
                Landing {
                    entries,
                    converting,
                    converter,
                },
                ui,
                converted,
            );
        }
    }

    /// Drop `node_id`'s value, and void every conversion of it still in
    /// flight.
    pub(crate) fn remove(&mut self, node_id: NodeId) {
        self.entries.remove(&node_id);
        self.converting.remove(&node_id);
    }

    /// Drop every value, and every conversion still in flight.
    pub(crate) fn clear(&mut self) {
        self.entries.clear();
        self.converting.clear();
    }

    /// Release every presentation resource the document no longer retains.
    ///
    /// Run unconditionally once a frame: the retain is a lookup per stored
    /// value, over the handful of nodes holding one. That is cheaper than the
    /// bookkeeping a dirty flag needed — the store is written *outside* the
    /// frame too (`land` runs from the worker drain), so a flag had to survive
    /// until the next frame rather than resetting with it, and every edit path
    /// that moved the retained set had to remember to raise it.
    ///
    /// A preview's own node is its retention: delete the node and the value it
    /// was showing has nothing left to draw it. The shared liveness rule
    /// narrowed to preview nodes — see `Document::holds_preview_node` for why
    /// this cache is stricter than the others.
    pub(crate) fn reconcile(&mut self, document: &Document) {
        self.entries
            .retain(|node_id, _| document.holds_preview_node(*node_id));
        self.converting
            .retain(|node_id, _| document.holds_preview_node(*node_id));
    }
}

/// The store's fields a landing writes, borrowed apart from the converter's
/// result queue that drives it.
#[derive(Debug)]
struct Landing<'a> {
    entries: &'a mut HashMap<NodeId, StoredContent>,
    converting: &'a mut HashMap<NodeId, Converting>,
    converter: &'a PreviewConverter,
}

/// Land one finished conversion, or drop it when it no longer has a place.
fn accept(landing: Landing<'_>, ui: &Ui, converted: Converted) {
    let Converted {
        node_id,
        ticket,
        outcome,
    } = converted;
    match outcome {
        Outcome::Card(card) => land_card(landing, ui, node_id, ticket, card),
        Outcome::Full(raster) => {
            if let Some(StoredContent::Image(image)) = landing.entries.get(&node_id)
                && image.ticket == ticket
            {
                image.land_full(ui, &raster);
            }
        }
    }
}

/// Land `node_id`'s card from publish `ticket`, unless no conversion of the
/// node is pending — its value was removed or swept — or this one was
/// published before a removal voided it.
fn land_card(landing: Landing<'_>, ui: &Ui, node_id: NodeId, ticket: Ticket, card: Card) {
    let Landing {
        entries,
        converting,
        converter,
    } = landing;
    let Some(span) = converting.get_mut(&node_id) else {
        return;
    };
    if ticket < span.oldest {
        return;
    }
    if ticket == span.newest {
        converting.remove(&node_id);
    } else {
        span.oldest = ticket.next();
    }
    let content = match card {
        Card::Text(text) => StoredContent::Text(text),
        Card::Failed(error) => StoredContent::Error(error),
        Card::Image { thumbnail, source } => {
            let Thumbnail {
                raster,
                native_size,
                native_format,
            } = thumbnail;
            match ui.load_image(&raster) {
                Ok(handle) => StoredContent::Image(DrawablePreview {
                    preview: DrawableImage {
                        texels: handle.size(),
                        handle,
                        native_size,
                        native_format,
                    },
                    source_bytes: source.ram_usage().total(),
                    full: RefCell::new(FullImage::Pending(source)),
                    node_id,
                    ticket,
                    jobs: converter.jobs().clone(),
                }),
                Err(error) => StoredContent::Error(error.into()),
            }
        }
    };
    entries.insert(node_id, content);
}

impl DrawablePreview {
    /// What a viewer draws for this image. The first ask queues the
    /// full-resolution conversion, and until it lands the thumbnail stands in
    /// at the full texture's size.
    ///
    /// **Lazy.** The full raster is the source's own resolution — a
    /// full-sensor frame is hundreds of MB of RGBA8 — and only a viewer pane
    /// ever wants one; a preview card draws the thumbnail. Asking is therefore
    /// the whole scoping rule: a pane records only when its tab is the visible
    /// one, so a viewer stacked behind another converts nothing.
    ///
    /// `&self` because that ask comes from the record pass, which holds the
    /// run projection immutably (see [`AppCtx`](crate::gui::frame::app_ctx::AppCtx)).
    /// The borrow is taken, resolved and dropped inside this call, so nothing
    /// can observe the cell mid-change or hold it across a frame.
    ///
    /// A failure is stored, not just returned: it is terminal, so a broken
    /// image is diagnosed once rather than retried on every frame that draws
    /// it.
    pub(crate) fn full(&self, ui: &Ui) -> Result<FullView, DrawablePreviewError> {
        let mut full = self.full.borrow_mut();
        if let FullImage::Pending(source) = &*full {
            let texels = capped_target(self.preview.native_size, ui.max_image_dimension());
            self.jobs.send(Job {
                node_id: self.node_id,
                ticket: self.ticket,
                work: Work::Full {
                    source: source.clone(),
                    texels,
                },
            });
            *full = FullImage::Converting { texels };
        }
        match &*full {
            FullImage::Resident(handle) => Ok(FullView::Ready(DrawableImage {
                handle: handle.clone(),
                texels: handle.size(),
                ..self.preview.clone()
            })),
            FullImage::Converting { texels } => Ok(FullView::Converting(DrawableImage {
                texels: *texels,
                ..self.preview.clone()
            })),
            FullImage::Failed(error) => Err(error.clone()),
            FullImage::Pending(_) => unreachable!("the pending source was just queued"),
        }
    }

    /// Register the full raster converted for this image.
    fn land_full(&self, ui: &Ui, raster: &Raster) {
        let mut full = self.full.borrow_mut();
        debug_assert!(
            matches!(*full, FullImage::Converting { .. }),
            "a full raster lands only on the image that asked for it"
        );
        *full = match ui.load_image(raster) {
            Ok(handle) => FullImage::Resident(handle),
            Err(error) => FullImage::Failed(error.into()),
        };
    }
}

/// The texel size of an image `desc` describes, or why no texture holds it.
fn native_size(desc: ImageDesc) -> Result<UVec2, DrawablePreviewError> {
    let (Ok(x), Ok(y)) = (
        u32::try_from(desc.size.width),
        u32::try_from(desc.size.height),
    ) else {
        return Err(DrawablePreviewError::EdgeTooLong { size: desc.size });
    };
    if x == 0 || y == 0 {
        return Err(DrawablePreviewError::Empty);
    }
    Ok(UVec2::new(x, y))
}

/// Downscale to `target` and convert to RGBA8 — the one place pixels are
/// produced, and so the seam a test asserts exact output on.
fn rgba8_raster(cpu: &CpuImage, target: UVec2) -> Raster {
    let rgba = Preview::new(Size2us::new(target.x as usize, target.y as usize)).to_rgba8(cpu);
    let desc = rgba.desc();
    debug_assert_eq!(desc.color_format, ColorFormat::RGBA_U8);
    let pixels = rgba.into_bytes();
    debug_assert_eq!(pixels.len(), desc.row_bytes() * desc.size.height);
    Raster::from_srgba8(target, pixels).expect("a non-zero target the converter filled exactly")
}

/// `native` scaled to fit `max_dim` on its longest edge — aspect preserved,
/// never upscaled. `None` is no ceiling, and answers the source's own
/// dimensions.
#[expect(
    clippy::cast_sign_loss,
    reason = "each edge is rounded and clamped to at least 1 before the cast"
)]
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

#[cfg(feature = "internals")]
pub(crate) mod internals {
    #[cfg(test)]
    use imaginarium::Image as RawImage;
    #[cfg(test)]
    use lens::Image as LensImage;

    use super::*;

    impl PreviewStore {
        /// Wait for every conversion in flight, and land each — what a test
        /// does where production waits for the next frame.
        #[cfg(test)]
        pub(crate) fn settle(&mut self, ui: &Ui) {
            while self.in_flight() {
                self.land_next(ui);
            }
        }

        /// Wait for the next finished conversion, and land it alone.
        #[cfg(test)]
        pub(crate) fn land_next(&mut self, ui: &Ui) {
            let converted = self.converter.wait();
            let Self {
                entries,
                converting,
                converter,
                ..
            } = self;
            accept(
                Landing {
                    entries,
                    converting,
                    converter,
                },
                ui,
                converted,
            );
        }

        /// Show `content` for `node_id`, as a landed conversion would.
        #[cfg(test)]
        pub(crate) fn seed(&mut self, node_id: NodeId, content: StoredContent) {
            self.entries.insert(node_id, content);
        }

        /// Whether no node shows anything.
        #[cfg(test)]
        pub(crate) fn is_empty(&self) -> bool {
            self.entries.is_empty()
        }

        /// Whether a card or a full raster is still converting.
        #[cfg(test)]
        fn in_flight(&self) -> bool {
            !self.converting.is_empty()
                || self.entries.values().any(|content| {
                    content.image().is_some_and(|image| {
                        matches!(*image.full.borrow(), FullImage::Converting { .. })
                    })
                })
        }

        /// Land `card` as `node_id`'s newest publish, on the calling thread —
        /// for a bench that lands one conversion every frame and times the
        /// landing alone.
        pub(crate) fn land_converted(&mut self, ui: &Ui, node_id: NodeId, card: Card) {
            let ticket = self.last_ticket.next();
            self.last_ticket = ticket;
            self.converting.insert(
                node_id,
                Converting {
                    oldest: ticket,
                    newest: ticket,
                },
            );
            let Self {
                entries,
                converting,
                converter,
                ..
            } = self;
            land_card(
                Landing {
                    entries,
                    converting,
                    converter,
                },
                ui,
                node_id,
                ticket,
                card,
            );
        }
    }

    #[cfg(test)]
    impl DrawablePreview {
        /// Whether the full-resolution texture has been uploaded yet.
        ///
        /// The one question a test can't ask through [`DrawablePreview::full`],
        /// since asking that *is* what queues it.
        pub(crate) fn is_full_resident(&self) -> bool {
            matches!(&*self.full.borrow(), FullImage::Resident(_))
        }
    }

    /// A black image of `size` in `format`, as a node would publish it.
    #[cfg(test)]
    pub(crate) fn image_value(size: Size2us, format: ColorFormat) -> DynamicValue {
        let raw = RawImage::new_black(ImageDesc::new(size, format)).unwrap();
        DynamicValue::from_custom(LensImage::from(raw))
    }

    /// The smallest opaque image a preview card will render — 2×1 RGB8, which
    /// has no alpha channel to be transparent with.
    ///
    /// Publishing a value is what makes a card clickable at all (it records
    /// `Sense::NONE` without one), so every test about that chip, or about the
    /// viewer tab it opens, starts by publishing and settling this.
    #[cfg(test)]
    pub(crate) fn opaque_image_value() -> DynamicValue {
        image_value(Size2us::new(2, 1), ColorFormat::RGB_U8)
    }
}

#[cfg(test)]
mod tests;
