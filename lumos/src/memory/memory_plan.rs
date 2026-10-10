//! [`MemoryPlan`]: the tier decision for one run, and the concurrency each stage may use under it.

use crate::io::raw::demosaic::DemosaicMemory;
use crate::memory;
use crate::memory::{DECODE_TRANSIENT_FACTOR, FRAME_QUALITY_PLANES};

/// What one frame costs the register-and-warp stage, derived from the decoded frame rather than
/// assumed — so a mono frame is not charged for channels it does not have, and a demosaiced
/// three-channel one is charged for all of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PerFrameBytes {
    /// Resident once warped: the frame's own pixels plus its two quality planes, its flags and its
    /// flat gain grid.
    pub(crate) warped: usize,
    /// What one in-flight warp holds: the source and the warped output together, until it drops
    /// the source. All of it is transient for a spilled frame; a resident frame's warped output is
    /// its share of the resident set, so it adds only the source beyond that.
    pub(crate) working: usize,
}

impl PerFrameBytes {
    /// For a frame of `output_bytes` whose planes are `plane_bytes` each, its flag plane of one
    /// byte per pixel, and its flat gain grid of `gain_bytes`.
    pub(crate) const fn new(plane_bytes: usize, output_bytes: usize, gain_bytes: usize) -> Self {
        let warped = output_bytes
            .saturating_add(FRAME_QUALITY_PLANES.saturating_mul(plane_bytes))
            .saturating_add(plane_bytes / size_of::<f32>())
            .saturating_add(gain_bytes);
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
        let usable = memory::memory_budget(available);
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
            memory::load_concurrency(decoded, decode_bytes, resident_frames, available, workers);
        let warp_concurrency = memory::load_concurrency(
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
    /// throughout, and the larger of its decode's peak and the source it warps. On the spill tier
    /// it also keeps its warp buffers from one frame to the next. As in [`Self::plan`]'s decode
    /// pass, the run is resident when the warped set fits beside one worker, and the workers fan
    /// out as far as the rest allows. Both stages fan out alike, since every worker does both.
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
        let usable = memory::memory_budget(available);
        let in_flight = decode
            .peak_bytes
            .max(decode.output_bytes)
            .saturating_add(detection_bytes);
        let warped_resident = (warp.warped as u64).saturating_mul(frame_count as u64);
        let working_minimum = warped_resident.saturating_add(in_flight as u64);
        let combine_peak = warped_resident.saturating_add(output_bytes as u64);
        let fits_in_ram = working_minimum.max(combine_peak) <= usable;
        let concurrency = if fits_in_ram {
            memory::load_concurrency(warp.warped, in_flight, frame_count, available, workers)
        } else {
            memory::load_concurrency(
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
