//! Concurrency helpers for Rayon work.

pub(crate) mod unsafe_send_ptr;

use std::mem::MaybeUninit;

use common::par;
use rayon::prelude::*;

/// Samples per rayon work item. Parallelizing per sample drowns a cheap per-pixel op in rayon's
/// recursive split/join overhead (it dominated SCNR); a coarse block amortizes that while staying
/// load-balanced and letting the inner loop auto-vectorize.
pub(crate) const SAMPLES_PER_BLOCK: usize = 8192;

/// How many samples a cancellable walk covers between cancel polls.
///
/// The frame checks and the normalization gather chunk their iteration to this rather than test
/// the token per sample. The figure is not tuned to either: it is a granularity, small enough that
/// a cancelled run stops promptly and large enough that the poll is lost in the work.
pub(crate) const CANCEL_POLL_CHUNK: usize = 16_384;

/// The `f32` samples of a 2 MiB transparent huge page.
const HUGE_PAGE_SAMPLES: usize = 2 * 1024 * 1024 / size_of::<f32>();

/// `len` zeros, written by the workers a 2 MiB block each, for a plane workers then fill in
/// contiguous pieces smaller than a page. A fresh plane's pages are zeroed by the kernel at the
/// first write to each; when such pieces make those first writes, several workers fault on one
/// huge page and wait on each other, which took two thirds of a 24 MP FITS decode. Where the first
/// writes already spread over the pages, as a demosaic's tiles do, this pass only adds work.
pub(crate) fn zeroed_in_parallel(len: usize) -> Vec<f32> {
    let mut plane = Vec::with_capacity(len);
    par::chunks_mut(&mut plane.spare_capacity_mut()[..len], HUGE_PAGE_SAMPLES)
        .for_each(|block| block.fill(MaybeUninit::new(0.0)));
    unsafe { plane.set_len(len) };
    plane
}

#[cfg(test)]
mod tests;
