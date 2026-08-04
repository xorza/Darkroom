use super::*;
use palantir::internals::UiHarness;

use imaginarium::{Image as RawImage, ImageBuffer, ImageDesc};
use scenarium::{ConstValue, Node, NodeKind, SpecialNode};

use crate::core::document::harness::DocFixture;
use crate::core::preview::preview_func;

fn image_value(width: usize, height: usize, format: ColorFormat) -> DynamicValue {
    let desc = ImageDesc::new(width, height, format);
    let bytes = vec![128; desc.row_bytes() * height];
    let raw = RawImage::new_with_data(desc, bytes).unwrap();
    DynamicValue::from_custom(LensImage::from(ImageBuffer::from_cpu(raw)))
}

/// A document holding one preview node at `node`. The id is the caller's, so a
/// test can build the same node's document twice and have the store recognise
/// it as one preview rather than two.
///
/// No viewer tab: retention is the node's alone. Whether a viewer is open, and
/// whether it is the visible tab, stopped mattering to the store when the
/// full-resolution upload became the pane's own on-demand ask.
fn document_with_preview(node: NodeId) -> Document {
    let mut fixture = DocFixture::default();
    let func = preview_func(Default::default());
    fixture.library.add(func.clone());
    fixture.doc.graph.insert(node, Node::from(&func));
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

/// The pixels themselves: RGBA8 passes through untouched at its own size, and
/// a format carrying no alpha channel gains an opaque one — a source that came
/// back transparent would render as nothing at all.
#[test]
fn conversion_produces_opaque_rgba8_at_the_target_size() {
    let bytes = vec![255, 0, 0, 255, 0, 255, 0, 255];
    let rgba =
        RawImage::new_with_data(ImageDesc::new(2, 1, ColorFormat::RGBA_U8), bytes.clone()).unwrap();
    assert_eq!(
        rgba8_raster(&rgba, UVec2::new(2, 1)),
        Raster::from_rgba8(2, 1, bytes)
    );

    let rgb =
        RawImage::new_with_data(ImageDesc::new(1, 1, ColorFormat::RGB_F32), vec![0; 12]).unwrap();
    assert_eq!(
        rgba8_raster(&rgb, UVec2::new(1, 1)),
        Raster::from_rgba8(1, 1, vec![0, 0, 0, 255])
    );
}

/// What a pane reads off a registered image: the texture at the capped size,
/// and the *source's* dimensions and format beside it — the pair the viewer
/// header compares to say "downscaled view".
#[test]
fn a_drawable_reports_its_source_alongside_the_capped_texture() {
    let mut arena = UiHarness::arena();
    let value = image_value(1024, 512, ColorFormat::RGB_F32);
    let image = value.as_custom::<LensImage>().unwrap();

    let full = prepare_drawable(arena.ui(), image, FULL_TEXTURE_DIM).unwrap();
    assert_eq!(full.handle.size(), UVec2::new(1024, 512), "under the cap");
    assert_eq!(full.native_size, UVec2::new(1024, 512));
    assert_eq!(full.native_format, ColorFormat::RGB_F32);

    // The thumbnail caps the longest side at 256, and still reports the source
    // it was made from rather than what it was reduced to.
    let thumb = prepare_drawable(arena.ui(), image, PREVIEW_TEXTURE_DIM).unwrap();
    assert_eq!(thumb.handle.size(), UVec2::new(256, 128));
    assert_eq!(thumb.native_size, UVec2::new(1024, 512));
    assert_eq!(thumb.native_format, ColorFormat::RGB_F32);
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
    assert_eq!(image.preview.handle.size(), UVec2::new(256, 128));
    assert_eq!(image.preview.native_size, UVec2::new(512, 256));
    assert_eq!(image.preview.native_format, ColorFormat::RGBA_U8);
    assert_eq!(image.source_bytes, 512 * 256 * 4);
    assert!(
        !image.is_full_resident(),
        "ingesting a value uploads the thumbnail and nothing else"
    );

    // 512×256 is under the 8192 cap, so the full texture keeps the source's
    // own dimensions — twice the thumbnail's on each axis.
    let full = image.full(arena.ui()).expect("a valid image uploads");
    assert_eq!(full.handle.size(), UVec2::new(512, 256));
    assert_eq!(
        (full.native_size, full.native_format),
        (image.preview.native_size, image.preview.native_format),
        "both sizes describe the one source, so they report it identically"
    );
    assert!(image.is_full_resident());

    // Asking again answers from the stored texture. It cannot re-upload even
    // in principle: the source was dropped as it became one, so a second
    // upload would have nothing to read.
    let again = image.full(arena.ui()).expect("still uploaded");
    assert_eq!(again.handle.size(), full.handle.size());
    assert!(image.is_full_resident());

    // Both textures are the store's to release, and a node that has gone
    // retains neither.
    store.reconcile(&document_with_preview(node));
    assert!(
        store.entries.contains_key(&node),
        "a live node retains them"
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
    let mut document = document_with_preview(node);
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
