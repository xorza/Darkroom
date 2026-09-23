# Lumos

Astronomical image-processing library: RAW/FITS decoding, master-frame
calibration, star detection, star-pattern registration, frame stacking, drizzle
reconstruction, and non-linear display stretching. CPU-bound with hand-written
SIMD (AVX2 / SSE4.1 / NEON) hot paths and rayon parallelism; no GPU backend.
Pixels are stored **planar** (one f32 plane per channel) and normalized to
`[0, 1]`.

## Mission & scope

The **most precise and the fastest** astrophotography stacking pipeline,
growing from a good-looking image toward a **science data product**: the linear
stacked master _plus_ per-pixel quality planes (coverage, weight,
variance/noise) that let a downstream tool **measure** it — photometry, source
extraction, error bars — instead of merely viewing it.

- **The stacked master comes first.** Science-metadata extras are welcome only
  when they stay low-complexity and ride cheaply on data the pipeline already
  computes. Machinery that serves neither the image nor its measurability is
  out of scope — remove it rather than carry it.
- **Precision and correctness outrank speed.** The hot paths are aggressively
  optimized, but when the two conflict the numerically-correct choice wins;
  never trade accuracy of the stacked result for throughput.

## Pipeline

A stack of telescope exposures → one master: **load / decode → calibrate →
detect stars → register → combine**, then an optional **stretch** into the
display domain. Stretching is display-prep and runs strictly after all
linear-domain work.

## Verification

Tests leave out `real-data`:

```
cargo test -p lumos --tests --features ml,internals
```

`real-data` turns on the tests that read the gitignored ~7.4 GB dataset in
`test_data/lumos_data/` (`testing::calibration_dir` asserts it is there); run
them only when asked. The ML tests among them also need caller-supplied
weights — `STARNET2_ONNX` / `DEEPSNR_ONNX`, or the default files in
`test_data/` — and skip without them.
