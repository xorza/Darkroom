# Hot Pixel Detection: Current Issue and Fix Options

## The Problem

Hot pixel detection currently runs on **already-demosaiced RGB data**. The pipeline is:

```
RAW file → load_raw() → demosaic (Bayer/X-Trans → RGB) → AstroImage (3ch RGB)
                                                              ↓
                                              stack dark frames (RGB)
                                                              ↓
                                          master_dark (3ch RGB, demosaiced)
                                                              ↓
                                     HotPixelMap::from_master_dark() ← WRONG: analyzing interpolated data
                                                              ↓
                                        correct() on demosaiced light frame
```

`AstroImage::from_file()` calls `load_raw()` which **always demosaics** before returning. There is no way to get raw CFA data through the current API. Both `CalibrationMasters::create()` and `CalibrationMasters::load()` work entirely with demosaiced images.

## Why This Is Wrong

1. **A single defective sensor pixel becomes multiple artifacts after demosaicing.** Bilinear interpolation spreads the hot pixel's signal to neighboring RGB pixels. Instead of 1 defect to fix, you get a cluster of elevated values across channels.

2. **Detection statistics are corrupted.** The MAD-based detection computes per-channel statistics on interpolated data where hot pixel energy has been smeared. This makes thresholds less accurate - some real defects get diluted below threshold, while interpolation artifacts create false positives.

3. **Correction is imprecise.** Replacing a demosaiced pixel with median-of-neighbors replaces interpolated values with other interpolated values. The original defective raw sample still contributes to the neighbor values used for replacement.

4. **The sigma floor hack (`sigma.max(median * 0.1)`) exists partly because of this.** When stacking demosaiced darks, the noise characteristics differ from raw data in ways that cause the MAD to be unreliable.

## What Correct Software Does

PixInsight, Siril, and other astrophotography tools detect and correct hot pixels on **raw CFA data before demosaicing**:

1. Build master dark from raw (un-demosaiced) dark frames
2. Detect hot pixels on the raw master dark (1 channel, CFA pattern)
3. For each light frame: load raw → correct hot pixels on raw CFA → then demosaic

This way each defective sensor element is identified and replaced with same-color CFA neighbors (e.g., replace a hot R pixel with the median of nearby R pixels in the Bayer grid, not with interpolated RGB neighbors).

## Fix Options

### Option A: Add Raw (Un-demosaiced) Loading Path

Add a `load_raw_cfa()` function that returns the raw CFA data as a single-channel `AstroImage` without demosaicing. Then:

1. Load dark frames raw (1 channel) → stack → raw master dark (1 channel)
2. Detect hot pixels on raw master dark (CFA-aware, same-color neighbors only)
3. For each light frame: load raw → apply hot pixel correction on raw CFA → demosaic → calibrate (dark subtract, flat divide)

**Changes needed:**
- `raw/mod.rs`: New `load_raw_cfa()` that returns 1-channel f32 data + CFA pattern metadata
- `AstroImageMetadata`: Add `cfa_pattern: Option<CfaPattern>` field
- `hot_pixels.rs`: CFA-aware `median_of_neighbors_raw()` that only uses same-color neighbors (stride 2 for Bayer, stride based on 6x6 pattern for X-Trans)
- `calibration_masters/mod.rs`: Use `load_raw_cfa()` for dark frame stacking, store raw master dark
- `stacking/cache.rs`: Support stacking 1-channel CFA frames
- Pipeline caller: Load light raw → hot pixel correct → demosaic → calibrate

**Pros:** Correct approach. Matches what PixInsight/Siril do. Best detection accuracy.
**Cons:** Significant refactor. Must handle CFA-aware neighbor selection. Master darks become incompatible (1ch raw vs 3ch RGB). Need separate load path for calibration frames vs light frames. Dark subtraction on raw data requires matching CFA alignment.

### Option B: Detect on Raw, Correct After Demosaic (Hybrid)

Only change detection to work on raw CFA data. Keep correction on demosaiced images.

1. Load dark frames raw → stack as 1-channel → raw master dark
2. Detect hot pixels on raw master dark → get pixel positions (x,y)
3. For each light frame: load and demosaic normally → correct the flagged positions on RGB data

**Changes needed:**
- Same raw loading changes as Option A (for darks only)
- `hot_pixels.rs`: Detection on 1-channel raw data, but correction stays RGB
- `HotPixelMap`: Store positions only (not per-channel), apply to all channels at flagged positions
- `calibration_masters/mod.rs`: Load darks via raw path, lights via normal path

**Pros:** Detection is accurate (on raw data). Correction is simpler (no CFA-aware neighbor logic). Less disruption to existing pipeline.
**Cons:** Correction still operates on interpolated data - the defective raw sample has already contributed to neighboring RGB pixels through interpolation. The corrected pixel value is better but its neighbors remain slightly affected.

### Option C: Cosmetic Correction in Demosaic Step

Integrate hot pixel correction directly into the demosaicing functions. Before interpolation, replace hot raw pixels with same-color CFA neighbor medians, then demosaic normally.

1. Build raw master dark → detect on raw
2. Pass hot pixel map into `demosaic_bayer()` / `process_xtrans()`
3. Inside demosaic: before interpolating, check if each raw pixel is flagged → if so, replace with CFA-neighbor median

**Changes needed:**
- Same raw loading + detection from Option A
- `demosaic/mod.rs`: Accept `Option<&HotPixelMap>`, replace flagged pixels before interpolation
- Keep the demosaic API backward-compatible (None = no correction)

**Pros:** Cleanest result - hot pixels are gone before any interpolation happens. No artifacts propagated.
**Cons:** Couples hot pixel correction to demosaicing code. Every demosaic variant (Bayer bilinear, X-Trans Markesteijn, libraw fallback) needs modification.

### Option D: Do Nothing (Accept Current Behavior)

The current approach isn't ideal but works reasonably well in practice:
- 5-sigma MAD detection catches the worst offenders even on demosaiced data
- Median-of-neighbors correction on RGB data does remove visible artifacts
- The sigma floor prevents over-detection

**Pros:** No work required. Current results are acceptable for many use cases.
**Cons:** Not correct. Subtle artifacts remain near hot pixels. Star photometry near hot pixel sites is affected.

## Recommendation

**Option B (hybrid)** gives the best effort-to-quality ratio. The detection accuracy is the most important part - correctly identifying which sensor pixels are defective. Correcting on demosaiced data is imperfect but the remaining error (slight contamination of neighbors) is small and usually invisible.

If higher quality is needed later, Option C can be built on top of Option B's raw detection infrastructure.

## Key Files

| File | Role |
|------|------|
| `lumos/src/raw/mod.rs` | Raw loading + demosaicing (always demosaics) |
| `lumos/src/raw/demosaic/mod.rs` | Bayer demosaic implementation |
| `lumos/src/raw/demosaic/xtrans.rs` | X-Trans demosaic implementation |
| `lumos/src/stacking/hot_pixels.rs` | Hot pixel detection (MAD) + correction (median) |
| `lumos/src/stacking/cache.rs` | Image cache for stacking (loads via `from_file`) |
| `lumos/src/calibration_masters/mod.rs` | Master creation + calibration pipeline |
| `lumos/src/astro_image/mod.rs` | AstroImage struct, `from_file()` routing |
| `lumos/src/astro_image/sensor.rs` | Sensor type detection (Bayer/X-Trans/Mono) |
