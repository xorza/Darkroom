//! How much of the machine the pipeline is willing to use, and what that buys.
//!
//! One budget ([`memory_budget`]) and everything derived from it: per-frame footprints, the row
//! chunk a combine reads at a time, how many frames may decode or warp concurrently, and the
//! resident-vs-spilled tier decision. Every caller sizes its work against this file rather than
//! against `available_memory` directly, through the one [`RunMemory`](run_memory::RunMemory) its
//! run read at the entry.

pub(crate) mod run_memory;

use crate::io::image::image_dimensions::ImageDimensions;
use crate::io::raw::demosaic::DemosaicMemory;
use crate::math::size2us::Size2us;
use std::sync::PoisonError;

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
    let mut system = SYSTEM.lock().unwrap_or_else(PoisonError::into_inner);
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
    (u128::from(available_memory) * u128::from(MEMORY_PERCENT) / 100) as u64
}

/// Bytes one frame's pixels occupy, planar f32.
pub(crate) const fn frame_bytes(dimensions: ImageDimensions) -> usize {
    dimensions.sample_count() * size_of::<f32>()
}

/// Statistics hold a full-frame scratch buffer beside the decoded pixels.
pub(crate) const DECODE_TRANSIENT_FACTOR: usize = 2;

const MIN_CHUNK_ROWS: usize = 64;

/// What one combine pass holds in memory, so [`ChunkMemoryLayout::optimal_chunk_rows`] can price a
/// row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ChunkMemoryLayout {
    /// Bytes read per pixel of the active row chunk, across every plane read concurrently: four
    /// for each f32 plane, one for a flag plane.
    pub(crate) input_bytes: usize,
    /// Full image-sized planes held throughout chunk processing.
    pub(crate) resident_planes: usize,
}

impl ChunkMemoryLayout {
    /// Rows a combine may hold at once: the budget left after the resident planes, divided by the
    /// cost of a row, floored at [`MIN_CHUNK_ROWS`] so a tight budget still makes progress.
    pub(crate) fn optimal_chunk_rows(self, size: Size2us, available_memory: u64) -> usize {
        let bytes_per_row = size
            .width
            .checked_mul(self.input_bytes)
            .map_or(u64::MAX, |value| value as u64);
        if bytes_per_row == 0 {
            return MIN_CHUNK_ROWS;
        }
        let resident_bytes = size
            .width
            .checked_mul(size.height)
            .and_then(|value| value.checked_mul(self.resident_planes))
            .and_then(|value| value.checked_mul(size_of::<f32>()))
            .map_or(u64::MAX, |value| value as u64);
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

/// Quality planes a frame carries beside its image: `coverage` and `confidence`, one image-sized
/// plane each. A warp emits them, and so does a decoder that found pixels the source declared no
/// measurement for — see `registration::resample::WarpResult` and `frame_store::FrameQuality`.
const FRAME_QUALITY_PLANES: usize = 2;

/// Bytes the quality planes add to a frame that carries them.
///
/// One plane per pixel rather than per sample: coverage and confidence are channel-independent, so
/// an RGB frame pays for two planes here, not six.
pub(crate) const fn quality_plane_bytes(dimensions: ImageDimensions) -> usize {
    FRAME_QUALITY_PLANES * dimensions.pixel_count() * size_of::<f32>()
}

/// Bytes a frame's flag plane adds: one per pixel, whatever its channel count. Charged to every
/// frame, since a decoder that flags saturation, and a calibration that flags its repairs, give one
/// to most frames.
pub(crate) const fn flag_plane_bytes(dimensions: ImageDimensions) -> usize {
    dimensions.pixel_count()
}

/// Image-sized planes the star detector's pool holds at its high-water mark over every preset:
/// four f32 planes (the residual, the sky noise, and the matched filter's output and pass
/// scratch), the u32 label map, and two bitmasks (saturation and threshold). A bitmask is a
/// thirty-second of an f32 plane; charging each as a whole one keeps this integral and errs high.
///
/// `star_detection::mem_budget` pins the detector's actual pool and checks it against this,
/// so a stage that grows its scratch cannot drift away from the planner silently.
pub(crate) const DETECTION_WORKING_PLANES: usize = 7;

/// What one frame costs the register-and-warp stage, derived from the decoded frame rather than
/// assumed — so a mono frame is not charged for channels it does not have, and a demosaiced
/// three-channel one is charged for all of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PerFrameBytes {
    /// Resident once warped: the frame's own pixels plus its two quality planes.
    pub(crate) warped: usize,
    /// What one in-flight warp holds: the source and the warped output together, until it drops
    /// the source. All of it is transient for a spilled frame; a resident frame's warped output is
    /// its share of the resident set, so it adds only the source beyond that.
    pub(crate) working: usize,
}

impl PerFrameBytes {
    /// For a frame of `output_bytes` whose planes are `plane_bytes` each, and its flag plane of one
    /// byte per pixel.
    pub(crate) const fn new(plane_bytes: usize, output_bytes: usize) -> Self {
        let warped = output_bytes
            .saturating_add(FRAME_QUALITY_PLANES.saturating_mul(plane_bytes))
            .saturating_add(plane_bytes / size_of::<f32>());
        Self {
            warped,
            working: output_bytes.saturating_add(warped),
        }
    }
}

/// What one run's frames cost, stage by stage: the input to [`MemoryPlan::plan`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RunShape {
    pub(crate) frame_count: usize,
    /// One frame's decode: what it leaves resident, and its peak on the way there.
    pub(crate) decode: DemosaicMemory,
    /// What of each frame the caller held before the run read the machine: the whole decoded frame
    /// for frames handed over in memory, zero for frames decoded from files. The reading already
    /// left those bytes out, so every figure that holds the frame is charged net of them, and the
    /// run frees them as it warps each frame.
    pub(crate) held_bytes: usize,
    /// What each concurrent decode holds beside it for the whole pass: the star detector's
    /// scratch pool, kept from frame to frame. Zero for a run that detects nothing.
    pub(crate) detection_bytes: usize,
    /// What one frame costs the detect-register-warp stage; `None` for a run that combines its
    /// frames as they decoded.
    pub(crate) warp: Option<PerFrameBytes>,
    /// What the combine holds resident beside its frames — see
    /// [`QualityPlanes::resident_bytes`](crate::QualityPlanes::resident_bytes).
    pub(crate) output_bytes: usize,
}

impl RunShape {
    /// A stack of frames decoded straight into the combine, with no warp: each one's resident
    /// bytes (its pixels, and its quality planes if it may carry them) plus the statistics
    /// scratch its decode holds beside them.
    pub(crate) const fn decoded_stack(
        frame_count: usize,
        resident_bytes: usize,
        frame_bytes: usize,
        output_bytes: usize,
    ) -> Self {
        Self {
            frame_count,
            decode: DemosaicMemory {
                output_bytes: resident_bytes,
                peak_bytes: resident_bytes
                    .saturating_add((DECODE_TRANSIENT_FACTOR - 1).saturating_mul(frame_bytes)),
            },
            held_bytes: 0,
            detection_bytes: 0,
            warp: None,
            output_bytes,
        }
    }
}

/// The tier decision for one run, plus the concurrency each stage may use under it — the one rule
/// every stacking entry decides with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MemoryPlan {
    pub(crate) fits_in_ram: bool,
    pub(crate) decode_concurrency: usize,
    /// For a run with no warp stage, the worker count: nothing to bound.
    pub(crate) warp_concurrency: usize,
}

impl MemoryPlan {
    /// Decide whether a run stays resident or spills, and how wide decode and warp may fan out.
    ///
    /// The run fits in RAM only when each of its peaks does: decoding every frame (plus one
    /// decode's transient and its detector), holding every warped frame while `workers` of them
    /// are being worked on, and holding every frame beside the combine's resident output planes.
    ///
    /// A resident frame's own bytes count once, in the resident set, so what each in-flight frame
    /// adds is only what it holds beyond them; a spilled frame's bytes are all transient.
    pub(crate) fn plan(shape: RunShape, threads: usize, available: u64) -> Self {
        let RunShape {
            frame_count,
            decode,
            held_bytes,
            detection_bytes,
            warp,
            output_bytes,
        } = shape;
        debug_assert!(
            held_bytes <= decode.output_bytes,
            "a held frame is at most the decoded frame"
        );
        assert!(
            frame_count > 0,
            "memory planning requires at least one frame"
        );
        let workers = frame_count.min(threads.max(1));
        let (warped, working) = warp.map_or((decode.output_bytes, 0), |per_frame| {
            (per_frame.warped, per_frame.working)
        });
        let usable = memory_budget(available);
        // What each frame adds once decoded and once warped, beyond what the caller held.
        let decoded = decode.output_bytes - held_bytes;
        let warped_added = warped - held_bytes;
        let resident_decode = decode
            .peak_bytes
            .saturating_sub(decode.output_bytes)
            .saturating_add(detection_bytes);
        // The source a worker warps stays alive beside the warped set until the warp ends, held or
        // not.
        let resident_warp = working.saturating_sub(warped);

        let decoded_resident = (decoded as u64).saturating_mul(frame_count as u64);
        let decode_minimum = decoded_resident.saturating_add(resident_decode as u64);
        let warped_resident = (warped_added as u64).saturating_mul(frame_count as u64);
        let working_peak =
            warped_resident.saturating_add((resident_warp as u64).saturating_mul(workers as u64));
        let combine_peak = warped_resident.saturating_add(output_bytes as u64);
        let fits_in_ram = decode_minimum.max(working_peak).max(combine_peak) <= usable;

        let (resident_frames, decode_bytes, warp_bytes) = if fits_in_ram {
            (frame_count, resident_decode, resident_warp)
        } else {
            (
                0,
                decode
                    .peak_bytes
                    .saturating_add(detection_bytes)
                    .saturating_sub(held_bytes),
                working.saturating_sub(held_bytes),
            )
        };
        let decode_concurrency =
            load_concurrency(decoded, decode_bytes, resident_frames, available, workers);
        let warp_concurrency = load_concurrency(
            warped_added,
            warp_bytes,
            resident_frames,
            available,
            workers,
        );
        Self {
            fits_in_ram,
            decode_concurrency,
            warp_concurrency,
        }
    }

    /// [`Self::plan`] for a run whose workers take each frame through the decode, the detection,
    /// the warp and the store in one go, with nothing parked between: the lights of a run whose
    /// reference is known.
    ///
    /// No decoded set is ever resident, only the warped one. A worker holds its detector's scratch
    /// throughout, and the larger of its decode's peak and the source it warps. On the spill tier it
    /// also keeps its warp buffers from one frame to the next. As in [`Self::plan`]'s decode pass,
    /// the run is resident when the warped set fits beside one worker, and the workers fan out as
    /// far as the rest allows. Both stages fan out alike, since every worker does both.
    pub(crate) fn single_pass(shape: RunShape, threads: usize, available: u64) -> Self {
        let RunShape {
            frame_count,
            decode,
            held_bytes,
            detection_bytes,
            warp,
            output_bytes,
        } = shape;
        assert!(
            frame_count > 0,
            "memory planning requires at least one frame"
        );
        debug_assert_eq!(held_bytes, 0, "a single pass decodes its own frames");
        let warp = warp.expect("a single pass warps its frames");
        let workers = frame_count.min(threads.max(1));
        let usable = memory_budget(available);
        let in_flight = decode
            .peak_bytes
            .max(decode.output_bytes)
            .saturating_add(detection_bytes);
        let warped_resident = (warp.warped as u64).saturating_mul(frame_count as u64);
        let working_minimum = warped_resident.saturating_add(in_flight as u64);
        let combine_peak = warped_resident.saturating_add(output_bytes as u64);
        let fits_in_ram = working_minimum.max(combine_peak) <= usable;
        let concurrency = if fits_in_ram {
            load_concurrency(warp.warped, in_flight, frame_count, available, workers)
        } else {
            load_concurrency(
                warp.warped,
                in_flight.saturating_add(warp.warped),
                0,
                available,
                workers,
            )
        };
        Self {
            fits_in_ram,
            decode_concurrency: concurrency,
            warp_concurrency: concurrency,
        }
    }
}

#[cfg(test)]
mod tests;
