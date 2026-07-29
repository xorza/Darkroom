//! Runtime presentation resources for the values preview nodes publish.
//!
//! The store is the sole owner of preview-card and viewer textures. An image
//! uploads a small thumbnail immediately, retains its source only until a
//! viewer first needs the full texture, then drops the source after that
//! upload. Non-image values are formatted on receipt and dropped immediately.

use std::collections::HashMap;
use std::mem::take;

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
    /// Whether [`Self::reconcile`] has work to do: the document's retained
    /// set may have moved, or a fresh value landed that a viewer still needs
    /// uploaded at full resolution. The flag lives here rather than beside
    /// `Editor::needs_relayout` because the store is also written *outside*
    /// the frame — `ingest` runs from the worker drain in `App::update` — so
    /// a request has to survive until the next frame instead of resetting
    /// with it.
    needs_reconcile: bool,
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
        self.needs_reconcile = true;
    }

    /// Ask for a reconcile pass on the next frame. Raised by every edit whose
    /// step [`crate::core::edit::intent::types::UndoStep::requires_reconcile`]
    /// and by the non-undoable half of opening a viewer tab.
    pub(crate) fn request_reconcile(&mut self) {
        self.needs_reconcile = true;
    }

    /// Reconcile only if something asked for it. An idle frame changes
    /// neither the retained set nor the stored values, so it skips the pass
    /// entirely.
    pub(crate) fn reconcile_if_needed(&mut self, ui: &Ui, document: &Document) {
        if take(&mut self.needs_reconcile) {
            self.reconcile(ui, document);
        }
    }

    /// Release every presentation resource the document no longer retains and
    /// upload the full-resolution texture each *visible* viewer needs.
    fn reconcile(&mut self, ui: &Ui, document: &Document) {
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
mod tests {
    use super::*;
    use palantir::internals::UiHarness;

    use imaginarium::{Image as RawImage, ImageBuffer, ImageDesc};
    use scenarium::{Node, NodeKind, SpecialNode, StaticValue};

    use crate::core::document::TabRef;
    use crate::core::document::dock::DockOp;
    use crate::core::preview::preview_func;

    fn image_value(width: usize, height: usize, format: ColorFormat) -> DynamicValue {
        let desc = ImageDesc::new(width, height, format);
        let bytes = vec![128; desc.row_bytes() * height];
        let raw = RawImage::new_with_data(desc, bytes).unwrap();
        DynamicValue::from_custom(LensImage::from(ImageBuffer::from_cpu(raw)))
    }

    /// A document holding one preview node, optionally with its viewer tab
    /// open and active — only a group's visible tab draws, so only that one
    /// materializes a full-resolution texture.
    fn document_with_preview(viewer: bool) -> (Document, NodeId) {
        let mut document = Document::default();
        let node = document
            .graph
            .add_func_node(&preview_func(Default::default()));
        if viewer {
            let primary = document.layout.primary().id;
            let tab = TabRef::ImageViewer(node);
            document.layout.find_or_insert(tab, primary);
            document.layout.apply(DockOp::ActivateTab { tab });
        }
        (document, node)
    }

    #[test]
    fn capped_target_preserves_aspect_without_upscaling() {
        assert_eq!(
            capped_target(UVec2::new(6000, 4000), FULL_TEXTURE_DIM),
            UVec2::new(6000, 4000)
        );
        assert_eq!(
            capped_target(UVec2::new(16_384, 8192), FULL_TEXTURE_DIM),
            UVec2::new(8192, 4096)
        );
        assert_eq!(
            capped_target(UVec2::new(100_000, 1), FULL_TEXTURE_DIM),
            UVec2::new(8192, 1)
        );
        assert_eq!(
            capped_target(UVec2::new(1024, 512), PREVIEW_TEXTURE_DIM),
            UVec2::new(256, 128)
        );
    }

    #[test]
    fn image_preparation_converts_pixels_and_reports_native_metadata() {
        let desc = ImageDesc::new(2, 1, ColorFormat::RGBA_U8);
        let bytes = vec![255, 0, 0, 255, 0, 255, 0, 255];
        let raw = RawImage::new_with_data(desc, bytes.clone()).unwrap();
        let value = DynamicValue::from_custom(LensImage::from(ImageBuffer::from_cpu(raw)));
        let prepared = prepare_image(as_image(&value).unwrap(), FULL_TEXTURE_DIM).unwrap();
        assert_eq!(prepared.native_size, UVec2::new(2, 1));
        assert_eq!(prepared.native_format, ColorFormat::RGBA_U8);
        assert_eq!(prepared.raster, AptImage::from_rgba8(2, 1, bytes));

        let desc = ImageDesc::new(1, 1, ColorFormat::RGB_F32);
        let raw = RawImage::new_with_data(desc, vec![0; 12]).unwrap();
        let value = DynamicValue::from_custom(LensImage::from(ImageBuffer::from_cpu(raw)));
        let prepared = prepare_image(as_image(&value).unwrap(), FULL_TEXTURE_DIM).unwrap();
        assert_eq!(prepared.native_format, ColorFormat::RGB_F32);
        assert_eq!(
            prepared.raster,
            AptImage::from_rgba8(1, 1, vec![0, 0, 0, 255])
        );

        let error = as_image(&DynamicValue::from(42i64)).unwrap_err();
        assert_eq!(error, "value is not an image");
    }

    /// An image's source is held only until a viewer needs it at full
    /// resolution, then dropped — the card itself needs the thumbnail alone.
    #[test]
    fn image_source_lives_only_until_the_full_texture_is_registered() {
        let mut arena = UiHarness::arena();
        let mut store = PreviewStore::default();
        let (card_only, node) = document_with_preview(false);

        store.ingest_preview(
            arena.ui(),
            node,
            image_value(512, 256, ColorFormat::RGBA_U8),
        );
        let StoredContent::Image(image) = &store.entries[&node] else {
            panic!("an image value must create an image resource");
        };
        assert_eq!(image.preview.size(), UVec2::new(256, 128));
        assert_eq!(image.native_size, UVec2::new(512, 256));
        assert_eq!(image.native_format, ColorFormat::RGBA_U8);
        assert_eq!(image.source_bytes, 512 * 256 * 4);
        assert!(matches!(image.full, FullImage::Deferred(_)));

        let mut viewer = Document::default();
        viewer.graph.insert(
            node,
            Node::new(NodeKind::Func(preview_func(Default::default()).id)),
        );
        let primary = viewer.layout.primary().id;
        let tab = TabRef::ImageViewer(node);
        viewer.layout.find_or_insert(tab, primary);
        viewer.layout.apply(DockOp::ActivateTab { tab });
        store.reconcile(arena.ui(), &viewer);
        let StoredContent::Image(image) = &store.entries[&node] else {
            panic!("viewer demand must retain the image resource");
        };
        assert!(
            matches!(&image.full, FullImage::Resident(handle) if handle.size() == UVec2::new(512, 256))
        );

        store.reconcile(arena.ui(), &card_only);
        assert!(
            store.entries.contains_key(&node),
            "the node retains its value after the viewer closes"
        );
        store.reconcile(arena.ui(), &Document::default());
        assert!(
            store.entries.is_empty(),
            "no node leaves presentation resources alive"
        );
    }

    /// A preview's value lives exactly as long as its node: re-publishing
    /// replaces in place, and deleting the node releases it.
    #[test]
    fn a_previews_value_lives_exactly_as_long_as_its_node() {
        let mut arena = UiHarness::arena();
        let mut store = PreviewStore::default();
        let (mut document, node) = document_with_preview(false);
        let other = document
            .graph
            .add(Node::new(NodeKind::Special(SpecialNode::RunSinks)));

        store.ingest_preview(arena.ui(), node, DynamicValue::Static(StaticValue::Int(7)));
        store.ingest_preview(arena.ui(), other, DynamicValue::Static(StaticValue::Int(9)));
        assert!(
            matches!(&store.entries[&node], StoredContent::Text(t) if t == "7"),
            "the published value is formatted on receipt"
        );

        store.ingest_preview(arena.ui(), node, DynamicValue::Static(StaticValue::Int(8)));
        assert_eq!(store.entries.len(), 2, "a re-publish replaces in place");
        assert!(matches!(&store.entries[&node], StoredContent::Text(t) if t == "8"));

        store.reconcile_if_needed(arena.ui(), &document);
        assert!(
            store.entries.contains_key(&node),
            "a live preview node retains its value"
        );
        assert!(
            !store.entries.contains_key(&other),
            "a value keyed to a non-preview node is dropped"
        );

        document.graph.detach_node(node);
        store.request_reconcile();
        store.reconcile_if_needed(arena.ui(), &document);
        assert!(
            store.entries.is_empty(),
            "deleting the node releases what it was showing"
        );
    }

    #[test]
    fn the_reconcile_pass_runs_only_when_it_was_requested() {
        let mut arena = UiHarness::arena();
        let mut store = PreviewStore::default();
        let (document, node) = document_with_preview(false);
        store.ingest_preview(arena.ui(), node, DynamicValue::Static(StaticValue::Int(7)));

        // Spend the request `ingest_preview` raised for its own value.
        store.reconcile_if_needed(arena.ui(), &document);
        assert!(store.entries.contains_key(&node), "the node retains it");

        // Against a document holding nothing, the entry still survives while
        // nothing has asked for a pass — that's what makes the gate
        // load-bearing rather than decorative.
        let empty = Document::default();
        store.reconcile_if_needed(arena.ui(), &empty);
        assert!(
            store.entries.contains_key(&node),
            "an unrequested pass releases nothing"
        );

        store.request_reconcile();
        store.reconcile_if_needed(arena.ui(), &empty);
        assert!(
            store.entries.is_empty(),
            "a requested pass releases what the document stopped holding"
        );
    }
}
