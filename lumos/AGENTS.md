# Lumos

Astronomical stacking pipeline: **load / decode (RAW, FITS) → calibrate →
detect stars → register → combine (stack or drizzle)**, then an optional
non-linear **stretch**, strictly after all linear-domain work. CPU only:
vector kernels written once over the `simd` crate's `Isa` (AVX2+FMA, NEON, a
portable fallback) and rayon. Pixels are **planar** f32, one plane per
channel, normalized to `[0, 1]`.

## Scope

The **most precise and the fastest** stacking pipeline, growing toward a
**science data product**: the linear master plus per-pixel quality planes
(coverage, weight, variance) a downstream tool measures it with — photometry,
source extraction, error bars.

- **The stacked master comes first.** A science extra must stay simple and
  ride on data the pipeline already computes. Machinery that serves neither
  the image nor its measurement is removed, not carried.
- **Thin-plate-spline distortion and drizzle stay**, even without a
  production caller.
- **Precision outranks speed.** Never trade the stacked result's accuracy for
  throughput.

## Memory

- **The memory budget is soft: it steers caching and spilling, and never refuses
  work.** A run reads the memory it plans for once, at its entry — the
  caller's override, else what the system reports available — and plans with
  a share of it whether frames stay resident or spill to the run's scratch,
  and how wide each stage runs: it trades memory for disk I/O. A stage that
  needs more than the budget runs anyway; it logs its overcommit and
  continues.
- **No decode or stage is refused for its size.** Nothing compares what a file
  or a stage needs with the budget or with the system's memory. An allocation
  the system cannot back stops the process. Refused: a ceiling of a share of
  the available memory, which blocked files the machine could load, and one of
  the physical RAM.
