use super::*;

/// The planes of a 2x1 RGB image: (0.125, 0.5), (0.25, 0.625), (0.375, 0.75) — dyadic, so the
/// round-trip between layouts is exact.
fn planes() -> [Vec<f32>; 3] {
    [vec![0.125f32, 0.5], vec![0.25, 0.625], vec![0.375, 0.75]]
}

/// [`planes`] as a `lumos` image.
fn planar_master() -> LinearImage {
    LinearImage::from_planar_channels(ImageDimensions::new((2, 1), 3), planes())
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
    let out = Image::from(planar).into_planar();
    assert_eq!(out.channel(0).pixels().as_ptr(), planes);
}

#[test]
fn crossing_to_the_interleaved_domain_repacks_in_pixel_order() {
    let image = Image::from(planar_master());
    // Reading dimensions is not a repack.
    assert_eq!(
        image.desc(),
        imaginarium::ImageDesc::new(2, 1, imaginarium::ColorFormat::RGB_F32)
    );
    assert_eq!(image.into_interleaved().bytes(), interleaved_bytes());
}

#[test]
fn borrowing_is_free_in_the_matching_layout_and_repacks_in_the_other() {
    let interleaved = Image::from(
        imaginarium::Image::new_with_data(
            imaginarium::ImageDesc::new(2, 1, imaginarium::ColorFormat::RGB_F32),
            interleaved_bytes(),
        )
        .unwrap(),
    );
    assert!(matches!(interleaved.interleaved(), Cow::Borrowed(_)));
    assert!(matches!(interleaved.planar(), Cow::Owned(_)));
    assert_eq!(interleaved.planar().channel(1).pixels(), &[0.25, 0.625]);

    let planar = Image::from_planes(2, 1, planes());
    assert!(matches!(planar.planar(), Cow::Borrowed(_)));
    assert_eq!(planar.planar().channel(2).pixels(), &[0.375, 0.75]);
    assert!(matches!(planar.interleaved(), Cow::Owned(_)));
    assert_eq!(planar.interleaved().bytes(), interleaved_bytes());
}

/// An input already produced by another astro node is planar, so `Image::take_planar` hands its
/// planes straight on — no repack between astro nodes, which is the whole point of the graph
/// carrying planar frames. A shared one still has to be copied, and pointer identity of the
/// plane allocation tells the two apart.
#[test]
fn a_planar_input_is_taken_without_repacking_and_a_shared_one_is_cloned() {
    let dimensions = ImageDimensions::new((4, 3), 1);

    let planar = LinearImage::from_planar_channels(dimensions, [vec![0.25f32; 12]]);
    let planes = planar.channel(0).pixels().as_ptr();
    let unique = DynamicValue::from_custom(Image::from(planar));
    let out = Image::take_planar(unique);
    assert_eq!(
        out.channel(0).pixels().as_ptr(),
        planes,
        "unique planar input: the planes are moved, not repacked"
    );

    let planar = LinearImage::from_planar_channels(dimensions, [vec![0.25f32; 12]]);
    let planes = planar.channel(0).pixels().as_ptr();
    let shared = DynamicValue::from_custom(Image::from(planar));
    let second_holder = shared.clone();
    let out = Image::take_planar(shared);
    assert_ne!(
        out.channel(0).pixels().as_ptr(),
        planes,
        "shared planar input: the planes are deep-cloned"
    );
    assert_eq!(out.dimensions(), dimensions);
    let original = second_holder.as_custom::<Image>().unwrap();
    assert_eq!(
        original.desc(),
        imaginarium::ImageDesc::new(4, 3, imaginarium::ColorFormat::L_F32),
        "the shared original stays intact behind the other holder"
    );
}

/// The other side of the boundary: an input from the `imaginarium` domain is interleaved, so it
/// does convert — once, here, rather than inside every op.
#[test]
fn an_interleaved_input_deinterleaves_at_the_domain_boundary() {
    // 2x1 RGB: pixels (0.125, 0.25, 0.375) and (0.5, 0.625, 0.75).
    let samples = [0.125f32, 0.25, 0.375, 0.5, 0.625, 0.75];
    let raw = imaginarium::Image::new_with_data(
        imaginarium::ImageDesc::new(2, 1, imaginarium::ColorFormat::RGB_F32),
        samples.iter().flat_map(|v| v.to_le_bytes()).collect(),
    )
    .unwrap();
    let bytes = raw.bytes().as_ptr();
    let out = Image::take_planar(DynamicValue::from_custom(Image::from(raw.clone())));
    assert_eq!(out.channel(0).pixels(), &[0.125, 0.5]);
    assert_eq!(out.channel(1).pixels(), &[0.25, 0.625]);
    assert_eq!(out.channel(2).pixels(), &[0.375, 0.75]);

    // Staying in the interleaved domain moves a last holder's samples too.
    let kept = Image::take_interleaved(DynamicValue::from_custom(Image::from(raw)));
    assert_eq!(kept.bytes().as_ptr(), bytes, "moved, not copied");
}
