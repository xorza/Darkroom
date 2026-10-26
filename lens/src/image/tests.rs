use common::Buffer2;

use super::*;

/// The planes of a 2x1 RGB image: (0.125, 0.5), (0.25, 0.625), (0.375, 0.75) — dyadic, so the
/// round-trip between layouts is exact.
fn planes() -> [Vec<f32>; 3] {
    [vec![0.125f32, 0.5], vec![0.25, 0.625], vec![0.375, 0.75]]
}

/// [`planes`] as a `lumos` image.
fn planar_master() -> LinearImage {
    LinearImage::from_planar_channels(PlanarDesc::new((2, 1), ChannelLayout::Rgb), planes())
}

/// The interleaved samples `planar_master` repacks to, in pixel order.
fn interleaved_bytes() -> Vec<u8> {
    [0.125f32, 0.25, 0.375, 0.5, 0.625, 0.75]
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

#[test]
fn to_planar_moves_the_planes_of_an_already_planar_image() {
    // The astro-chain fast path: consecutive `lumos` nodes hand the same allocation along
    // rather than repacking at each one.
    let planar = planar_master();
    let planes = planar.channel(0).pixels().as_ptr();
    let out = Image::from(planar).into_planar().unwrap();
    assert_eq!(out.channel(0).pixels().as_ptr(), planes);
}

#[test]
fn crossing_to_the_interleaved_domain_repacks_in_pixel_order() {
    let image = Image::from(planar_master());
    // Reading dimensions is not a repack.
    assert_eq!(
        image.desc(),
        imaginarium::InterleavedDesc::new(Size2us::new(2, 1), imaginarium::ColorFormat::RGB_F32)
    );
    assert_eq!(image.into_interleaved().bytes(), interleaved_bytes());
}

#[test]
fn borrowing_is_free_in_the_matching_layout_and_repacks_in_the_other() {
    let interleaved = Image::from(
        imaginarium::InterleavedImage::new_with_data(
            imaginarium::InterleavedDesc::new(
                Size2us::new(2, 1),
                imaginarium::ColorFormat::RGB_F32,
            ),
            interleaved_bytes(),
        )
        .unwrap(),
    );
    assert!(matches!(interleaved.interleaved(), Cow::Borrowed(_)));
    assert!(matches!(interleaved.planar(), Cow::Owned(_)));
    assert_eq!(interleaved.planar().channel(1).pixels(), &[0.25, 0.625]);

    let planar = Image::from(PlanarImage::new_rgb(
        planes().map(|plane| Buffer2::new((2, 1), plane)),
    ));
    assert!(matches!(planar.planar(), Cow::Borrowed(_)));
    assert_eq!(planar.planar().channel(2).pixels(), &[0.375, 0.75]);
    assert!(matches!(planar.interleaved(), Cow::Owned(_)));
    assert_eq!(planar.interleaved().bytes(), interleaved_bytes());
}

/// An input already produced by another astro node is planar, so `Image::take_planar` hands its
/// planes straight on — no repack between astro nodes, which is the whole point of the graph
/// carrying planar frames. A shared one still has to be copied, and pointer identity of the
/// plane allocation tells the two apart. Either way the metadata and the flags come along.
#[test]
fn a_planar_input_is_taken_without_repacking_and_a_shared_one_is_cloned() {
    let desc = PlanarDesc::new((4, 3), ChannelLayout::Mono);
    let master = || {
        let mut planar = LinearImage::from_planar_channels(desc, [vec![0.25f32; 12]]);
        planar.metadata.header.object = Some("M31".to_owned());
        let mut flags = vec![0; 12];
        flags[5] = 1;
        planar.set_flags(PixelFlags::from_bytes(Buffer2::new((4, 3), flags)));
        planar
    };
    let carried = |out: &LinearImage| {
        assert_eq!(out.metadata.header.object.as_deref(), Some("M31"));
        assert_eq!(out.flags().map(|flags| flags.bytes()[5]), Some(1));
    };

    let planar = master();
    let planes = planar.channel(0).pixels().as_ptr();
    let unique = DynamicValue::from_custom(Image::from(planar));
    let out = Image::take_planar(unique).unwrap();
    assert_eq!(
        out.channel(0).pixels().as_ptr(),
        planes,
        "unique planar input: the planes are moved, not repacked"
    );
    carried(&out);

    let planar = master();
    let planes = planar.channel(0).pixels().as_ptr();
    let shared = DynamicValue::from_custom(Image::from(planar));
    let second_holder = shared.clone();
    let out = Image::take_planar(shared).unwrap();
    assert_ne!(
        out.channel(0).pixels().as_ptr(),
        planes,
        "shared planar input: the planes are deep-cloned"
    );
    carried(&out);
    assert_eq!(out.desc(), desc);
    let original = second_holder.as_custom::<Image>().unwrap();
    assert_eq!(
        original.desc(),
        imaginarium::InterleavedDesc::new(Size2us::new(4, 3), imaginarium::ColorFormat::MONO_F32),
        "the shared original stays intact behind the other holder"
    );
}

/// The other side of the boundary: an input from the `imaginarium` domain is interleaved, so it
/// does convert — once, here, rather than inside every op.
#[test]
fn an_interleaved_input_deinterleaves_at_the_domain_boundary() {
    // 2x1 RGB: pixels (0.125, 0.25, 0.375) and (0.5, 0.625, 0.75).
    let samples = [0.125f32, 0.25, 0.375, 0.5, 0.625, 0.75];
    let raw = imaginarium::InterleavedImage::new_with_data(
        imaginarium::InterleavedDesc::new(Size2us::new(2, 1), imaginarium::ColorFormat::RGB_F32),
        samples.iter().flat_map(|v| v.to_le_bytes()).collect(),
    )
    .unwrap();
    let bytes = raw.bytes().as_ptr();
    let out = Image::take_planar(DynamicValue::from_custom(Image::from(raw.clone()))).unwrap();
    assert_eq!(out.channel(0).pixels(), &[0.125, 0.5]);
    assert_eq!(out.channel(1).pixels(), &[0.25, 0.625]);
    assert_eq!(out.channel(2).pixels(), &[0.375, 0.75]);

    // Staying in the interleaved domain moves a last holder's samples too.
    let kept = Image::take_interleaved(DynamicValue::from_custom(Image::from(raw)));
    assert_eq!(kept.bytes().as_ptr(), bytes, "moved, not copied");
}

/// A `lumos` frame has no side past `MAX_SIDE`, so neither has an image taken planar; an alpha
/// channel is left out of the frame's planes.
#[test]
fn an_image_makes_a_frame_up_to_the_side_limit() {
    let desc = |width| {
        imaginarium::InterleavedDesc::new(Size2us::new(width, 1), imaginarium::ColorFormat::RGBA_U8)
    };
    assert_eq!(
        frame_desc_of(desc(frame_desc::MAX_SIDE)),
        Ok(PlanarDesc::new(
            (frame_desc::MAX_SIDE, 1),
            ChannelLayout::Rgb
        ))
    );
    let size = Size2us::new(frame_desc::MAX_SIDE + 1, 1);
    assert_eq!(
        frame_desc_of(desc(size.width)),
        Err(FrameDescError::SidePastMax { size })
    );
}
