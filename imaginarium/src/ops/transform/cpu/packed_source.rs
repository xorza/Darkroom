//! [`PackedSource`]: the input image a transform samples.

use std::array;

use glam::Vec2;

use crate::color::sample::Sample;
use crate::image::Image;

/// The input image a transform samples: `width × height` pixels in row-major order, each its `N`
/// channels of `T` side by side.
#[derive(Debug, Clone, Copy)]
pub(super) struct PackedSource<'a, T> {
    /// The samples, which a SIMD kernel loads a tap's channels from at [`Self::tap`].
    pub(super) pixels: &'a [T],
    width: usize,
    height: usize,
}

impl<'a, T: Sample> PackedSource<'a, T> {
    /// `image`'s pixels as samples of `T`.
    pub(super) fn of(image: &'a Image) -> Self {
        Self {
            pixels: bytemuck::cast_slice(image.bytes()),
            width: image.desc().width,
            height: image.desc().height,
        }
    }

    /// The index of the first of pixel `(x, y)`'s `N` channels, or `None` outside the image.
    #[inline]
    pub(super) fn tap<const N: usize>(self, x: i32, y: i32) -> Option<usize> {
        let (x, y) = (usize::try_from(x).ok()?, usize::try_from(y).ok()?);
        (x < self.width && y < self.height).then_some((y * self.width + x) * N)
    }

    /// The `N` channels of the pixel at integer `(x, y)` in the low lanes of an `[f32; 4]`
    /// (unused lanes stay zero); out-of-bounds reads as all-zero. Only the low `N` lanes are ever
    /// written back, so the padding never affects output.
    #[inline]
    fn read<const N: usize>(self, x: i32, y: i32) -> [f32; 4] {
        let Some(base) = self.tap::<N>(x, y) else {
            return [0.0; 4];
        };
        let mut px = [0.0f32; 4];
        for (lane, &raw) in px.iter_mut().zip(&self.pixels[base..base + N]) {
            *lane = raw.to_f32();
        }
        px
    }

    /// The pixel nearest `pos`.
    #[inline]
    pub(super) fn nearest<const N: usize>(self, pos: Vec2) -> [f32; 4] {
        // `round_ties_even` matches WGSL `round`, which rounds halves to even.
        let x = pos.x.round_ties_even() as i32;
        let y = pos.y.round_ties_even() as i32;
        self.read::<N>(x, y)
    }

    /// The four pixels about `pos`, blended by its fraction.
    #[inline]
    pub(super) fn bilinear<const N: usize>(self, pos: Vec2) -> [f32; 4] {
        let fx0 = pos.x.floor();
        let fy0 = pos.y.floor();
        let fx = pos.x - fx0;
        let fy = pos.y - fy0;
        let x0 = fx0 as i32;
        let y0 = fy0 as i32;

        let c00 = self.read::<N>(x0, y0);
        let c10 = self.read::<N>(x0 + 1, y0);
        let c01 = self.read::<N>(x0, y0 + 1);
        let c11 = self.read::<N>(x0 + 1, y0 + 1);

        let c0 = mix(c00, c10, fx);
        let c1 = mix(c01, c11, fx);
        mix(c0, c1, fy)
    }
}

/// `mix(a, b, t) = a * (1 - t) + b * t`, per channel — matching WGSL `mix`.
#[inline]
fn mix(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    array::from_fn(|i| a[i] * (1.0 - t) + b[i] * t)
}
