use imaginarium::Image as RawImage;
use scenarium::DynamicValue;

use crate::astro::nodes::runtime::image_to_planar;
use crate::image::Image;

/// An input already produced by another astro node is planar, so `image_to_planar` hands its
/// planes straight on — no repack between astro nodes, which is the whole point of the graph
/// carrying planar frames. A shared one still has to be copied, and pointer identity of the plane
/// allocation tells the two apart.
#[test]
fn a_planar_input_is_taken_without_repacking_and_a_shared_one_is_cloned() {
    let dimensions = lumos::ImageDimensions::new((4, 3), 1);

    let planar = lumos::LinearImage::from_planar_channels(dimensions, [vec![0.25f32; 12]]);
    let planes = planar.channel(0).pixels().as_ptr();
    let unique = DynamicValue::from_custom(Image::from(planar));
    let out = image_to_planar(unique);
    assert_eq!(
        out.channel(0).pixels().as_ptr(),
        planes,
        "unique planar input: the planes are moved, not repacked"
    );

    let planar = lumos::LinearImage::from_planar_channels(dimensions, [vec![0.25f32; 12]]);
    let planes = planar.channel(0).pixels().as_ptr();
    let shared = DynamicValue::from_custom(Image::from(planar));
    let second_holder = shared.clone();
    let out = image_to_planar(shared);
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
    let raw = RawImage::new_with_data(
        imaginarium::ImageDesc::new(2, 1, imaginarium::ColorFormat::RGB_F32),
        samples.iter().flat_map(|v| v.to_le_bytes()).collect(),
    )
    .unwrap();
    let out = image_to_planar(DynamicValue::from_custom(Image::from(raw)));
    assert_eq!(out.channel(0).pixels(), &[0.125, 0.5]);
    assert_eq!(out.channel(1).pixels(), &[0.25, 0.625]);
    assert_eq!(out.channel(2).pixels(), &[0.375, 0.75]);
}
