//! Runtime presentation resources for worker-pushed outputs.
//!
//! The store is the sole owner of pinned preview and viewer textures. Image
//! pushes upload a small preview immediately, retain their source only until
//! a viewer first needs the full texture, then drop the source after that
//! upload. Non-image values are formatted on receipt and dropped immediately.

use std::collections::HashMap;
use std::mem::take;

use aperture::{Image as AptImage, ImageHandle, Ui};
use glam::UVec2;
use imaginarium::{ColorFormat, Preview, ProcessingContext};
use lens::Image as LensImage;
use scenarium::{DynamicValue, NodeId, OutputPort, PinnedOutput};

use crate::core::document::Document;

const PREVIEW_TEXTURE_DIM: u32 = 256;
const FULL_TEXTURE_DIM: u32 = 8192;

#[derive(Default, Debug)]
pub(crate) struct PinnedOutputStore {
    pub(crate) entries: HashMap<OutputPort, StoredContent>,
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
    Image(PinnedImage),
    Error(String),
}

#[derive(Debug)]
pub(crate) struct PinnedImage {
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

impl PinnedOutputStore {
    pub(crate) fn ingest(
        &mut self,
        ui: &Ui,
        node_id: NodeId,
        values: Vec<PinnedOutput>,
        document: &Document,
    ) {
        if values.is_empty() {
            return;
        }
        // Collected once for the whole push: the membership test recurses
        // every nested graph, so asking per value re-walked the document
        // once per pushed output.
        let retained = document.retained_output_ports();
        for output in values {
            let port = OutputPort::new(node_id, output.port_idx);
            if !retained.contains(&port) {
                continue;
            }
            // PortRef cannot identify a particular graph instance, so the
            // latest push is the only value the UI can consistently present.
            self.entries.insert(port, prepare_content(ui, output.value));
            // A fresh image arrives preview-only; an open viewer needs the
            // reconcile pass to upload its full-resolution texture.
            self.needs_reconcile = true;
        }
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
        for port in document.visible_viewer_outputs() {
            self.materialize_full(ui, port);
        }
        // Collected once: the pinned half recurses every nested graph, so
        // calling it from the retain predicate cost one whole-document walk
        // per stored entry.
        let retained = document.retained_output_ports();
        self.entries.retain(|port, _| retained.contains(port));
    }

    fn materialize_full(&mut self, ui: &Ui, port: OutputPort) {
        let Some(content) = self.entries.remove(&port) else {
            return;
        };
        let content = match content {
            StoredContent::Image(image) => StoredContent::Image(image.materialize_full(ui)),
            content => content,
        };
        self.entries.insert(port, content);
    }
}

impl PinnedImage {
    fn materialize_full(self, ui: &Ui) -> Self {
        let full = match self.full {
            FullImage::Deferred(value) => match prepare_image(&value, FULL_TEXTURE_DIM) {
                Ok(prepared) => match ui.register_image(prepared.raster) {
                    Ok(handle) => FullImage::Resident(handle),
                    Err(error) => FullImage::Failed(error.to_string()),
                },
                Err(message) => FullImage::Failed(message),
            },
            full => full,
        };
        Self { full, ..self }
    }
}

fn prepare_content(ui: &Ui, value: DynamicValue) -> StoredContent {
    if value.as_custom::<LensImage>().is_none() {
        return StoredContent::Text(value.to_string());
    }
    match prepare_image(&value, PREVIEW_TEXTURE_DIM) {
        Ok(prepared) => match ui.register_image(prepared.raster) {
            Ok(preview) => {
                let source_bytes = value.ram_usage().total();
                StoredContent::Image(PinnedImage {
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

fn prepare_image(value: &DynamicValue, max_dim: u32) -> Result<PreparedImage, String> {
    let image = value
        .as_custom::<LensImage>()
        .ok_or_else(|| "value is not an image".to_owned())?;
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

    use imaginarium::{Image as RawImage, ImageBuffer, ImageDesc};
    use scenarium::StaticValue;

    use crate::core::document::TabRef;
    use crate::core::document::dock::DockOp;

    fn image_value(width: usize, height: usize, format: ColorFormat) -> DynamicValue {
        let desc = ImageDesc::new(width, height, format);
        let bytes = vec![128; desc.row_bytes() * height];
        let raw = RawImage::new_with_data(desc, bytes).unwrap();
        DynamicValue::from_custom(LensImage::from(ImageBuffer::from_cpu(raw)))
    }

    /// A document retaining `port` through a graph pin, an open viewer tab, or
    /// both. The viewer tab is activated, as opening one always does — only a
    /// group's visible tab draws, so only that one materializes.
    fn demanding_document(port: OutputPort, pinned: bool, viewer: bool) -> Document {
        let mut document = Document::default();
        document.graph.set_output_pinned(port, pinned);
        if viewer {
            let primary = document.layout.primary().id;
            let tab = TabRef::ImageViewer(port);
            document.layout.find_or_insert(tab, primary);
            document.layout.apply(DockOp::ActivateTab { tab });
        }
        document
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
        let prepared = prepare_image(&value, FULL_TEXTURE_DIM).unwrap();
        assert_eq!(prepared.native_size, UVec2::new(2, 1));
        assert_eq!(prepared.native_format, ColorFormat::RGBA_U8);
        assert_eq!(prepared.raster, AptImage::from_rgba8(2, 1, bytes));

        let desc = ImageDesc::new(1, 1, ColorFormat::RGB_F32);
        let raw = RawImage::new_with_data(desc, vec![0; 12]).unwrap();
        let value = DynamicValue::from_custom(LensImage::from(ImageBuffer::from_cpu(raw)));
        let prepared = prepare_image(&value, FULL_TEXTURE_DIM).unwrap();
        assert_eq!(prepared.native_format, ColorFormat::RGB_F32);
        assert_eq!(
            prepared.raster,
            AptImage::from_rgba8(1, 1, vec![0, 0, 0, 255])
        );

        let error = prepare_image(&DynamicValue::from(42i64), FULL_TEXTURE_DIM).unwrap_err();
        assert_eq!(error, "value is not an image");
    }

    #[test]
    fn ingest_formats_text_filters_unwanted_ports_and_replaces_by_authoring_port() {
        let ui = Ui::default();
        let mut store = PinnedOutputStore::default();
        let node = NodeId::unique();
        let first_port = OutputPort::new(node, 0);
        let document = demanding_document(first_port, true, false);
        store.ingest(
            &ui,
            node,
            vec![
                PinnedOutput {
                    port_idx: 0,
                    value: DynamicValue::Static(StaticValue::Int(7)),
                },
                PinnedOutput {
                    port_idx: 1,
                    value: DynamicValue::Static(StaticValue::Int(9)),
                },
            ],
            &document,
        );
        assert_eq!(store.entries.len(), 1);
        assert!(matches!(&store.entries[&first_port], StoredContent::Text(text) if text == "7"));

        store.ingest(
            &ui,
            node,
            vec![PinnedOutput {
                port_idx: 0,
                value: DynamicValue::Static(StaticValue::Int(8)),
            }],
            &document,
        );
        assert_eq!(store.entries.len(), 1);
        assert!(matches!(&store.entries[&first_port], StoredContent::Text(text) if text == "8"));
    }

    #[test]
    fn image_source_lives_only_until_full_texture_is_registered() {
        let ui = Ui::default();
        let mut store = PinnedOutputStore::default();
        let node = NodeId::unique();
        let first_port = OutputPort::new(node, 0);
        let pinned = demanding_document(first_port, true, false);
        store.ingest(
            &ui,
            node,
            vec![PinnedOutput {
                port_idx: 0,
                value: image_value(512, 256, ColorFormat::RGBA_U8),
            }],
            &pinned,
        );
        let StoredContent::Image(image) = &store.entries[&first_port] else {
            panic!("image output must create an image resource");
        };
        assert_eq!(image.preview.size(), UVec2::new(256, 128));
        assert_eq!(image.native_size, UVec2::new(512, 256));
        assert_eq!(image.native_format, ColorFormat::RGBA_U8);
        assert_eq!(image.source_bytes, 512 * 256 * 4);
        assert!(matches!(image.full, FullImage::Deferred(_)));

        let viewer = demanding_document(first_port, false, true);
        store.reconcile(&ui, &viewer);
        let StoredContent::Image(image) = &store.entries[&first_port] else {
            panic!("viewer demand must retain the image resource");
        };
        assert!(
            matches!(&image.full, FullImage::Resident(handle) if handle.size() == UVec2::new(512, 256))
        );

        store.reconcile(&ui, &pinned);
        assert!(
            store.entries.contains_key(&first_port),
            "a graph pin retains the preview after the viewer closes"
        );
        store.reconcile(&ui, &Document::default());
        assert!(
            store.entries.is_empty(),
            "no graph pin or viewer leaves presentation resources alive"
        );
    }

    #[test]
    fn the_reconcile_pass_runs_only_when_it_was_requested() {
        let ui = Ui::default();
        let mut store = PinnedOutputStore::default();
        let node = NodeId::unique();
        let port = OutputPort::new(node, 0);
        let pinned = demanding_document(port, true, false);
        store.ingest(
            &ui,
            node,
            vec![PinnedOutput {
                port_idx: 0,
                value: DynamicValue::Static(StaticValue::Int(7)),
            }],
            &pinned,
        );

        // Spend the request `ingest` raised for its own value.
        store.reconcile_if_needed(&ui, &pinned);
        assert!(store.entries.contains_key(&port), "the pin retains it");

        // Against a document that retains nothing, the entry still survives
        // while nothing has asked for a pass — that's what makes the gate
        // load-bearing rather than decorative.
        let empty = Document::default();
        store.reconcile_if_needed(&ui, &empty);
        assert!(
            store.entries.contains_key(&port),
            "an unrequested pass releases nothing"
        );

        store.request_reconcile();
        store.reconcile_if_needed(&ui, &empty);
        assert!(
            store.entries.is_empty(),
            "a requested pass releases what the document stopped retaining"
        );
    }

    #[test]
    fn only_the_visible_viewer_tab_pays_for_a_full_texture() {
        let ui = Ui::default();
        let mut store = PinnedOutputStore::default();
        let front = NodeId::unique();
        let back = NodeId::unique();
        let front_port = OutputPort::new(front, 0);
        let back_port = OutputPort::new(back, 0);

        // Two viewer tabs stacked in one pane: `back_port` opens first, then
        // `front_port` lands on top and becomes the group's visible tab.
        let mut document = Document::default();
        let primary = document.layout.primary().id;
        for port in [back_port, front_port] {
            document.graph.set_output_pinned(port, true);
            document
                .layout
                .find_or_insert(TabRef::ImageViewer(port), primary);
        }
        document.layout.apply(DockOp::ActivateTab {
            tab: TabRef::ImageViewer(front_port),
        });
        for (node, port) in [(front, front_port), (back, back_port)] {
            store.ingest(
                &ui,
                node,
                vec![PinnedOutput {
                    port_idx: 0,
                    value: image_value(512, 256, ColorFormat::RGBA_U8),
                }],
                &document,
            );
            assert!(store.entries.contains_key(&port));
        }

        fn full(store: &PinnedOutputStore, port: OutputPort) -> &FullImage {
            match &store.entries[&port] {
                StoredContent::Image(image) => &image.full,
                other => panic!("expected an image resource, got {other:?}"),
            }
        }

        store.reconcile_if_needed(&ui, &document);
        assert!(
            matches!(full(&store, front_port), FullImage::Resident(_)),
            "the visible tab's texture is uploaded"
        );
        assert!(
            matches!(full(&store, back_port), FullImage::Deferred(_)),
            "the tab behind it keeps only its preview — no full-resolution upload"
        );

        // Activating the back tab makes it the one that pays.
        document.layout.apply(DockOp::ActivateTab {
            tab: TabRef::ImageViewer(back_port),
        });
        store.request_reconcile();
        store.reconcile_if_needed(&ui, &document);
        assert!(
            matches!(full(&store, back_port), FullImage::Resident(_)),
            "activation uploads the newly-visible tab"
        );
    }
}
