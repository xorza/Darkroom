//! Shared decode policy: cancellation, resource ceilings, and format options.

use std::path::Path;

use common::CancelToken;

use crate::io::image::error::ImageError;
use crate::io::image::fits::options::FitsLoadOptions;
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
}

impl LoadContext {
    /// Creates a context with strict FITS defaults and the supplied resource controls.
    pub fn new(cancel: CancelToken, memory_limit_bytes: u64) -> Self {
        Self {
            cancel,
            memory_limit_bytes,
            fits: FitsLoadOptions::default(),
        }
    }

    pub(crate) fn check_cancelled(&self, path: &Path) -> Result<(), ImageError> {
        if self.cancel.is_cancelled() {
            return Err(ImageError::Cancelled {
                path: path.to_path_buf(),
            });
        }
        Ok(())
    }
}

impl Default for LoadContext {
    fn default() -> Self {
        Self::new(
            CancelToken::never(),
            memory::memory_budget(memory::available_memory()),
        )
    }
}
