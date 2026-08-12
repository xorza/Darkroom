//! How much of the machine the pipeline is willing to use, and what that buys.
//!
//! One budget ([`memory_budget`]) and everything derived from it: per-frame footprints, the row
//! chunk a combine reads at a time, how many frames may decode or warp concurrently, and the
//! resident-vs-spilled tier decision. Every caller sizes its work against this file rather than
//! against `available_memory` directly.

use crate::io::image::image_dimensions::ImageDimensions;
use crate::io::raw::demosaic::DemosaicMemory;
use crate::math::size2us::Size2us;

/// Share of available RAM the pipeline will commit, leaving the rest as headroom for allocator
/// slack, the OS page cache, and whatever else the machine is doing.
const MEMORY_PERCENT: u64 = 75;

pub(crate) fn available_memory() -> u64 {
    use std::sync::{LazyLock, Mutex};
    use sysinfo::System;

    // Reused rather than built per call. Constructing a `System` costs ~20 µs against ~5 µs to
    // refresh one that already exists, and asking for memory alone
    // (`new_with_specifics(RefreshKind::nothing().with_memory(..))`) measured no cheaper — the cost
    // is the construction itself, so reuse is the only thing that helps. Every planning decision
    // in the pipeline goes through here.
    static SYSTEM: LazyLock<Mutex<System>> = LazyLock::new(|| Mutex::new(System::new()));

    // Recover rather than propagate: a poisoned lock means some earlier caller panicked, but the
    // `System` behind it is a cache of OS counters with no invariant to corrupt.
    let mut system = SYSTEM
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    system.refresh_memory();
    let available = system.available_memory();

    // macOS can report zero when compressed pages exceed free, inactive, and purgeable pages.
    if available == 0 {
        system.total_memory().saturating_sub(system.used_memory())
    } else {
        available
    }
}

pub(crate) fn memory_budget(available_memory: u64) -> u64 {
    (available_memory as u128 * MEMORY_PERCENT as u128 / 100) as u64
}

/// Bytes one frame's pixels occupy, planar f32.
pub(crate) fn frame_bytes(dimensions: ImageDimensions) -> usize {
    dimensions.sample_count() * size_of::<f32>()
}

/// Statistics hold a full-frame scratch buffer beside the decoded pixels.
const DECODE_TRANSIENT_FACTOR: usize = 2;

/// Peak bytes one in-flight decode holds, pixels plus its transient scratch.
pub(crate) fn decode_transient_bytes(dimensions: ImageDimensions) -> usize {
    DECODE_TRANSIENT_FACTOR * frame_bytes(dimensions)
}

const MIN_CHUNK_ROWS: usize = 64;

/// What one combine pass holds in memory, in planes, so [`ChunkMemoryLayout::optimal_chunk_rows`]
/// can price a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ChunkMemoryLayout {
    /// Planes read concurrently for the active row chunk.
    pub(crate) input_planes: usize,
    /// Full image-sized planes held throughout chunk processing.
    pub(crate) resident_planes: usize,
}

impl ChunkMemoryLayout {
    /// Rows a combine may hold at once: the budget left after the resident planes, divided by the
    /// cost of a row, floored at [`MIN_CHUNK_ROWS`] so a tight budget still makes progress.
    pub(crate) fn optimal_chunk_rows(self, size: Size2us, available_memory: u64) -> usize {
        let bytes_per_row = size
            .width
            .checked_mul(self.input_planes)
            .and_then(|value| value.checked_mul(size_of::<f32>()))
            .map(|value| value as u64)
            .unwrap_or(u64::MAX);
        if bytes_per_row == 0 {
            return MIN_CHUNK_ROWS;
        }
        let resident_bytes = size
            .width
            .checked_mul(size.height)
            .and_then(|value| value.checked_mul(self.resident_planes))
            .and_then(|value| value.checked_mul(size_of::<f32>()))
            .map(|value| value as u64)
            .unwrap_or(u64::MAX);
        (memory_budget(available_memory).saturating_sub(resident_bytes) / bytes_per_row)
            .max(MIN_CHUNK_ROWS as u64) as usize
    }
}

/// Frames that may be in flight at once: budget minus what stays resident, divided by the peak
/// one in-flight frame adds, capped at the worker count and never below one.
pub(crate) fn load_concurrency(
    resident_bytes_per_frame: usize,
    transient_bytes_per_decode: usize,
    resident_frames: usize,
    available_memory: u64,
    max_workers: usize,
) -> usize {
    let usable = memory_budget(available_memory);
    let transient = (transient_bytes_per_decode as u64).max(1);
    let resident = (resident_bytes_per_frame as u64).saturating_mul(resident_frames as u64);
    let headroom = usable.saturating_sub(resident);
    ((headroom / transient).max(1) as usize).min(max_workers.max(1))
}

/// Whether `frame_count` images of `bytes_per_image` fit the budget all at once.
pub(crate) fn fits_in_memory(
    bytes_per_image: usize,
    frame_count: usize,
    available_memory: u64,
) -> bool {
    bytes_per_image
        .checked_mul(frame_count)
        .is_some_and(|bytes| bytes as u64 <= memory_budget(available_memory))
}

/// Quality planes a frame carries beside its image: `coverage` and `confidence`, one image-sized
/// plane each. A warp emits them, and so does a decoder that found pixels the source declared no
/// measurement for — see `registration::resample::WarpResult` and `frame_store::FrameQuality`.
const FRAME_QUALITY_PLANES: usize = 2;

/// Bytes the quality planes add to a frame that carries them.
///
/// One plane per pixel rather than per sample: coverage and confidence are channel-independent, so
/// an RGB frame pays for two planes here, not six.
pub(crate) fn quality_plane_bytes(dimensions: ImageDimensions) -> usize {
    FRAME_QUALITY_PLANES * dimensions.pixel_count() * size_of::<f32>()
}

/// Image-sized planes the star detector's pool holds at its high-water mark: six f32 planes, the
/// u32 label map, and the threshold bitmask. The bitmask is a thirty-second of an f32 plane;
/// charging it as a whole one keeps this integral and errs high.
///
/// `star_detection::mem_budget_tests` pins the detector's actual pool and checks it against this,
/// so a stage that grows its scratch cannot drift away from the planner silently.
pub(crate) const DETECTION_WORKING_PLANES: usize = 8;

/// What one frame costs the detect-register-warp stage, derived from the decoded frame rather
/// than assumed — so a mono frame is not charged for channels it does not have, and a demosaiced
/// three-channel one is charged for all of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PerFrameBytes {
    /// Resident once warped: the frame's own pixels plus its two quality planes.
    pub(crate) warped: usize,
    /// Peak transient one in-flight frame adds, whichever stage wants more — the warp, which
    /// holds the source and the warped output together until it drops the source, or detection
    /// with its whole scratch pool.
    pub(crate) working: usize,
}

impl PerFrameBytes {
    pub(crate) fn new(plane_bytes: usize, demosaic: DemosaicMemory) -> Self {
        let warped = demosaic
            .output_bytes
            .saturating_add(FRAME_QUALITY_PLANES.saturating_mul(plane_bytes));
        Self {
            warped,
            working: demosaic
                .output_bytes
                .saturating_add(warped)
                .max(DETECTION_WORKING_PLANES.saturating_mul(plane_bytes)),
        }
    }
}

/// The tier decision for one run, plus the concurrency each stage may use under it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MemoryPlan {
    pub(crate) fits_in_ram: bool,
    pub(crate) decode_concurrency: usize,
    pub(crate) warp_concurrency: usize,
}

impl MemoryPlan {
    /// The plan for a run handed frames that are already decoded and resident.
    ///
    /// [`Self::plan`] models a run that decodes its own frames, and charges the decode both an
    /// output and a transient peak. There is no decode here, so the frame's own bytes are the whole
    /// of it and the transient arena is nothing — which is what this expresses, rather than leaving
    /// each caller to encode "already decoded" as a [`DemosaicMemory`] whose two halves happen to
    /// be equal. `decode_concurrency` has no meaning under that and the callers do not read it.
    pub(crate) fn for_decoded_frames(
        dimensions: ImageDimensions,
        frame_count: usize,
        threads: usize,
        available: u64,
    ) -> Self {
        let decoded = frame_bytes(dimensions);
        Self::plan(
            dimensions.pixel_count() * size_of::<f32>(),
            DemosaicMemory {
                output_bytes: decoded,
                peak_bytes: decoded,
            },
            frame_count,
            threads,
            available,
        )
    }

    /// Decide whether a run stays resident or spills, and how wide decode and warp may fan out.
    ///
    /// The run fits in RAM only when both peaks do: decoding every frame (plus the demosaic's own
    /// transient) and holding every warped frame while `workers` of them are being worked on.
    pub(crate) fn plan(
        plane_bytes: usize,
        demosaic: DemosaicMemory,
        frame_count: usize,
        threads: usize,
        available: u64,
    ) -> Self {
        assert!(
            frame_count > 0,
            "memory planning requires at least one frame"
        );
        let workers = frame_count.min(threads.max(1));
        let per_frame = PerFrameBytes::new(plane_bytes, demosaic);
        let decode_extra = demosaic.peak_bytes.saturating_sub(demosaic.output_bytes);
        let usable = memory_budget(available);

        let decoded_resident = (demosaic.output_bytes as u64).saturating_mul(frame_count as u64);
        let decode_minimum = decoded_resident.saturating_add(decode_extra as u64);
        let warped_resident = (per_frame.warped as u64).saturating_mul(frame_count as u64);
        let working_peak = warped_resident
            .saturating_add((per_frame.working as u64).saturating_mul(workers as u64));
        let fits_in_ram = decode_minimum.max(working_peak) <= usable;

        let (decode_resident_frames, decode_bytes) = if fits_in_ram {
            (frame_count, decode_extra)
        } else {
            (0, demosaic.peak_bytes.max(per_frame.working))
        };
        let warp_resident_frames = usize::from(fits_in_ram) * frame_count;
        let decode_concurrency = load_concurrency(
            demosaic.output_bytes,
            decode_bytes,
            decode_resident_frames,
            available,
            workers,
        );
        let warp_concurrency = load_concurrency(
            per_frame.warped,
            per_frame.working,
            warp_resident_frames,
            available,
            workers,
        );
        Self {
            fits_in_ram,
            decode_concurrency,
            warp_concurrency,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::memory::*;

    const GB: u64 = 1024 * 1024 * 1024;
    const FRAME_96MB: usize = 6240 * 4160 * size_of::<f32>();

    /// The `System` behind this is a process-wide singleton reused across calls, so the second
    /// call must be as good as the first — a stale or half-refreshed instance would show up as a
    /// zero or as a value that stops tracking.
    #[test]
    fn available_memory_reports_a_plausible_figure_on_every_call() {
        let first = available_memory();
        let second = available_memory();

        assert!(first > 0, "no available memory reported");
        assert!(second > 0, "second call reported none");
        // Both are sampled within microseconds of each other on an otherwise-idle test process, so
        // they cannot differ by more than a small fraction without the refresh being broken.
        let (low, high) = (first.min(second), first.max(second));
        assert!(
            high - low < high / 4,
            "consecutive samples disagree wildly: {first} then {second}"
        );
        // A budget derived from it stays inside the machine, which a garbage reading would not.
        assert!(memory_budget(first) < first);
    }

    #[test]
    fn quality_planes_are_charged_per_pixel_not_per_sample() {
        // Coverage and confidence are channel-independent, so an RGB frame carries two of them, not
        // six. Charging per sample would triple the figure and push a masked colour stack to disk
        // for planes it never allocates.
        let mono = ImageDimensions::new((100, 50), 1);
        let rgb = ImageDimensions::new((100, 50), 3);
        assert_eq!(quality_plane_bytes(mono), 2 * 100 * 50 * 4);
        assert_eq!(quality_plane_bytes(rgb), quality_plane_bytes(mono));

        // Against the frame's own pixels, which *are* per sample: a masked mono frame is three
        // planes resident and a masked RGB one is five, not six.
        assert_eq!(
            frame_bytes(mono) + quality_plane_bytes(mono),
            3 * 100 * 50 * 4
        );
        assert_eq!(
            frame_bytes(rgb) + quality_plane_bytes(rgb),
            5 * 100 * 50 * 4
        );
    }

    #[test]
    fn memory_budget_keeps_one_quarter_as_headroom_without_overflow() {
        assert_eq!(
            memory_budget(8 * 1024 * 1024 * 1024),
            6 * 1024 * 1024 * 1024
        );
        assert_eq!(memory_budget(u64::MAX), 13_835_058_055_282_163_711);
    }

    #[test]
    fn load_concurrency_accounts_for_resident_and_transient_memory() {
        let cases = [
            (FRAME_96MB, 2 * FRAME_96MB, 20, 27 * GB, 16, 16),
            (FRAME_96MB, 2 * FRAME_96MB, 200, 25 * GB, 16, 1),
            (GB as usize, GB as usize, 0, 4 * GB, 64, 3),
            (GB as usize, 2 * GB as usize, 0, 4 * GB, 64, 1),
            (FRAME_96MB, 2 * FRAME_96MB, 0, 4 * GB, 8, 8),
            (GB as usize, GB as usize, 0, 2 * GB, 16, 1),
            (GB as usize, GB as usize, 0, 8 * GB, 16, 6),
            (0, 0, 0, 0, 16, 1),
            (FRAME_96MB, 2 * FRAME_96MB, 5, 27 * GB, 0, 1),
        ];

        for (resident, transient, frames, available, workers, expected) in cases {
            assert_eq!(
                load_concurrency(resident, transient, frames, available, workers),
                expected
            );
        }
    }

    #[test]
    fn fits_in_memory_honors_budget_boundary_channels_and_overflow() {
        let bytes_per_image = 1000 * 1000 * size_of::<f32>();
        let frame_count = 10;
        let bytes_needed = (bytes_per_image * frame_count) as u64;
        let available_at_boundary = (bytes_needed * 100).div_ceil(75);

        assert!(fits_in_memory(
            bytes_per_image,
            frame_count,
            available_at_boundary
        ));
        assert!(!fits_in_memory(
            bytes_per_image,
            frame_count,
            available_at_boundary - 2
        ));
        assert!(fits_in_memory(6000 * 4000 * 4, 20, 4 * GB));
        assert!(!fits_in_memory(6000 * 4000 * 3 * 4, 20, 4 * GB));
        assert!(!fits_in_memory(usize::MAX, 2, u64::MAX));
    }

    #[test]
    fn optimal_chunk_rows_matches_budget_arithmetic() {
        let cases = [
            (6000, 3, 20, 8 * GB),
            (1000, 3, 5, 4 * GB),
            (8000, 3, 100, 16 * GB),
            (6000, 3, 20, GB),
            (6000, 3, 20, 256 * 1024 * 1024),
            (6000, 1, 20, 8 * GB),
            (100, 1, 2, 0),
        ];

        for (width, channels, frames, available) in cases {
            let input_planes = channels * frames;
            let bytes_per_row = (width * input_planes * size_of::<f32>()) as u64;
            let usable = (available as u128 * 75 / 100) as u64;
            let expected = (usable / bytes_per_row).max(MIN_CHUNK_ROWS as u64) as usize;
            assert_eq!(
                ChunkMemoryLayout {
                    input_planes,
                    resident_planes: 0,
                }
                .optimal_chunk_rows(Size2us::new(width, 100), available),
                expected
            );
        }

        // 1 MiB available → 786,432 usable bytes. Six resident 100×200 f32 planes consume 480,000
        // bytes; nine active input planes consume 3,600 bytes/row, leaving exactly 85 whole rows.
        assert_eq!(
            ChunkMemoryLayout {
                input_planes: 9,
                resident_planes: 6,
            }
            .optimal_chunk_rows(Size2us::new(100, 200), 1024 * 1024),
            85
        );
        assert_eq!(
            ChunkMemoryLayout {
                input_planes: 60,
                resident_planes: 3,
            }
            .optimal_chunk_rows(Size2us::new(0, 100), 8 * GB),
            MIN_CHUNK_ROWS
        );
        assert_eq!(
            ChunkMemoryLayout {
                input_planes: 9,
                resident_planes: 10,
            }
            .optimal_chunk_rows(Size2us::new(100, 200), 1024 * 1024),
            MIN_CHUNK_ROWS
        );
        assert_eq!(memory_budget(8 * GB), 6 * GB);
    }
}
