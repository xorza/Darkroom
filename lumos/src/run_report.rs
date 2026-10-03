//! [`RunReport`]: what a run decided without failing, so that no decision is silent.

use std::sync::atomic::{AtomicU64, Ordering};

use crate::io::image::pixel_flags::Flags;

/// The decisions a run took on its own and did not fail over, returned with its result.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RunReport {
    /// Channel samples the combine left out because their pixel carried the flag, with enough
    /// unflagged samples left at that pixel to keep the minimum survivor count.
    pub excluded_samples: FlagCounts,
    /// Channel samples the combine kept although their pixel carried the flag, because leaving
    /// them out would have dropped the pixel below the minimum survivor count.
    pub kept_flagged_samples: FlagCounts,
    /// The variance plane holds the background noise only: some frame did not state its gain, so
    /// the photon noise of the signal above the sky is missing from it.
    pub variance_background_only: bool,
}

/// A count per data-quality flag a combine acts on. A sample with two flags counts under both.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FlagCounts {
    pub saturated: u64,
    pub defect: u64,
    pub cosmic_ray: u64,
    pub repaired: u64,
    pub flat_floor: u64,
}

/// The flags a [`FlagCounts`] counts, in its field order.
const COUNTED: [Flags; 5] = [
    Flags::SATURATED,
    Flags::DEFECT,
    Flags::COSMIC_RAY,
    Flags::REPAIRED,
    Flags::FLAT_FLOOR,
];

/// One worker's [`FlagCounts`], in [`COUNTED`] order.
#[derive(Debug, Default)]
pub(crate) struct LocalFlagCounts([u64; 5]);

impl LocalFlagCounts {
    /// Count a sample carrying `flags` under each counted flag it holds.
    #[inline]
    pub(crate) fn count(&mut self, flags: Flags) {
        for (count, flag) in self.0.iter_mut().zip(COUNTED) {
            *count += u64::from(flags.intersects(flag));
        }
    }
}

/// [`FlagCounts`] that parallel workers add into.
#[derive(Debug, Default)]
pub(crate) struct AtomicFlagCounts([AtomicU64; 5]);

impl AtomicFlagCounts {
    pub(crate) fn add(&self, local: &LocalFlagCounts) {
        for (total, &count) in self.0.iter().zip(&local.0) {
            if count > 0 {
                total.fetch_add(count, Ordering::Relaxed);
            }
        }
    }

    pub(crate) fn totals(&self) -> FlagCounts {
        let [saturated, defect, cosmic_ray, repaired, flat_floor] =
            self.0.each_ref().map(|count| count.load(Ordering::Relaxed));
        FlagCounts {
            saturated,
            defect,
            cosmic_ray,
            repaired,
            flat_floor,
        }
    }
}
