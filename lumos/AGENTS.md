# Lumos

Astronomical image-processing library: RAW/FITS decoding, master-frame
calibration, star detection, star-pattern registration, frame stacking, drizzle
reconstruction, and non-linear display stretching. CPU-bound with hand-written
SIMD (AVX2 / SSE4.1 / NEON) hot paths and rayon parallelism; no GPU backend.
Pixels are stored **planar** (one f32 plane per channel) and normalized to
`[0, 1]`.

## Mission & scope

Lumos aims to be the **most precise and the fastest** astrophotography stacking
pipeline there is, and is growing from "produce a good-looking image" toward a
**science data product**: the calibrated, registered, **stacked** deep-sky image
_plus_ the ancillary per-pixel quality planes (coverage, weight,
variance/noise) that let a downstream tool **measure** the result — photometry,
source extraction, error bars — instead of merely viewing it.

The core deliverable is still that stacked master — load → calibrate → detect →
register → combine — and it always comes first. **Science-metadata extras are
welcome alongside it, but only when they stay low-complexity and don't derail
the core**: they should ride cheaply on data the pipeline already computes.
Anything that adds significant machinery without serving either the image or
its measurability is out of scope and should be removed rather than carried.

**Precision and correctness outrank speed.** Both are first-class goals — the
hot paths are aggressively optimized — but when the two conflict, the
numerically-correct choice wins; never trade accuracy of the stacked result for
throughput.

## Pipeline

A stack of telescope exposures → one calibrated, aligned, combined deep-sky
image: **load / decode → calibrate → detect stars → register → combine**, with
an optional final **stretch** into the display domain. The science deliverable
is the linear stacked master; stretching is display-prep that runs strictly
after all linear-domain work.
