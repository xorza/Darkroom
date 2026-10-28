//! The tiling both demosaics run on: where the tiles lie, the frame's output planes each tile
//! writes its own part of, and the memory a run holds.

use crate::memory::footprint::Footprint;
use crate::unsafe_send_ptr::UnsafeSendPtr;
use common::USize2;

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
    pub(crate) size: USize2,
}

impl TilePlace {
    /// The frame row past the tile's last.
    pub(crate) const fn bottom(self) -> usize {
        self.top + self.size.height
    }

    /// The frame column past the tile's last.
    pub(crate) const fn right(self) -> usize {
        self.left + self.size.width
    }
}

/// One pixel of the frame a kernel writes: its flat `idx`, its `native` color, and the `sample`
/// the input holds there.
#[derive(Debug, Clone, Copy)]
pub(crate) struct NativePixel {
    pub(crate) idx: usize,
    pub(crate) native: usize,
    pub(crate) sample: f32,
}

/// The frame's output planes, which every tile writes at the pixels it alone owns, and the gains
/// the kernels balanced the colors by, which a write takes back out.
#[derive(Debug, Clone, Copy)]
pub(crate) struct OutputPlanes {
    planes: [UnsafeSendPtr<f32>; 3],
    gains: [f32; 3],
}

impl Tiling {
    /// The pixels nearest the frame's edge that no tile writes: the inset, and a tile's margin.
    pub(crate) const fn border(self) -> usize {
        self.inset + self.margin
    }

    /// The tiles over a frame of `size`, overlapping by twice the margin so that the parts they
    /// write tile the frame within the border; none when the frame is all border.
    pub(crate) fn places(self, size: USize2) -> Vec<TilePlace> {
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
            (0..count).map(move |idx| self.inset + idx * step)
        };
        starts(size.height)
            .flat_map(|top| {
                starts(size.width).map(move |left| TilePlace {
                    top,
                    left,
                    size: USize2::new(
                        (size.width - self.inset - left).min(self.tile),
                        (size.height - self.inset - top).min(self.tile),
                    ),
                })
            })
            .collect()
    }
}

impl OutputPlanes {
    /// The planes over `planes`, written in the balance `gains` take out.
    pub(crate) fn new(planes: &mut [Vec<f32>; 3], gains: [f32; 3]) -> Self {
        Self {
            planes: planes
                .each_mut()
                .map(|plane| UnsafeSendPtr::new(plane.as_mut_ptr())),
            gains,
        }
    }

    /// `values` into plane `channel` from `idx` on, as they are: native samples, and values
    /// [`Self::unbalanced`] brought back.
    ///
    /// # Safety
    ///
    /// The planes cover the frame, and no other tile writes the pixels from `idx` to
    /// `idx + values.len()`.
    #[inline(always)]
    pub(super) const unsafe fn write_row(self, channel: usize, idx: usize, values: &[f32]) {
        unsafe {
            self.planes[channel]
                .get()
                .add(idx)
                .copy_from_nonoverlapping(values.as_ptr(), values.len());
        }
    }

    /// `value` of `channel`, as a kernel computed it in balanced units, in the frame's own
    /// balance.
    #[inline(always)]
    pub(crate) fn unbalanced(self, channel: usize, value: f32) -> f32 {
        value / self.gains[channel]
    }

    /// The three channels of `pixel`: each of `values`, as a kernel computed it in balanced units,
    /// brought back by its channel's gain — but at the pixel's native color its own sample, as
    /// the input held it, so it leaves exact.
    ///
    /// # Safety
    ///
    /// The planes cover the frame, and no other tile writes the pixel.
    #[inline(always)]
    pub(super) unsafe fn write_pixel(self, pixel: NativePixel, values: [f32; 3]) {
        let NativePixel {
            idx,
            native,
            sample,
        } = pixel;
        for (channel, value) in values.into_iter().enumerate() {
            let value = if channel == native {
                sample
            } else {
                self.unbalanced(channel, value)
            };
            unsafe { self.planes[channel].get().add(idx).write(value) };
        }
    }
}

/// The output's three planes, and the peak: the caller's input, the output, and the buffers of
/// `tile_bytes` each of `workers` holds. Demosaics running at once share the workers, so the charge
/// to each is an upper bound.
pub(crate) const fn footprint(size: USize2, tile_bytes: usize, workers: usize) -> Footprint {
    let plane_bytes = size
        .width
        .saturating_mul(size.height)
        .saturating_mul(size_of::<f32>());
    let output_bytes = plane_bytes.saturating_mul(3);
    Footprint {
        kept: output_bytes,
        peak: plane_bytes
            .saturating_add(output_bytes)
            .saturating_add(workspace_bytes(tile_bytes, workers)),
    }
}

/// The tile buffers of `workers`, at most one each: a worker makes its tile for a run of tiles and
/// drops it before it takes other work.
pub(crate) const fn workspace_bytes(tile_bytes: usize, workers: usize) -> usize {
    workers.saturating_mul(tile_bytes)
}

/// How a kernel runs over a frame's tiles: where they lie, how a worker makes the tile it works
/// in, and `run`, which demosaics one place with it.
#[derive(Debug)]
pub(crate) struct TileKernel<N, R> {
    pub(crate) tiling: Tiling,
    pub(crate) new_tile: N,
    pub(crate) run: R,
}
