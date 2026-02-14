# X-Trans Markesteijn Demosaic

## Algorithm (1-pass, 4 directions)

1. **Green min/max** (`compute_green_minmax`): For non-green pixels, scan first 6 hex
   neighbors to find min/max green bounds. Green pixels get identity (gmin=gmax=value).
   Row-parallel via rayon.

2. **Green interpolation** (`interpolate_green`): 4 directional estimates (H, V, D1, D2)
   using weighted hexagonal neighbor formulas from dcraw/libraw. Weights are exact binary
   fractions matching reference: 0.6796875 (= 87/128), 0.87109375 (= 111.5/128),
   0.12890625, 0.359375, 0.640625. Results clamped to [gmin, gmax]. Green pixels get raw
   value in all 4 directions. Row-parallel via rayon with UnsafeSendPtr for disjoint writes.

3. **Derivatives** (`compute_derivatives`): YPbPr spatial Laplacian per direction. RGB is
   NOT materialized -- recomputed on-the-fly via `compute_rgb_pixel()` which calls
   `interpolate_missing_color_fast()` using `ColorInterpLookup` (precomputed 6x6x2 table
   of Pair/Single/None neighbor strategies). Uses sliding 3-row YPbPr cache to avoid
   redundant RGB recomputation. Direction-by-chunk parallelism via rayon.

4. **Homogeneity** (`compute_homogeneity`): Two sub-passes:
   (a) Find minimum derivative across 4 directions per pixel -> threshold = 8 * min_drv
   (b) In 3x3 window, count pixels where drv <= threshold -> homo u8 value (0-9).
   Border pixels set to 0. Parallelized across (direction, row) pairs.

5. **Blend** (`blend_final`): Sequential SAT construction (one direction at a time) for
   O(1) 5x5 window homogeneity queries. Selects directions scoring >= 7/8 of max,
   averages their RGB (recomputed on-the-fly). Row-parallel output via rayon.

## Memory Layout (Arena)

```
10P f32 total, where P = width * height
Region A [0..4P]:   green_dir (4 directions)     - Written Step 2, read Steps 3-5
Region B [4P..8P]:  drv (Steps 3-4) / output RGB (Step 5, first 3P of region)
Region C [8P..9P]:  gmin (Steps 1-2) -> homo as u8 (Steps 4-5, f32->u8 reinterpret cast)
Region D [9P..10P]: gmax (Steps 1-2) -> threshold (Step 4)
```

RGB is never materialized as a buffer -- recomputed on-the-fly from green_dir to avoid
the 12P rgb_dir buffer (~1.1 GB for 6032x4028). Peak arena: ~920 MB for full-res X-Trans.

## Correctness Assessment

**Faithful to reference**: The implementation closely follows dcraw/libraw's Markesteijn
1-pass. Key correctness indicators:

- Hex lookup construction: Replicates libraw's `allhex` table generation using ORTH and PATT
  coefficient arrays. The 3x3 repeating pattern correctly handles all 9 position types.
  The `+6` offset trick for negative modular arithmetic is correctly applied.
- Green interpolation weights: All coefficients are exact binary fractions matching the
  dcraw reference (verified by inspection against dcraw source).
- YPbPr conversion: Uses BT.2020 coefficients (0.2627, 0.6780, 0.0593). This differs from
  BT.709 (0.2126, 0.7152, 0.0722) but matches libraw's choice. The difference is negligible
  for direction-selection purposes (relative metric, not absolute color).
- Homogeneity threshold: 8x minimum derivative, matching reference.
- Blend threshold: 7/8 of max homogeneity score, matching reference.
- Per-channel normalization: `read_normalized()` correctly applies per-channel black
  subtraction and per-channel WB multipliers for both U16 and F32 pixel sources.

**Quality benchmark results** (vs libraw 1-pass, 6032x4028 X-Trans image):
- MAE ~0.0005, avg Pearson correlation ~0.91 (R=0.89, G=0.96, B=0.88)
- For reference, libraw's own 1-pass vs 3-pass differs by MAE ~0.0003
- Lower red/blue correlation (0.88-0.89 vs green's 0.96) expected: green is directly
  measured at ~55% of X-Trans pixels while red/blue rely entirely on interpolation
- 2.1x speedup over libraw single-threaded (multi-threaded via rayon)
- Performance: ~620ms demosaic for 6032x4028 (vs libraw ~1750ms single-threaded)

**Known issues from external reports** (RawTherapee/darktable community):
- High ISO (>3200): Markesteijn 1-pass can produce crosshatch noise patterns.
  Mitigated by stacking in astrophotography (noise averages out).
- Highlight details: Filter overshoot artifacts possible near clipped highlights.
  Not a significant concern for astrophotography (well-exposed subs avoid saturation).
- No median refinement (3-pass mode): Slightly less accurate at color transitions.
  Per darktable/RawTherapee users, the quality difference is only visible in low-ISO shots.

**Implementation-specific correctness notes**:
- `UnsafeSendPtr` is required for Edition 2024 closure captures when sharing raw
  pointers across rayon threads. Each thread writes to unique (y, x) indices.
- The `gmin -> homo` u8 reinterpret cast is safe because f32 alignment (4) satisfies
  u8 alignment (1), and gmin data is dead after Step 2.
- The output copy at the end (`arena.storage[4P..7P].to_vec()`) could be avoided by
  returning a sub-slice or by writing output directly to the caller's buffer.

## Performance Optimizations Applied

- `ColorInterpLookup`: Precomputed 6x6x2 neighbor strategies (Pair/Single/None) avoids
  per-pixel pattern lookups in `interpolate_missing_color_fast()`.
- Interior fast path: skips all bounds checks for ~99% of pixels (only border pixels
  within 1 pixel of edge use the checked slow path).
- Sliding 3-row YPbPr cache in derivatives: 3x fewer `compute_rgb_pixel` calls. Each
  row computed once, reused as prev/next neighbor. 64-row chunks for parallelism.
- SATs built one direction at a time (1P u32 peak instead of 4P). Homogeneity scores
  stored in `hm_buf: Vec<[u32; 4]>` between SAT construction and parallel blend.
- Rayon parallelism at row/chunk level in all 5 steps.
- U16 raw data kept until demosaic (47 MB vs 93 MB for pre-normalized f32).
- Libraw guard and file buffer dropped before demosaic to reduce peak memory by ~77 MB.
- Per-channel black + WB folded into `read_normalized()` per-pixel path (zero overhead).

## SIMD Opportunities (Not Yet Exploited)

The X-Trans demosaic is currently scalar. Potential SIMD targets:

1. **Green interpolation inner loop**: The 4-direction weighted sums use fixed coefficients.
   AVX2 could process multiple pixels per iteration, but the hex neighbor access pattern
   (irregular offsets) makes gather-based vectorization difficult. AVX2 gather was rejected
   elsewhere in the project (~2% slower due to gather latency > L1 scalar loads).

2. **Derivative computation**: The YPbPr Laplacian (2*center - forward - backward) is
   SIMD-friendly. The sliding window cache makes row-sequential processing natural for SSE.

3. **Homogeneity 3x3 window**: The comparison + counting loop could use SIMD masking, but
   the u8 output and 3x3 window size limit the benefit.

4. **Blend averaging**: The direction selection + RGB averaging could benefit from SIMD for
   the `compute_rgb_pixel()` calls, but branch-heavy logic limits vectorization.

Overall, the irregular X-Trans pattern makes SIMD less beneficial than for Bayer demosaicing
(which has regular 2x2 structure). The ~620ms timing for 24MP is already fast enough
for the astrophotography workflow.

## Known Limitation: No 3-pass Mode

Only 1-pass (4 directions). Reference also has 3-pass (8 directions with median refinement)
for slightly better quality at ~3x cost. Per darktable/RawTherapee users, the quality
difference is only visible in low-ISO shots. 1-pass is sufficient for astrophotography
workflows where images are stacked.
