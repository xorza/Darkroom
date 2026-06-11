# Local contrast (CLAHE)

Contrast-Limited Adaptive Histogram Equalization for the **display-domain** (stretched `[0,1]`)
image — the astro use of PixInsight *LocalHistogramEqualization* / StarTools *Contrast*. Brings out
**medium-scale structure** (dust lanes, dark rifts, nebula filaments, spiral arms, Milky-Way
star-cloud texture) by equalizing contrast *locally* instead of globally.

A **post-stretch** tone operation (it works on the perceptual distribution, not linear flux).

## Algorithm (Zuiderveld, *Graphics Gems IV* 1994)

1. **Tile** the image into a `tiles × tiles` grid.
2. Per tile, build a **histogram** (`N_BINS = 256`) of the intensity.
3. **Clip** each bin at `clip = clip_limit · tilePixels / N_BINS` (the "contrast-limited" step — caps
   how much any one level is amplified, which is what stops noise/flat regions from blowing up).
   **Redistribute** the clipped excess equally across all bins.
4. The normalized **CDF** of the clipped histogram is that tile's mapping LUT (`bin → [0,1]`,
   monotonic).
5. Per pixel, **bilinearly interpolate** the four surrounding tile-centre LUTs (so tile seams don't
   show), look up the pixel's bin → output. Border/corner pixels clamp to the nearest tiles.

The result is **blended** with the original by `strength ∈ [0,1]` (`0` = identity).

**Why contrast-*limited*:** plain adaptive HE over-amplifies low-variance regions (background noise).
Clipping the histogram bounds the local slope to `clip_limit×`, so a flat tile maps ≈ linearly
(stays put) instead of being stretched to full range.

## In lumos

- **Color:** computed on the combined intensity `I=(r+g+b)/3`; channels are then scaled by `f(I)/I`
  (hue-preserving, same as `stretching`). Grayscale applies the mapping directly.
- **Params:** `tiles` (grid count per axis — *larger* tiles / fewer cells for wide-field; ~8 typical),
  `clip_limit ≥ 1` (amplification cap; 2–4 typical, 1 ≈ off), `strength` (blend).
- For a wide-angle Milky Way: a few large tiles + a modest `clip_limit` pops the dust-lane / rift
  structure without crushing the background.

## References
- Zuiderveld, *Contrast Limited Adaptive Histogram Equalization*, Graphics Gems IV, 1994.
- PixInsight *LocalHistogramEqualization* (CLAHE); StarTools *Contrast*.
- `en.wikipedia.org/wiki/Adaptive_histogram_equalization`.
