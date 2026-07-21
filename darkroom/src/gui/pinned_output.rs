//! Runtime presentation resources for worker-pushed outputs.
//!
//! The store is the sole owner of pinned preview and viewer textures. Image
//! pushes upload a small preview immediately, retain their source only until
//! a viewer first needs the full texture, then drop the source after that
//! upload. Non-image values are formatted on receipt and dropped immediately.

use std::collections::{HashMap, HashSet};

use aperture::{Image as AptImage, ImageHandle, Ui};
use glam::UVec2;
use imaginarium::{ColorFormat, Preview, ProcessingContext};
use lens::Image as LensImage;
use scenarium::{DynamicValue, OutputPort, PinnedOutputs};

use crate::core::document::Document;

const PREVIEW_TEXTURE_DIM: u32 = 256;
const FULL_TEXTURE_DIM: u32 = 8192;

#[derive(Default, Debug)]
pub(crate) struct PinnedOutputStore {
    pub(crate) entries: HashMap<OutputPort, StoredContent>,
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
    pub(crate) fn ingest(&mut self, ui: &Ui, pushed: PinnedOutputs, document: &Document) {
        if pushed.values.is_empty() {
            return;
        }
        for output in pushed.values {
            let port = OutputPort::new(pushed.node.node_id, output.port_idx);
            if !document.retains_output_resource(port) {
                continue;
            }
            // PortRef cannot identify a particular graph instance, so the
            // latest push is the only value the UI can consistently present.
            self.entries.insert(port, prepare_content(ui, output.value));
        }
    }

    pub(crate) fn reconcile(&mut self, ui: &Ui, document: &Document) {
        let viewer_ports: HashSet<OutputPort> = document.viewer_outputs().collect();
        for &port in &viewer_ports {
            self.materialize_full(ui, port);
        }
        self.entries
            .retain(|port, _| viewer_ports.contains(port) || document.is_output_pinned(*port));
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
                Ok(prepared) => FullImage::Resident(ui.register_image(prepared.raster)),
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
        Ok(prepared) => {
            let source_bytes = value.ram_usage().total();
            StoredContent::Image(PinnedImage {
                preview: ui.register_image(prepared.raster),
                full: FullImage::Deferred(value),
                native_size: prepared.native_size,
                native_format: prepared.native_format,
                source_bytes,
            })
        }
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
    use scenarium::{NodeAddress, NodeId, PinnedOutput, StaticValue};

    use crate::core::document::{PortKind, PortRef, TabRef};

    fn image_value(width: usize, height: usize, format: ColorFormat) -> DynamicValue {
        let desc = ImageDesc::new(width, height, format);
        let bytes = vec![128; desc.row_bytes() * height];
        let raw = RawImage::new_with_data(desc, bytes).unwrap();
        DynamicValue::from_custom(LensImage::from(ImageBuffer::from_cpu(raw)))
    }

    fn push(node: NodeId, values: Vec<PinnedOutput>) -> PinnedOutputs {
        PinnedOutputs {
            node: NodeAddress::root(node),
            values,
        }
    }

    fn demanding_document(port: OutputPort, pinned: bool, viewer: bool) -> Document {
        let mut document = Document::default();
        document.graph.set_output_pinned(port, pinned);
        if viewer {
            let primary = document.layout.primary().id;
            document.layout.find_or_insert(
                TabRef::ImageViewer(PortRef {
                    node_id: port.node_id,
                    kind: PortKind::Output,
                    port_idx: port.port_idx,
                }),
                primary,
            );
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
            push(
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
            ),
            &document,
        );
        assert_eq!(store.entries.len(), 1);
        assert!(matches!(&store.entries[&first_port], StoredContent::Text(text) if text == "7"));

        store.ingest(
            &ui,
            PinnedOutputs {
                node: NodeAddress {
                    instances: vec![NodeId::unique()],
                    node_id: node,
                },
                values: vec![PinnedOutput {
                    port_idx: 0,
                    value: DynamicValue::Static(StaticValue::Int(8)),
                }],
            },
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
            push(
                node,
                vec![PinnedOutput {
                    port_idx: 0,
                    value: image_value(512, 256, ColorFormat::RGBA_U8),
                }],
            ),
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
}
