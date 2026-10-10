//! Shared decode policy: cancellation, resource ceilings, and format options.

use std::path::Path;

use common::CancelToken;

use crate::io::image::error::ImageError;
use crate::io::image::fits::options::FitsLoadOptions;
use crate::io::raw::demosaic::xtrans::markesteijn::MarkesteijnPasses;
use crate::memory;

/// Cancellation, resource controls, and format policy shared by file decoders.
#[derive(Debug, Clone)]
pub struct LoadContext {
    /// Cooperative cancellation token polled between bounded decode stages.
    pub cancel: CancelToken,
    /// FITS source, output, and estimated peak byte ceiling.
    pub memory_limit_bytes: u64,
    /// FITS-specific policy; ignored by non-FITS decoders.
    pub fits: FitsLoadOptions,
    /// How many passes an X-Trans demosaic makes; ignored for every other mosaic.
    pub xtrans_passes: MarkesteijnPasses,
    /// How many threads one decode may run inside a decoder that parallelizes itself — LibRaw's
    /// tiled CR3, compressed Fuji and Panasonic v8 decoders. Every thread rayon has for one decode
    /// at a time; a run decoding several frames at once shares them out, so the decodes together
    /// stay within the cores.
    pub decode_threads: usize,
}

impl LoadContext {
    /// Creates a context with strict FITS defaults and the supplied resource controls.
    pub fn new(cancel: CancelToken, memory_limit_bytes: u64) -> Self {
        Self {
            cancel,
            memory_limit_bytes,
            fits: FitsLoadOptions::default(),
            xtrans_passes: MarkesteijnPasses::default(),
            decode_threads: rayon::current_num_threads(),
        }
    }

    /// This context for `slots` decodes running at once: each with an equal share of rayon's
    /// threads, one at least — see [`Self::decode_threads`].
    pub(crate) fn for_decode_slots(&self, slots: usize) -> Self {
        Self {
            decode_threads: threads_per_decode(rayon::current_num_threads(), slots),
            ..self.clone()
        }
    }

    pub(crate) fn check_cancelled(&self, path: &Path) -> Result<(), ImageError> {
        if self.cancel.is_cancelled() {
            return Err(ImageError::cancelled(path));
        }
        Ok(())
    }
}

/// The threads each of `slots` decodes running at once gets of `cores`: an equal share, one at
/// least, so the decodes together use the cores and no more.
const fn threads_per_decode(cores: usize, slots: usize) -> usize {
    assert!(slots > 0, "a run decodes in one slot at least");
    let share = cores / slots;
    if share == 0 { 1 } else { share }
}

impl Default for LoadContext {
    fn default() -> Self {
        Self::new(
            CancelToken::never(),
            memory::memory_budget(memory::available_memory()),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sixteen cores over one decode give it all sixteen, over four slots four each, over five
    /// three (15 of 16, never 20), and over more slots than cores one each.
    #[test]
    fn decodes_share_the_cores() {
        for (cores, slots, threads) in [(16, 1, 16), (16, 4, 4), (16, 5, 3), (4, 8, 1), (1, 1, 1)] {
            assert_eq!(
                threads_per_decode(cores, slots),
                threads,
                "{cores} cores over {slots}"
            );
        }
    }
}
