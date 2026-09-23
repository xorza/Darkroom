# Lumos

Astronomical stacking pipeline: **load / decode (RAW, FITS) → calibrate →
detect stars → register → combine (stack or drizzle)**, then an optional
non-linear **stretch** into the display domain, strictly after all
linear-domain work. CPU only — hand-written SIMD (AVX2 / SSE4.1 / NEON) and
rayon. Pixels are **planar** (one f32 plane per channel), normalized to
`[0, 1]`.

## Scope

The **most precise and the fastest** stacking pipeline, growing toward a
**science data product**: the linear stacked master _plus_ per-pixel quality
planes (coverage, weight, variance/noise) that let a downstream tool
**measure** it — photometry, source extraction, error bars.

- **The stacked master comes first.** Science extras are welcome only when
  they stay low-complexity and ride on data the pipeline already computes.
  Machinery that serves neither the image nor its measurability is out of
  scope — remove it rather than carry it.
- **Precision and correctness outrank speed.** When they conflict, the
  numerically-correct choice wins; never trade accuracy of the stacked result
  for throughput.

## Verification

Tests leave out `real-data`:

```
cargo test -p lumos --tests --features ml,internals
```

`real-data` runs the tests that read the gitignored ~7.4 GB dataset in
`test_data/lumos_data/` — only when asked. Its ML tests also need ONNX weights
(`STARNET2_ONNX` / `DEEPSNR_ONNX`, or the default files in `test_data/`) and
skip without them.
