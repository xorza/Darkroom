//! Drawing primitives — circles and crosses — over `L_F32` and `RGB_F32` images. On a
//! single-channel image a color is written as its luminance; every entry point panics on any
//! other format.

use std::ops::RangeInclusive;

use glam::Vec2;

use crate::common::color::Color;
use crate::common::color_format::ColorFormat;
use crate::image::Image;

/// The `f32` pixels of an `L_F32` or `RGB_F32` image, with its geometry.
#[derive(Debug)]
struct Canvas<'a> {
    pixels: &'a mut [f32],
    width: usize,
    height: usize,
    channels: usize,
}

impl<'a> Canvas<'a> {
    /// # Panics
    /// Unless `image` is `L_F32` or `RGB_F32`.
    fn new(image: &'a mut Image) -> Self {
        let desc = image.desc();
        assert!(
            desc.color_format == ColorFormat::L_F32 || desc.color_format == ColorFormat::RGB_F32,
            "drawing requires an L_F32 or RGB_F32 image, got {}",
            desc.color_format
        );
        Self {
            pixels: bytemuck::cast_slice_mut(image.bytes_mut()),
            width: desc.width,
            height: desc.height,
            channels: desc.color_format.channel_count.count(),
        }
    }

    fn set(&mut self, x: usize, y: usize, color: Color) {
        let pixel = &mut self.pixels[(y * self.width + x) * self.channels..][..self.channels];
        match pixel {
            [grey] => *grey = color.luminance(),
            [r, g, b] => [*r, *g, *b] = [color.r, color.g, color.b],
            _ => unreachable!("a canvas has one or three channels"),
        }
    }

    /// Every pixel whose bounding box `[lo, hi]` reaches, as column and row ranges, or `None`
    /// when the box misses the image — or is not a box at all, for a NaN bound.
    fn covered(&self, lo: Vec2, hi: Vec2) -> Option<[RangeInclusive<usize>; 2]> {
        Some([
            span(lo.x, hi.x, self.width)?,
            span(lo.y, hi.y, self.height)?,
        ])
    }
}

/// The pixel indices from `⌊lo⌋` to `⌈hi⌉` that lie in `0..extent`.
#[expect(
    clippy::cast_sign_loss,
    reason = "both ends are clamped to zero or above"
)]
fn span(lo: f32, hi: f32, extent: usize) -> Option<RangeInclusive<usize>> {
    let first = lo.floor().max(0.0);
    let last = hi.ceil().min(extent as f32 - 1.0);
    (first <= last).then_some(first as usize..=last as usize)
}

/// Draws a hollow circle: every pixel whose centre lies between `radius − thickness / 2` and
/// `radius + thickness / 2` from `center`.
///
/// # Panics
/// Panics unless `image` is `L_F32` or `RGB_F32`.
pub fn draw_circle(image: &mut Image, center: Vec2, radius: f32, color: Color, thickness: f32) {
    let mut canvas = Canvas::new(image);
    let inner = (radius - thickness / 2.0).max(0.0);
    let outer = radius + thickness / 2.0;
    let Some([columns, rows]) = canvas.covered(center - outer, center + outer) else {
        return;
    };
    for y in rows {
        for x in columns.clone() {
            let distance_sq = (Vec2::new(x as f32, y as f32) - center).length_squared();
            if (inner * inner..=outer * outer).contains(&distance_sq) {
                canvas.set(x, y, color);
            }
        }
    }
}

/// Draws an upright cross marker: two lines through `center`, each arm
/// `arm_length` pixels long.
///
/// # Panics
/// Panics unless `image` is `L_F32` or `RGB_F32`.
pub fn draw_cross(image: &mut Image, center: Vec2, arm_length: f32, color: Color, thickness: f32) {
    let mut canvas = Canvas::new(image);
    let horizontal = Vec2::new(arm_length, 0.0);
    let vertical = Vec2::new(0.0, arm_length);
    draw_line(
        &mut canvas,
        center - horizontal,
        center + horizontal,
        color,
        thickness,
    );
    draw_line(
        &mut canvas,
        center - vertical,
        center + vertical,
        color,
        thickness,
    );
}

/// Draws every pixel whose centre lies within `thickness / 2` of the segment — and, for a line
/// thinner than a pixel, within half a pixel, which keeps one pixel per column of a shallow line
/// and one per row of a steep one, so a hairline stays connected.
fn draw_line(canvas: &mut Canvas<'_>, start: Vec2, end: Vec2, color: Color, thickness: f32) {
    let reach = (thickness / 2.0).max(0.5);
    let Some([columns, rows]) = canvas.covered(start.min(end) - reach, start.max(end) + reach)
    else {
        return;
    };
    let along = end - start;
    let length_sq = along.length_squared();
    for y in rows {
        for x in columns.clone() {
            let point = Vec2::new(x as f32, y as f32);
            let t = if length_sq > 0.0 {
                ((point - start).dot(along) / length_sq).clamp(0.0, 1.0)
            } else {
                0.0
            };
            if (point - (start + along * t)).length_squared() <= reach * reach {
                canvas.set(x, y, color);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use glam::Vec2;

    use crate::common::color::Color;
    use crate::common::color_format::ColorFormat;
    use crate::drawing::{draw_circle, draw_cross};
    use crate::image::Image;
    use crate::image::image_desc::ImageDesc;

    fn canvas(format: ColorFormat, size: usize) -> Image {
        Image::new_black(ImageDesc::new(size, size, format)).unwrap()
    }

    /// The `(x, y)` of every pixel whose first channel was drawn.
    fn drawn(image: &Image) -> Vec<(usize, usize)> {
        let desc = image.desc();
        let channels = desc.color_format.channel_count.count();
        let pixels: &[f32] = bytemuck::cast_slice(image.bytes());
        (0..desc.height)
            .flat_map(|y| (0..desc.width).map(move |x| (x, y)))
            .filter(|&(x, y)| pixels[(y * desc.width + x) * channels] != 0.0)
            .collect()
    }

    /// A ring of zero thickness at radius 2 holds exactly the centres at distance 2:
    /// `x² + y² = 4` has only the four axis solutions on the integer grid.
    #[test]
    fn a_circle_draws_the_centres_on_its_ring() {
        let mut image = canvas(ColorFormat::RGB_F32, 5);
        draw_circle(
            &mut image,
            Vec2::new(2.0, 2.0),
            2.0,
            Color::rgb(1.0, 0.5, 0.25),
            0.0,
        );
        assert_eq!(drawn(&image), [(2, 0), (0, 2), (4, 2), (2, 4)]);
        let pixels: &[f32] = bytemuck::cast_slice(image.bytes());
        assert_eq!(&pixels[..3 * 3][2 * 3..], &[1.0, 0.5, 0.25]);
    }

    /// Clipped at the top-left corner, and off the image entirely: no pixel of the image is
    /// drawn, and the bounding box does not wrap.
    #[test]
    fn a_circle_off_the_edge_is_clipped() {
        let mut image = canvas(ColorFormat::L_F32, 3);
        draw_circle(&mut image, Vec2::ZERO, 1.0, Color::GREEN, 0.0);
        assert_eq!(drawn(&image), [(1, 0), (0, 1)]);

        for center in [
            Vec2::new(-50.0, 1.0),
            Vec2::new(1.0, -50.0),
            Vec2::new(60.0, 1.0),
        ] {
            let mut image = canvas(ColorFormat::L_F32, 3);
            draw_circle(&mut image, center, 3.0, Color::GREEN, 2.0);
            assert_eq!(drawn(&image), [], "{center}");
        }
        let mut image = canvas(ColorFormat::L_F32, 3);
        draw_circle(&mut image, Vec2::NAN, 3.0, Color::GREEN, 2.0);
        assert_eq!(drawn(&image), []);
    }

    /// A one-pixel cross is one row and one column; a three-pixel one, three of each. Grey
    /// takes the colour's luminance: green's weight, 0.7152.
    #[test]
    fn a_cross_is_as_thick_as_asked() {
        let mut image = canvas(ColorFormat::L_F32, 5);
        draw_cross(&mut image, Vec2::new(2.0, 2.0), 2.0, Color::GREEN, 1.0);
        let row: Vec<_> = (0..5).map(|x| (x, 2)).collect();
        let mut expected: Vec<_> = (0..5).map(|y| (2, y)).chain(row).collect();
        expected.sort_by_key(|&(x, y)| (y, x));
        expected.dedup();
        assert_eq!(drawn(&image), expected);
        let pixels: &[f32] = bytemuck::cast_slice(image.bytes());
        assert_eq!(pixels[2 * 5 + 2], 0.7152);

        let mut image = canvas(ColorFormat::L_F32, 7);
        draw_cross(&mut image, Vec2::new(3.0, 3.0), 3.0, Color::GREEN, 3.0);
        let bands = |x: usize, y: usize| (2..=4).contains(&x) || (2..=4).contains(&y);
        let expected: Vec<_> = (0..7)
            .flat_map(|y| (0..7).map(move |x| (x, y)))
            .filter(|&(x, y)| bands(x, y))
            .collect();
        assert_eq!(drawn(&image), expected);
    }

    #[test]
    #[should_panic(expected = "drawing requires an L_F32 or RGB_F32 image, got RGBA f32")]
    fn rgba_is_refused() {
        let mut image = canvas(ColorFormat::RGBA_F32, 3);
        draw_circle(&mut image, Vec2::ONE, 1.0, Color::GREEN, 1.0);
    }
}
