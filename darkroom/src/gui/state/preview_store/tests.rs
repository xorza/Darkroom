use super::*;
use palantir::internals::UiHarness;

use imaginarium::{Image as RawImage, ImageBuffer, ImageDesc};
use scenarium::{ConstValue, Node, NodeKind, SpecialNode};

use crate::core::document::TabRef;
use crate::core::document::harness::DocFixture;
use crate::core::preview::preview_func;

fn image_value(width: usize, height: usize, format: ColorFormat) -> DynamicValue {
    let desc = ImageDesc::new(width, height, format);
    let bytes = vec![128; desc.row_bytes() * height];
    let raw = RawImage::new_with_data(desc, bytes).unwrap();
    DynamicValue::from_custom(LensImage::from(ImageBuffer::from_cpu(raw)))
}

/// A document holding one preview node at `node`, optionally with its viewer
/// tab open and active — only a group's visible tab draws, so only that one
/// materializes a full-resolution texture.
///
/// The id is the caller's so the two shapes can name the same node: a store
/// reconciled against both has to see one preview gaining and losing a viewer,
/// not two unrelated ones.
fn document_with_preview(node: NodeId, viewer: bool) -> Document {
    let mut fixture = DocFixture::default();
    let func = preview_func(Default::default());
    fixture.library.add(func.clone());
    fixture.doc.graph.insert(node, Node::from(&func));
    if viewer {
        fixture = fixture.with_tab(TabRef::ImageViewer(node));
    }
    fixture.doc
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
    assert!(
        matches!(error, PreviewImageError::NotAnImage),
        "got {error:?}"
    );
}

/// An image's source becomes a full-resolution texture only when a viewer
/// asks for one — the card itself needs the thumbnail alone — and the ask is
/// answered on the spot rather than scheduled.
#[test]
fn the_full_texture_is_uploaded_on_the_first_ask_and_reused_after() {
    let mut arena = UiHarness::arena();
    let mut store = PreviewStore::default();
    let node = NodeId::unique();

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
    assert!(
        !image.is_full_resident(),
        "ingesting a value uploads the thumbnail and nothing else"
    );

    // 512×256 is under the 8192 cap, so the full texture keeps the source's
    // own dimensions — twice the thumbnail's on each axis.
    let handle = image.full(arena.ui()).expect("a valid image uploads");
    assert_eq!(handle.size(), UVec2::new(512, 256));
    assert!(image.is_full_resident());

    // Asking again answers from the stored texture. It cannot re-upload even
    // in principle: the source was dropped as it became one, so a second
    // upload would have nothing to read.
    let again = image.full(arena.ui()).expect("still uploaded");
    assert_eq!(again.size(), handle.size());
    assert!(image.is_full_resident());
}

/// A stored value lives exactly as long as its node retains it, whatever the
/// viewer did with it.
#[test]
fn reconcile_releases_what_no_node_retains() {
    let mut arena = UiHarness::arena();
    let mut store = PreviewStore::default();
    let node = NodeId::unique();

    store.ingest_preview(
        arena.ui(),
        node,
        image_value(512, 256, ColorFormat::RGBA_U8),
    );
    store.reconcile(&document_with_preview(node, true));
    assert!(
        store.entries.contains_key(&node),
        "an open viewer retains the value"
    );

    store.reconcile(&document_with_preview(node, false));
    assert!(
        store.entries.contains_key(&node),
        "the node retains its value after the viewer closes"
    );
    store.reconcile(&Document::default());
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
    let node = NodeId::unique();
    let mut document = document_with_preview(node, false);
    let other = document
        .graph
        .add(Node::new(NodeKind::Special(SpecialNode::RunSinks)));

    store.ingest_preview(arena.ui(), node, DynamicValue::Static(ConstValue::Int(7)));
    store.ingest_preview(arena.ui(), other, DynamicValue::Static(ConstValue::Int(9)));
    assert!(
        matches!(&store.entries[&node], StoredContent::Text(t) if t == "7"),
        "the published value is formatted on receipt"
    );

    store.ingest_preview(arena.ui(), node, DynamicValue::Static(ConstValue::Int(8)));
    assert_eq!(store.entries.len(), 2, "a re-publish replaces in place");
    assert!(matches!(&store.entries[&node], StoredContent::Text(t) if t == "8"));

    store.reconcile(&document);
    assert!(
        store.entries.contains_key(&node),
        "a live preview node retains its value"
    );
    assert!(
        !store.entries.contains_key(&other),
        "a value keyed to a non-preview node is dropped"
    );

    document.graph.detach_node(node);
    store.reconcile(&document);
    assert!(
        store.entries.is_empty(),
        "deleting the node releases what it was showing"
    );
}
