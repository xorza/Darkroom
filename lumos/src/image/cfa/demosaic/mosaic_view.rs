//! [`MosaicView`]: a frame's mosaic as a demosaic reads it, balanced, and the driver that runs a
//! kernel over its tiles.
//!
//! A kernel reads its input balanced by the camera's white balance, since its direction decisions
//! compare neighbours of different colors and would read a color cast as structure; dcraw,
//! RawTherapee, darktable and ART all balance before they demosaic. The view applies the balance
//! as a kernel reads a sample, and [`OutputPlanes`] takes it out as the kernel writes: each native
//! sample goes out as it came in, exact, and each interpolated one is divided by its color's gain.

use std::ops::Range;

use common::CancelToken;
use imaginarium::Buffer2;
use rayon::prelude::*;

use crate::error::Cancelled;
use crate::image::cfa::cfa_color::CfaColor;
use crate::image::cfa::demosaic::tiled::{NativePixel, OutputPlanes, TileKernel, TilePlace};
use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;

/// A color filter a demosaic reads a mosaic through, anchored at the mosaic's first pixel.
pub(crate) trait MosaicPattern: Copy + Sync {
    /// The color of the photosite at `position`.
    fn color_at(&self, position: Vec2us) -> CfaColor;
}

/// A frame's mosaic as a demosaic reads it: its samples row by row — calibration may put them
/// outside `[0, 1]` — its size, its `pattern`, and the gains, red, green and blue, that balance its
/// colors for the direction decisions; one each for samples already balanced.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MosaicView<'a, P> {
    data: &'a [f32],
    size: Size2us,
    pattern: P,
    gains: [f32; 3],
}

impl<'a, P: MosaicPattern> MosaicView<'a, P> {
    /// `data` under `pattern`, unbalanced.
    ///
    /// # Panics
    /// When `data` is empty.
    pub(crate) fn new(data: &'a Buffer2<f32>, pattern: P) -> Self {
        let size = Size2us::from(data);
        assert!(
            size.width > 0 && size.height > 0,
            "a mosaic of {size} holds no samples"
        );
        let data = data.pixels();
        debug_assert!(
            data.iter().all(|v| v.is_finite()),
            "a mosaic holds NaN or Infinity values"
        );
        Self {
            data,
            size,
            pattern,
            gains: [1.0; 3],
        }
    }

    /// This mosaic, its colors balanced by `gains` for the direction decisions.
    pub(super) const fn with_gains(self, gains: [f32; 3]) -> Self {
        Self { gains, ..self }
    }

    /// The frame's width, the stride of its rows.
    pub(super) const fn frame_width(&self) -> usize {
        self.size.width
    }

    pub(crate) const fn pattern(&self) -> P {
        self.pattern
    }

    /// The sample at `at`, balanced by its color's gain.
    #[inline(always)]
    pub(crate) fn balanced(&self, at: Vec2us) -> f32 {
        self.data[at.y * self.size.width + at.x] * self.gain_at(at)
    }

    /// The gain that balances the color of the photosite at `at`.
    #[inline(always)]
    pub(super) fn gain_at(&self, at: Vec2us) -> f32 {
        self.gains[self.pattern.color_at(at).index()]
    }

    /// Row `y`'s own samples over `columns`, as the input holds them.
    #[inline(always)]
    pub(crate) fn samples(&self, y: usize, columns: Range<usize>) -> &[f32] {
        let start = y * self.size.width;
        &self.data[start + columns.start..start + columns.end]
    }

    /// The pixel at `at` as a kernel writes it: its index, its color and its own sample.
    #[inline(always)]
    pub(crate) fn native(&self, at: Vec2us) -> NativePixel {
        let index = at.y * self.size.width + at.x;
        NativePixel {
            index,
            native: self.pattern.color_at(at).index(),
            sample: self.data[index],
        }
    }

    /// Demosaic the frame in tiles placed by `kernel.tiling`: each worker makes its tile with
    /// `kernel.new_tile` and runs `kernel.run` on it at each place, which writes into the planes
    /// the part of the place `tiling.margin` or more inside its edges, and nothing else. The border
    /// comes from [`Self::fill_border`].
    ///
    /// # Errors
    ///
    /// [`Cancelled`] when `cancel` is set, between tiles and before the border.
    pub(crate) fn demosaic<T>(
        &self,
        kernel: TileKernel<
            impl Fn() -> T + Sync + Send,
            impl Fn(&mut T, TilePlace, OutputPlanes) + Sync,
        >,
        cancel: &CancelToken,
    ) -> Result<[Vec<f32>; 3], Cancelled> {
        let TileKernel {
            tiling,
            new_tile,
            run,
        } = kernel;
        let pixels = self.size.pixel_count();
        let mut planes = [
            vec![0.0f32; pixels],
            vec![0.0f32; pixels],
            vec![0.0f32; pixels],
        ];
        let places = tiling.places(self.size);
        let out = OutputPlanes::new(&mut planes, self.gains);
        places
            .par_iter()
            .try_for_each_init(new_tile, |tile, &place| {
                Cancelled::check(cancel)?;
                run(tile, place, out);
                Ok(())
            })?;
        Cancelled::check(cancel)?;
        let border = if places.is_empty() {
            self.size.width.max(self.size.height)
        } else {
            tiling.border()
        };
        let [r, g, b] = &mut planes;
        self.fill_border([r, g, b], border);
        Ok(planes)
    }

    /// Fill the pixels within `border` of the frame's edge of `out`, row by row in parallel: the
    /// whole of each row in the band at the top and bottom, and the ends of each row between.
    fn fill_border(&self, [out_r, out_g, out_b]: [&mut [f32]; 3], border: usize) {
        let Size2us { width, height } = self.size;
        out_r
            .par_chunks_mut(width)
            .zip(out_g.par_chunks_mut(width))
            .zip(out_b.par_chunks_mut(width))
            .enumerate()
            .for_each(|(y, ((row_r, row_g), row_b))| {
                let mut fill_span = |span: Range<usize>| {
                    for x in span {
                        let [r, g, b] = self.border_pixel(Vec2us::new(x, y));
                        row_r[x] = r;
                        row_g[x] = g;
                        row_b[x] = b;
                    }
                };
                if y < border || y + border >= height || 2 * border >= width {
                    fill_span(0..width);
                } else {
                    fill_span(0..border);
                    fill_span(width - border..width);
                }
            });
    }

    /// Each color at `at` of the border: the pixel's own sample in its color, and each other
    /// color from its 3×3 neighbours of that color, those beside it weighted twice the diagonal
    /// ones; where none of those is of the color, the mean of its samples in the smallest square
    /// window around the pixel that holds any.
    ///
    /// A mean of one color's samples is the same whatever gain balances that color, so the border
    /// reads the input as it is, and its native samples are the input's own.
    fn border_pixel(&self, at: Vec2us) -> [f32; 3] {
        let Size2us { width, height } = self.size;
        let Vec2us { x, y } = at;
        let mut sums = [0.0f32; 3];
        let mut weights = [0.0f32; 3];
        for neighbor_y in y.saturating_sub(1)..=(y + 1).min(height - 1) {
            for neighbor_x in x.saturating_sub(1)..=(x + 1).min(width - 1) {
                let weight = match (neighbor_y.abs_diff(y), neighbor_x.abs_diff(x)) {
                    (0, 0) => 0.0,
                    (0, 1) | (1, 0) => 0.5,
                    (1, 1) => 0.25,
                    _ => unreachable!("a 3×3 neighbourhood"),
                };
                let color = self
                    .pattern
                    .color_at(Vec2us::new(neighbor_x, neighbor_y))
                    .index();
                sums[color] += self.data[neighbor_y * width + neighbor_x] * weight;
                weights[color] += weight;
            }
        }
        let native = self.pattern.color_at(at).index();
        let sample = self.data[y * width + x];
        let channel = |color: usize| {
            if color == native {
                sample
            } else if weights[color] > 0.0 {
                sums[color] / weights[color]
            } else {
                self.nearest_mean(at, color).unwrap_or(sample)
            }
        };
        [channel(0), channel(1), channel(2)]
    }

    /// The mean of `color`'s samples in the smallest square window around `at` that holds any, for
    /// a border pixel whose 3×3 neighbourhood has none. `None` only when the whole frame has no
    /// sample of `color`, where there is nothing to interpolate from and the caller keeps the
    /// pixel's own sample.
    fn nearest_mean(&self, at: Vec2us, color: usize) -> Option<f32> {
        let Size2us { width, height } = self.size;
        let Vec2us { x, y } = at;
        (2..width.max(height)).find_map(|radius| {
            let mut sum = 0.0f32;
            let mut count = 0usize;
            for neighbor_y in y.saturating_sub(radius)..=(y + radius).min(height - 1) {
                for neighbor_x in x.saturating_sub(radius)..=(x + radius).min(width - 1) {
                    if self
                        .pattern
                        .color_at(Vec2us::new(neighbor_x, neighbor_y))
                        .index()
                        == color
                    {
                        sum += self.data[neighbor_y * width + neighbor_x];
                        count += 1;
                    }
                }
            }
            (count > 0).then(|| sum / count as f32)
        })
    }
}

#[cfg(test)]
mod tests {
    use imaginarium::Buffer2;

    use crate::image::cfa::cfa_pattern::CfaPattern;
    use crate::image::cfa::demosaic::mosaic_view::MosaicView;
    use crate::internals::cfa::XTRANS_PATTERN;
    use crate::math::size2us::Size2us;
    use crate::math::vec2us::Vec2us;

    /// A view takes its plane's size, which must not be empty, reads each pixel's own sample as it
    /// is, and balances it by its color's gain: on RGGB, (1, 0) is green under 1, (1, 1) blue
    /// under 4, so 0.5 reads 0.5 and 2.
    #[test]
    fn a_view_reads_its_samples_and_balances_them_by_color() {
        let data = Buffer2::new_filled(6, 4, 0.5);
        let bayer = MosaicView::new(&data, CfaPattern::Rggb).with_gains([2.0, 1.0, 4.0]);
        assert_eq!(bayer.size, Size2us::new(6, 4));
        let blue = bayer.native(Vec2us::new(1, 1));
        assert_eq!((blue.index, blue.native, blue.sample), (7, 2, 0.5));
        assert_eq!(bayer.balanced(Vec2us::new(1, 0)), 0.5);
        assert_eq!(bayer.balanced(Vec2us::new(1, 1)), 2.0);
        assert_eq!(
            MosaicView::new(&data, XTRANS_PATTERN).size,
            Size2us::new(6, 4)
        );
        for size in [Size2us::new(0, 2), Size2us::new(2, 0)] {
            let data = Buffer2::new_default(size.width, size.height);
            let refused = std::panic::catch_unwind(|| {
                MosaicView::new(&data, CfaPattern::Rggb);
            });
            assert!(refused.is_err(), "{size:?}");
        }
    }
}
