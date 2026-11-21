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
        imaginarium::InterleavedDesc::new(USize2::new(2, 1), imaginarium::ColorFormat::RGB_F32)
    );
    assert_eq!(image.into_interleaved().bytes(), interleaved_bytes());
}

#[test]
fn borrowing_is_free_in_the_matching_layout_and_repacks_in_the_other() {
    let interleaved = Image::from(
        imaginarium::InterleavedImage::new_with_data(
            imaginarium::InterleavedDesc::new(USize2::new(2, 1), imaginarium::ColorFormat::RGB_F32),
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
        imaginarium::InterleavedDesc::new(USize2::new(4, 3), imaginarium::ColorFormat::MONO_F32),
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
        imaginarium::InterleavedDesc::new(USize2::new(2, 1), imaginarium::ColorFormat::RGB_F32),
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
        imaginarium::InterleavedDesc::new(USize2::new(width, 1), imaginarium::ColorFormat::RGBA_U8)
    };
    assert_eq!(
        frame_desc_of(desc(frame_desc::MAX_SIDE)),
        Ok(PlanarDesc::new(
            (frame_desc::MAX_SIDE, 1),
            ChannelLayout::Rgb
        ))
    );
    let size = USize2::new(frame_desc::MAX_SIDE + 1, 1);
    assert_eq!(
        frame_desc_of(desc(size.width)),
        Err(FrameDescError::SidePastMax { size })
    );
}

/// A pixel reads where the samples lie: interleaved `u16` in its own type, planes as `f32`, and
/// one past the edge as `None` in either.
#[test]
fn a_pixel_reads_in_the_layout_it_lies_in() {
    use imaginarium::{Channels, InterleavedDesc, InterleavedImage};

    let desc = InterleavedDesc::new(USize2::new(2, 1), imaginarium::ColorFormat::MONO_U16);
    // 1000 and 2000 as native-endian bytes.
    let bytes = [1000_u16, 2000]
        .iter()
        .flat_map(|v| v.to_ne_bytes())
        .collect();
    let interleaved = Image::from(InterleavedImage::new_with_data(desc, bytes).unwrap());
    assert_eq!(
        interleaved.sample(USizeVec2::new(1, 0)),
        Some(PixelSample::U16(Channels::Mono(2000)))
    );
    let planar = Image::from(PlanarImage::new_mono(Buffer2::new(
        (2, 1),
        vec![0.25, 0.75],
    )));
    assert_eq!(
        planar.sample(USizeVec2::new(1, 0)),
        Some(PixelSample::F32(Channels::Mono(0.75)))
    );
    assert_eq!(interleaved.sample(USizeVec2::new(2, 0)), None);
    assert_eq!(planar.sample(USizeVec2::new(0, 1)), None);
}

/// Auto shows the image through lumos's own stretch of it, byte for byte, which is not the
/// linear picture; a flat image has no background to place, and Auto says so where Linear draws.
#[test]
fn an_auto_preview_is_lumos_stretch_of_the_image() {
    use crate::image::display_stretch::{DisplayStretch, DisplayStretchError};

    // A dim 8 × 8 ramp, as a linear frame sits: 0.01 to 0.0731 in steps of 0.001.
    let ramp: Vec<f32> = (0..64).map(|v| 0.01 + v as f32 * 0.001).collect();
    let image = Image::from(PlanarImage::new_mono(Buffer2::new((8, 8), ramp)));
    let size = USize2::new(4, 4);

    let mut frame = image.to_planar().unwrap();
    DisplayStretch::AUTO.apply(&mut frame).unwrap();
    let lumos = imaginarium::Preview::new(size).planes_to_rgba8(&FrameParts::from(frame).samples);
    let auto = image.preview_rgba8(size, DisplayStretch::Auto).unwrap();
    assert_eq!(auto.bytes(), lumos.bytes());
    let linear = image.preview_rgba8(size, DisplayStretch::Linear).unwrap();
    assert_ne!(
        auto.bytes(),
        linear.bytes(),
        "the stretch changes the picture"
    );

    let flat = Image::from(PlanarImage::new_mono(Buffer2::new((8, 8), vec![0.5; 64])));
    assert!(flat.preview_rgba8(size, DisplayStretch::Linear).is_ok());
    assert!(matches!(
        flat.preview_rgba8(size, DisplayStretch::Auto),
        Err(DisplayStretchError::Stretch(_))
    ));
}
