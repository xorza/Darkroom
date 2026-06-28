# HDR multiscale dynamic-range compression

Reveal detail hidden in an **overexposed bright region** — galaxy/nebula cores (M42, M31 core),
Milky-Way star clouds — by compressing the **large-scale brightness** while preserving fine detail.
The astro use of PixInsight *HDRMultiscaleTransform* (HDRWaveletTransform, Vicent Peris 2006).

A **display-domain** (post-stretch) tone operation.

## Algorithm

It separates structure by scale with the à trous **starlet** transform (the same shared
`wavelet::StarletTransform` that `denoise` uses) and compresses only the coarse component:

```
image = residual + Σ layers            (à trous: layers = detail at ~2^j px; residual = the coarsest smooth)
residual' = mean + (1 − amount)·(residual − mean)     (flatten the large-scale brightness toward its mean)
out = residual' + Σ layers             (clamp to [0,1])
```

The bright core's overpowering glow is a **large-scale** structure → it lives in the `residual`.
Pulling the residual toward its mean (`amount` ∈ [0,1]) brings the core's brightness down toward the
surroundings, while the **detail layers are left untouched**, so the fine structure inside the core
(and everywhere else) is preserved and becomes visible. This is the multiscale insight: contrast at a
given scale isn't perturbed by larger structures at coarser scales.

- **`scales`** = where the large/small-scale boundary sits: structures coarser than ~`2^scales` px go
  into the residual and get compressed; finer detail is preserved. *More* scales → only the very
  largest structures compress (gentler). On a full-resolution frame, ~6 isolates the broad core glow.
- **`amount`** = compression strength. `0` = no-op; `1` = the large-scale brightness is fully
  flattened (detail-only, usually too much). ~0.5 typical.

## In lumos

- Computed on the combined intensity `I=(r+g+b)/3`; channels rescaled by `f(I)/I` (hue-preserving,
  same as `stretching`/`local_contrast`). Grayscale applies directly.
- Reuses `wavelet::StarletTransform::{forward, reconstruct}` — the exact transform `denoise` streams.

## References
- PixInsight *HDRMultiscaleTransform* / *HDRWaveletTransform* (Vicent Peris): `pixinsight.com/examples/HDRWT/`.
- Starck, Murtagh — starlet / à trous (flux-conserving, ideal for multiscale astro).
