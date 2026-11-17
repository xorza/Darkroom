use imaginarium::InterleavedImage;

use super::*;
use palantir::Harness;

use scenarium::{Binding, ConstValue, InputPort, Node, NodeId, SpecialNode};

use crate::core::document::internals::DocumentFixture;
use crate::core::preview_sink;
use crate::core::wake;

/// A document holding one preview node of the main graph at `node`; the id is the caller's, so the same node's
/// document can be built twice. No viewer tab: retention is the node's alone.
fn document_with_preview(node: ProgramId) -> Document {
    let mut fixture = DocumentFixture::default();
    let func = preview_sink::func(Arc::default());
    fixture.library.add(func.clone());
    fixture
        .document
        .graphs
        .main_mut()
        .insert(node.node_id(), Node::from(&func));
    fixture.document
}

/// [`document_with_preview`], its node's Display set to `stretch`.
fn document_with_display(node: ProgramId, stretch: DisplayStretch) -> Document {
    let mut document = document_with_preview(node);
    document.graphs.main_mut().bindings.insert(
        InputPort::new(node.node_id(), preview_sink::DISPLAY_PORT),
        Binding::Const(ConstValue::Enum(stretch.name().to_owned())),
    );
    document
}

/// A target is chosen against a ceiling or against none; `None` is the common answer for a
/// viewer on a device whose limit the source fits under.
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
    let rgba = InterleavedImage::new_with_data(
        InterleavedDesc::new(USize2::new(2, 1), ColorFormat::RGBA_U8),
        bytes.clone(),
    )
    .unwrap();
    assert_eq!(
        rgba8_raster(&Image::from(rgba), UVec2::new(2, 1), DisplayStretch::Linear).unwrap(),
        Raster::from_srgba8(UVec2::new(2, 1), bytes).expect("2×1 RGBA8 is 8 bytes")
    );

    let rgb = InterleavedImage::new_with_data(
        InterleavedDesc::new(USize2::new(1, 1), ColorFormat::RGB_F32),
        vec![0; 12],
    )
    .unwrap();
    assert_eq!(
        rgba8_raster(&Image::from(rgb), UVec2::new(1, 1), DisplayStretch::Linear).unwrap(),
        Raster::from_srgba8(UVec2::ONE, vec![0, 0, 0, 255]).expect("1×1 RGBA8 is 4 bytes")
    );
}

/// An edge one past `u32::MAX` is refused, not wrapped to a 0-wide texture, on either
/// axis; `u32::MAX` itself fits, and a zero edge is empty. Descriptions only: no image
/// that wide fits in memory.
#[test]
fn native_size_refuses_an_edge_no_texture_holds() {
    let size = |width, height| {
        native_size(InterleavedDesc::new(
            USize2::new(width, height),
            ColorFormat::RGB_U8,
        ))
    };
    let max = usize::try_from(u32::MAX).unwrap();
    assert_eq!(size(max, 3).unwrap(), UVec2::new(u32::MAX, 3));
    for (width, height) in [(max + 1, 3), (3, max + 1)] {
        assert!(
            matches!(
                size(width, height),
                Err(DrawablePreviewError::EdgeTooLong { size }) if size == USize2::new(width, height)
            ),
            "{width}×{height}",
        );
    }
}

/// An image's source becomes a full-resolution texture only when a viewer
/// asks for one — the card itself needs the thumbnail alone — and neither
/// conversion runs on the thread that asks. The card lands once its thumbnail
/// is converted; until the full texture lands, the thumbnail stands in for it
/// at the full texture's size, unasked and then converting, so the viewer's
/// framing never moves. An ask of an image asked for already, or of a node
/// with no value, changes nothing.
#[test]
fn the_full_texture_converts_on_the_first_ask_and_is_reused_after() {
    let arena = Harness::builder(UVec2::ONE).mono().build();
    let mut store = PreviewStore::new(wake::internals::noop());
    let node = ProgramId::from(NodeId::unique());

    store.publish(
        node,
        internals::image_value(USize2::new(1024, 512), ColorFormat::RGB_F32),
        DisplayStretch::Linear,
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
    let FullView::Unasked(unasked) = image.full(arena.ui()).expect("a valid image converts") else {
        panic!("no viewer asked yet");
    };
    assert_eq!(unasked.handle.size(), UVec2::new(256, 128));
    assert_eq!(unasked.texels, UVec2::new(1024, 512));
    assert!(!store.in_flight(), "reading the image asks for nothing");

    store.convert_full(node, None);
    store.convert_full(ProgramId::from(NodeId::unique()), None);
    let StoredContent::Image(image) = &store.entries[&node] else {
        panic!("the image stays the same entry");
    };
    let FullView::Converting(stand_in) = image.full(arena.ui()).expect("a valid image converts")
    else {
        panic!("the ask queues the conversion");
    };
    assert_eq!(stand_in.handle.size(), UVec2::new(256, 128));
    assert_eq!(stand_in.texels, UVec2::new(1024, 512));
    assert!(!image.is_full_resident());
    assert!(store.in_flight());

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

    // Asking again answers from the stored texture: only an unasked image
    // converts, though it keeps its source.
    store.convert_full(node, None);
    assert!(
        !store.in_flight(),
        "a resident texture is not asked for again"
    );
    let StoredContent::Image(image) = &store.entries[&node] else {
        panic!("the image stays the same entry");
    };
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
    let arena = Harness::builder(UVec2::ONE).mono().build();
    let mut store = PreviewStore::new(wake::internals::noop());
    let node = ProgramId::from(NodeId::unique());
    let mut document = document_with_preview(node);
    let other = ProgramId::from(
        document
            .graphs
            .main_mut()
            .add(Node::from(SpecialNode::RunSinks)),
    );
    let int = |value| DynamicValue::Static(ConstValue::Int(value));
    let shown = |store: &PreviewStore| match store.entries.get(&node) {
        Some(StoredContent::Text(text)) => Some(text.clone()),
        Some(content) => panic!("a number formats as text, not {content:?}"),
        None => None,
    };

    store.publish(node, int(7), DisplayStretch::Linear);
    store.publish(other, int(9), DisplayStretch::Linear);
    store.settle(arena.ui());
    assert_eq!(shown(&store).as_deref(), Some("7"), "formatted on landing");

    // Held unlanded, so the newer publish comes while the older one has converted.
    store.publish(node, int(8), DisplayStretch::Linear);
    let eight = store.next_converted();
    store.publish(node, int(10), DisplayStretch::Linear);
    assert_eq!(
        shown(&store).as_deref(),
        Some("7"),
        "the card keeps its value while the next converts"
    );
    store.land_one(arena.ui(), eight);
    assert_eq!(
        shown(&store).as_deref(),
        Some("8"),
        "a finished publish lands though a newer one is still converting"
    );
    store.land_next(arena.ui());
    assert_eq!(shown(&store).as_deref(), Some("10"));
    assert_eq!(store.entries.len(), 2, "a re-publish replaces in place");

    // Removed after it converted, then published again: the first conversion
    // lands first, on the one thread, and is void.
    store.publish(node, int(11), DisplayStretch::Linear);
    let eleven = store.next_converted();
    store.remove(node);
    store.publish(node, int(13), DisplayStretch::Linear);
    store.land_one(arena.ui(), eleven);
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

    document.graphs.main_mut().detach_node(node.node_id());
    store.reconcile(&document);
    assert!(
        store.entries.is_empty(),
        "deleting the node releases what it was showing"
    );
}

/// A preview whose node's Display moves converts its image again under the new stretch, once and
/// with no run: a reconcile that finds it converting waits, and one that finds it current queues
/// nothing. A flat image, which Auto finds nothing to stretch in, keeps its source, so moving back
/// to Linear converts it again.
#[test]
fn a_moved_display_converts_the_image_again_once() {
    let arena = Harness::builder(UVec2::ONE).mono().build();
    let mut store = PreviewStore::new(wake::internals::noop());
    let node = ProgramId::from(NodeId::unique());
    // A dim 4 × 4 ramp, 0.01 to 0.025 in steps of 0.001, which the stretch can place.
    let ramp: Vec<f32> = (0..16).map(|v| 0.01 + v as f32 * 0.001).collect();
    let ramp = InterleavedImage::new_with_data(
        InterleavedDesc::new(USize2::new(4, 4), ColorFormat::MONO_F32),
        ramp.iter().flat_map(|v| v.to_ne_bytes()).collect(),
    )
    .unwrap();
    let stretch_of = |store: &PreviewStore| match &store.entries[&node] {
        StoredContent::Image(image) => Ok(image.stretch),
        StoredContent::Unstretched(unstretched) => Err(unstretched.stretch),
        other => panic!("{other:?} keeps no image"),
    };

    store.publish(
        node,
        DynamicValue::from_custom(Image::from(ramp)),
        DisplayStretch::Linear,
    );
    store.settle(arena.ui());
    let published = store.publishes();
    let auto = document_with_display(node, DisplayStretch::Auto);
    store.reconcile(&auto);
    assert_eq!(store.publishes(), published + 1, "the move converts again");
    store.reconcile(&auto);
    assert_eq!(store.publishes(), published + 1, "converting, it waits");
    store.settle(arena.ui());
    store.reconcile(&auto);
    assert_eq!(
        store.publishes(),
        published + 1,
        "current, it queues nothing"
    );
    assert_eq!(stretch_of(&store), Ok(DisplayStretch::Auto));

    store.publish(
        node,
        internals::image_value(USize2::new(4, 4), ColorFormat::MONO_F32),
        DisplayStretch::Auto,
    );
    store.settle(arena.ui());
    assert_eq!(stretch_of(&store), Err(DisplayStretch::Auto));
    assert!(matches!(
        store.entries[&node].message(),
        Some(PreviewMessage::Failure(DrawablePreviewError::Stretch(_)))
    ));
    store.reconcile(&document_with_display(node, DisplayStretch::Linear));
    store.settle(arena.ui());
    assert_eq!(stretch_of(&store), Ok(DisplayStretch::Linear));
}
