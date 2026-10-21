use super::*;
use palantir::internals::ui_harness::UiHarness;
use std::sync::Arc;

use imaginarium::Image as RawImage;
use scenarium::{ConstValue, Node, SpecialNode};

use crate::core::document::internals::DocFixture;
use crate::core::preview_sink;
use crate::core::wake;

/// A document holding one preview node at `node`. The id is the caller's, so a
/// test can build the same node's document twice and have the store recognise
/// it as one preview rather than two.
///
/// No viewer tab: retention is the node's alone. A viewer asks for the
/// full-resolution upload itself, so the store does not read whether one is
/// open or visible.
fn document_with_preview(node: NodeId) -> Document {
    let mut fixture = DocFixture::default();
    let func = preview_sink::func(Arc::default());
    fixture.library.add(func.clone());
    fixture.doc.graph.insert(node, Node::from(&func));
    fixture.doc
}

/// The two ways a target is chosen: against a ceiling, and against none.
/// `None` is what a viewer's texture gets on any device whose limit the source
/// already fits under, so "the source's own dimensions" is the common answer
/// and not an edge case.
#[test]
fn capped_target_preserves_aspect_without_upscaling() {
    let device = NonZeroU32::new(16_384);

    // No ceiling: the source's own dimensions, however large.
    assert_eq!(
        capped_target(UVec2::new(30_000, 20_000), None),
        UVec2::new(30_000, 20_000)
    );
    // Under the device limit: likewise untouched.
    assert_eq!(
        capped_target(UVec2::new(6000, 4000), device),
        UVec2::new(6000, 4000)
    );
    // Over it: the longest edge lands exactly on the limit, aspect held —
    // 32768 → 16384 is a halving, so 20000 → 10000.
    assert_eq!(
        capped_target(UVec2::new(32_768, 20_000), device),
        UVec2::new(16_384, 10_000)
    );
    // A degenerate aspect keeps at least one row: 100000/16384 ≈ 0.164, and
    // 1 × 0.164 rounds to 0, which the floor lifts back to 1.
    assert_eq!(
        capped_target(UVec2::new(100_000, 1), device),
        UVec2::new(16_384, 1)
    );
    assert_eq!(
        capped_target(UVec2::new(1024, 512), Some(PREVIEW_TEXTURE_DIM)),
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
        Raster::from_srgba8(UVec2::new(2, 1), bytes).expect("2×1 RGBA8 is 8 bytes")
    );

    let rgb =
        RawImage::new_with_data(ImageDesc::new(1, 1, ColorFormat::RGB_F32), vec![0; 12]).unwrap();
    assert_eq!(
        rgba8_raster(&rgb, UVec2::new(1, 1)),
        Raster::from_srgba8(UVec2::ONE, vec![0, 0, 0, 255]).expect("1×1 RGBA8 is 4 bytes")
    );
}

/// An edge one past `u32::MAX` is refused, not wrapped to a 0-wide texture, on either
/// axis; `u32::MAX` itself fits, and a zero edge is empty. Descriptions only: no image
/// that wide fits in memory.
#[test]
fn native_size_refuses_an_edge_no_texture_holds() {
    let size = |width, height| native_size(ImageDesc::new(width, height, ColorFormat::RGB_U8));
    let max = usize::try_from(u32::MAX).unwrap();
    assert_eq!(size(max, 3).unwrap(), UVec2::new(u32::MAX, 3));
    for (width, height) in [(max + 1, 3), (3, max + 1)] {
        assert!(
            matches!(
                size(width, height),
                Err(PreviewImageError::EdgeTooLong { width: w, height: h }) if (w, h) == (width, height)
            ),
            "{width}×{height}",
        );
    }
    assert!(matches!(size(0, 3), Err(PreviewImageError::Empty)));
}

/// An image's source becomes a full-resolution texture only when a viewer
/// asks for one — the card itself needs the thumbnail alone — and neither
/// conversion runs on the thread that asks. The card lands once its thumbnail
/// is converted; until the full texture lands, the thumbnail stands in for it
/// at the full texture's size, so the viewer's framing never moves.
#[test]
fn the_full_texture_converts_on_the_first_ask_and_is_reused_after() {
    let mut arena = UiHarness::arena();
    let mut store = PreviewStore::new(wake::internals::noop());
    let node = NodeId::unique();

    store.publish(
        node,
        internals::image_value(1024, 512, ColorFormat::RGB_F32),
    );
    assert!(
        store.entries.is_empty(),
        "publishing converts nothing on the calling thread"
    );
    store.settle(arena.ui());
    let StoredContent::Image(image) = &store.entries[&node] else {
        panic!("an image value must create an image resource");
    };
    // The thumbnail caps the longest side at 256 — 1024 → 256 is a quarter,
    // so 512 → 128 — and reports the source it was cut from.
    assert_eq!(image.preview.handle.size(), UVec2::new(256, 128));
    assert_eq!(image.preview.texels, UVec2::new(256, 128));
    assert_eq!(image.preview.native_size, UVec2::new(1024, 512));
    assert_eq!(image.preview.native_format, ColorFormat::RGB_F32);
    // Three f32 channels: 12 bytes a pixel.
    assert_eq!(image.source_bytes, 1024 * 512 * 12);
    assert!(
        !image.is_full_resident(),
        "landing a value uploads the thumbnail and nothing else"
    );

    // A deviceless harness imposes no texture ceiling, so the full texture is
    // the source's own size — and until it lands, the thumbnail is drawn
    // there.
    assert_eq!(arena.ui().max_image_dimension(), None);
    let FullView::Converting(stand_in) = image.full(arena.ui()).expect("a valid image converts")
    else {
        panic!("the first ask queues the conversion");
    };
    assert_eq!(stand_in.handle.size(), UVec2::new(256, 128));
    assert_eq!(stand_in.texels, UVec2::new(1024, 512));
    assert!(!image.is_full_resident());

    store.settle(arena.ui());
    let StoredContent::Image(image) = &store.entries[&node] else {
        panic!("the image stays the same entry");
    };
    let FullView::Ready(full) = image.full(arena.ui()).expect("the conversion landed") else {
        panic!("a landed raster is resident");
    };
    assert_eq!(full.handle.size(), UVec2::new(1024, 512));
    assert_eq!(full.texels, UVec2::new(1024, 512));
    assert_eq!(
        (full.native_size, full.native_format),
        (image.preview.native_size, image.preview.native_format),
        "both sizes describe the one source, so they report it identically"
    );
    assert!(image.is_full_resident());

    // Asking again answers from the stored texture. It cannot convert again
    // even in principle: the image gave its source up to the conversion.
    let FullView::Ready(again) = image.full(arena.ui()).expect("still uploaded") else {
        panic!("a resident texture stays resident");
    };
    assert_eq!(again.handle.size(), full.handle.size());

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

/// A preview's value lives exactly as long as its node, and its publishes land
/// in order: the card keeps showing what it showed while a newer value
/// converts, each finished conversion shows even while a newer one is still
/// converting — so a node that publishes faster than its frames convert still
/// updates — and removing the node's value voids its conversions in flight.
#[test]
fn a_previews_value_lives_exactly_as_long_as_its_node() {
    let mut arena = UiHarness::arena();
    let mut store = PreviewStore::new(wake::internals::noop());
    let node = NodeId::unique();
    let mut document = document_with_preview(node);
    let other = document.graph.add(Node::from(SpecialNode::RunSinks));
    let int = |value| DynamicValue::Static(ConstValue::Int(value));
    let shown = |store: &PreviewStore| match store.entries.get(&node) {
        Some(StoredContent::Text(text)) => Some(text.clone()),
        Some(content) => panic!("a number formats as text, not {content:?}"),
        None => None,
    };

    store.publish(node, int(7));
    store.publish(other, int(9));
    store.settle(arena.ui());
    assert_eq!(shown(&store).as_deref(), Some("7"), "formatted on landing");

    store.publish(node, int(8));
    store.publish(node, int(10));
    assert_eq!(
        shown(&store).as_deref(),
        Some("7"),
        "the card keeps its value while the next converts"
    );
    store.land_next(arena.ui());
    assert_eq!(
        shown(&store).as_deref(),
        Some("8"),
        "a finished publish lands though a newer one is still converting"
    );
    store.land_next(arena.ui());
    assert_eq!(shown(&store).as_deref(), Some("10"));
    assert_eq!(store.entries.len(), 2, "a re-publish replaces in place");

    // Removed while converting, then published again: the first conversion
    // lands first, on the one thread, and is void.
    store.publish(node, int(11));
    store.remove(node);
    store.publish(node, int(13));
    store.land_next(arena.ui());
    assert_eq!(
        shown(&store),
        None,
        "a removed value's conversion lands nowhere"
    );
    store.land_next(arena.ui());
    assert_eq!(shown(&store).as_deref(), Some("13"));

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
