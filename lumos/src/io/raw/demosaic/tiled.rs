//! The tiled driver both demosaics run on: where the tiles lie, the frame's output planes each tile
//! writes its own part of, the memory a run holds, and the border no tile computes in full.
//!
//! A kernel reads its input balanced by the camera's white balance, since its direction decisions
//! compare neighbours of different colours and would read a colour cast as structure; dcraw,
//! RawTherapee, darktable and ART all balance before they demosaic. The balance is applied as a
//! tile reads its input and taken out as it writes: each native sample goes out as it came in,
//! exact, and each interpolated one is divided by its colour's gain.

use std::ops::Range;

use common::CancelToken;
use rayon::prelude::*;

use crate::concurrency::unsafe_send_ptr::UnsafeSendPtr;
use crate::io::cancelled::Cancelled;
use crate::io::raw::demosaic::DemosaicMemory;
use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;

/// How a kernel tiles a frame.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Tiling {
    /// The side of a tile.
    pub(crate) tile: usize,
    /// How far inside the frame's edges the first tile starts and the last ends: the reach of a
    /// kernel that reads its input beyond its tile.
    pub(crate) inset: usize,
    /// How far inside its edges a tile computes its pixels in full, and writes them.
    pub(crate) margin: usize,
}

/// Where a tile lies in the frame, and how much of it the frame holds.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TilePlace {
    pub(crate) top: usize,
    pub(crate) left: usize,
    pub(crate) size: Size2us,
}

/// The frame's output planes, which every tile writes at the pixels it alone owns.
#[derive(Debug, Clone, Copy)]
pub(crate) struct OutputPlanes {
    planes: [UnsafeSendPtr<f32>; 3],
}

impl Tiling {
    /// The pixels nearest the frame's edge that no tile writes: the inset, and a tile's margin.
    pub(crate) const fn border(self) -> usize {
        self.inset + self.margin
    }

    /// The tiles over a frame of `size`, overlapping by twice the margin so that the parts they
    /// write tile the frame within the border; none when the frame is all border.
    fn places(self, size: Size2us) -> Vec<TilePlace> {
        let border = self.border();
        if size.width <= 2 * border || size.height <= 2 * border {
            return Vec::new();
        }
        let step = self.tile - 2 * self.margin;
        let starts = |extent: usize| {
            let count = extent
                .saturating_sub(2 * self.inset + self.tile)
                .div_ceil(step)
                + 1;
            (0..count).map(move |index| self.inset + index * step)
        };
        starts(size.height)
            .flat_map(|top| {
                starts(size.width).map(move |left| TilePlace {
                    top,
                    left,
                    size: Size2us::new(
                        (size.width - self.inset - left).min(self.tile),
                        (size.height - self.inset - top).min(self.tile),
                    ),
                })
            })
            .collect()
    }
}

impl OutputPlanes {
    /// `values` into plane `channel` from `index` on, as they are.
    ///
    /// # Safety
    ///
    /// The planes cover the frame, and no other tile writes the pixels from `index` to
    /// `index + values.len()`.
    #[inline(always)]
    pub(crate) const unsafe fn write_row(self, channel: usize, index: usize, values: &[f32]) {
        // SAFETY: as this function's contract.
        unsafe {
            self.planes[channel]
                .get()
                .add(index)
                .copy_from_nonoverlapping(values.as_ptr(), values.len());
        }
    }

    /// Channel `channel` of the pixel at `index` of the frame: `value`, as a kernel computed it in
    /// balanced units, brought back by the channel's `gain` — but where the channel is the pixel's
    /// `native` colour, its own `sample`, as the input held it, so it leaves exact.
    ///
    /// # Safety
    ///
    /// The planes cover the frame, and no other tile writes `index`.
    #[inline(always)]
    pub(crate) unsafe fn write(
        self,
        channel: usize,
        index: usize,
        native: usize,
        sample: f32,
        value: f32,
        gain: f32,
    ) {
        let value = if channel == native {
            sample
        } else {
            value / gain
        };
        // SAFETY: as this function's contract.
        unsafe { self.planes[channel].get().add(index).write(value) };
    }
}

/// The output's three planes, and the peak: the caller's input, the output, and the workers'
/// buffers of `tile_bytes` each. The workers are the pool's, which demosaics running at once
/// share, so the charge to each is an upper bound.
pub(crate) fn demosaic_memory(size: Size2us, tile_bytes: usize) -> DemosaicMemory {
    let plane_bytes = size
        .width
        .saturating_mul(size.height)
        .saturating_mul(size_of::<f32>());
    let output_bytes = plane_bytes.saturating_mul(3);
    DemosaicMemory {
        output_bytes,
        peak_bytes: plane_bytes
            .saturating_add(output_bytes)
            .saturating_add(workspace_bytes(tile_bytes)),
    }
}

/// The tile buffers of every worker of the pool, at most one each: a worker makes its tile for a
/// run of tiles and drops it before it takes other work.
pub(crate) fn workspace_bytes(tile_bytes: usize) -> usize {
    rayon::current_num_threads().saturating_mul(tile_bytes)
}

/// Demosaic the `size` frame `data`, whose photosite at a point has the colour `color_at` gives,
/// in tiles placed by `tiling`: each worker makes its tile with `new_tile` and runs `demosaic` on
/// it at each place, which writes into the planes the part of the place `tiling.margin` or more
/// inside its edges, and nothing else. The border comes from [`fill_border`].
///
/// # Errors
///
/// [`Cancelled`] when `cancel` is set, between tiles and before the border.
pub(crate) fn demosaic<T>(
    data: &[f32],
    size: Size2us,
    color_at: impl Fn(Vec2us) -> usize + Sync,
    tiling: Tiling,
    new_tile: impl Fn() -> T + Sync + Send,
    demosaic: impl Fn(&mut T, TilePlace, OutputPlanes) + Sync,
    cancel: &CancelToken,
) -> Result<[Vec<f32>; 3], Cancelled> {
    let pixels = size.pixel_count();
    let mut planes = [
        vec![0.0f32; pixels],
        vec![0.0f32; pixels],
        vec![0.0f32; pixels],
    ];
    let places = tiling.places(size);
    let out = OutputPlanes {
        planes: planes
            .each_mut()
            .map(|plane| UnsafeSendPtr::new(plane.as_mut_ptr())),
    };
    places
        .par_iter()
        .try_for_each_init(new_tile, |tile, &place| {
            Cancelled::check(cancel)?;
            demosaic(tile, place, out);
            Ok(())
        })?;
    Cancelled::check(cancel)?;
    let border = if places.is_empty() {
        size.width.max(size.height)
    } else {
        tiling.border()
    };
    let [r, g, b] = &mut planes;
    fill_border(data, size, color_at, [r, g, b], border);
    Ok(planes)
}

/// Fill the pixels within `border` of the frame's edge of `out`, row by row in parallel: the
/// whole of each row in the band at the top and bottom, and the ends of each row between.
fn fill_border(
    data: &[f32],
    size: Size2us,
    color_at: impl Fn(Vec2us) -> usize + Sync,
    [out_r, out_g, out_b]: [&mut [f32]; 3],
    border: usize,
) {
    let Size2us { width, height } = size;
    out_r
        .par_chunks_mut(width)
        .zip(out_g.par_chunks_mut(width))
        .zip(out_b.par_chunks_mut(width))
        .enumerate()
        .for_each(|(y, ((row_r, row_g), row_b))| {
            let mut fill_span = |span: Range<usize>| {
                for x in span {
                    let [r, g, b] = border_pixel(data, size, &color_at, x, y);
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

/// Each colour at `(x, y)` of the border: the pixel's own sample in its colour, and each other
/// colour from its 3×3 neighbours of that colour, those beside it weighted twice the diagonal ones;
/// where none of those is of the colour, the mean of its samples in the smallest square window
/// around the pixel that holds any.
///
/// A mean of one colour's samples is the same whatever gain balances that colour, so the border
/// reads the input as it is, and its native samples are the input's own.
fn border_pixel(
    data: &[f32],
    size: Size2us,
    color_at: &impl Fn(Vec2us) -> usize,
    x: usize,
    y: usize,
) -> [f32; 3] {
    let Size2us { width, height } = size;
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
            let color = color_at(Vec2us::new(neighbor_x, neighbor_y));
            sums[color] += data[neighbor_y * width + neighbor_x] * weight;
            weights[color] += weight;
        }
    }
    let native = color_at(Vec2us::new(x, y));
    let sample = data[y * width + x];
    let channel = |color: usize| {
        if color == native {
            sample
        } else if weights[color] > 0.0 {
            sums[color] / weights[color]
        } else {
            nearest_mean(data, size, color_at, x, y, color).unwrap_or(sample)
        }
    };
    [channel(0), channel(1), channel(2)]
}

/// The mean of `color`'s samples in the smallest square window around `(x, y)` that holds any, for
/// a border pixel whose 3×3 neighbourhood has none. `None` only when the whole frame has no sample
/// of `color`, where there is nothing to interpolate from and the caller keeps the pixel's own
/// sample.
fn nearest_mean(
    data: &[f32],
    size: Size2us,
    color_at: &impl Fn(Vec2us) -> usize,
    x: usize,
    y: usize,
    color: usize,
) -> Option<f32> {
    let Size2us { width, height } = size;
    (2..width.max(height)).find_map(|radius| {
        let mut sum = 0.0f32;
        let mut count = 0usize;
        for neighbor_y in y.saturating_sub(radius)..=(y + radius).min(height - 1) {
            for neighbor_x in x.saturating_sub(radius)..=(x + radius).min(width - 1) {
                if color_at(Vec2us::new(neighbor_x, neighbor_y)) == color {
                    sum += data[neighbor_y * width + neighbor_x];
                    count += 1;
                }
            }
        }
        (count > 0).then(|| sum / count as f32)
    })
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::concurrency::unsafe_send_ptr::UnsafeSendPtr;
    use crate::io::raw::demosaic::tiled::OutputPlanes;

    impl OutputPlanes {
        /// The output planes over `planes`, for a test that runs one tile itself.
        pub(crate) fn of(planes: &mut [Vec<f32>; 3]) -> Self {
            Self {
                planes: planes
                    .each_mut()
                    .map(|plane| UnsafeSendPtr::new(plane.as_mut_ptr())),
            }
        }
    }
}
