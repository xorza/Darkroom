# Deblend Module - Implementation Notes

## Overview

Two deblending algorithms for separating overlapping astronomical sources within
connected components: a fast local-maxima approach and a SExtractor-style
multi-threshold tree. Both output `Region` structs (bbox, peak, peak_value, area).
Configured via `Config` fields: `deblend_n_thresholds` (0 = local maxima,
32+ = multi-threshold), `deblend_min_separation`, `deblend_min_prominence`,
`deblend_min_contrast`.

## File Map

| File | Lines | Purpose |
|------|-------|---------|
| `mod.rs` | 103 | Public types (`ComponentData`, `Pixel`), `MAX_PEAKS=8`, `iter_pixels`, `find_peak` |
| `region.rs` | 20 | `Region` struct (bbox, peak, peak_value, area) |
| `local_maxima/mod.rs` | 257 | Local maxima detection + Voronoi partitioning |
| `local_maxima/tests.rs` | 575 | 22 unit tests |
| `local_maxima/bench.rs` | 141 | 4 benchmarks on 4K/6K globular cluster images |
| `multi_threshold/mod.rs` | 1084 | SExtractor-style tree deblending with grid-based BFS |
| `multi_threshold/tests.rs` | 1404 | 34 unit tests including grid, node-grid, tree analysis |
| `multi_threshold/bench.rs` | 154 | 3 benchmarks on 4K/6K globular cluster images |
| `tests.rs` | 110 | 2 integration tests comparing both algorithms |

## Algorithm 1: Local Maxima (local_maxima/mod.rs)

### Flow
1. `find_local_maxima` (L95-135): scan component pixels, filter by prominence
   and separation, collect into `ArrayVec<Pixel, 8>`
2. If 0-1 peaks: return single `Region` from full component (L68-83)
3. If 2+ peaks: `deblend_by_nearest_peak` (L141-183) does Voronoi partitioning

### Peak Detection (L115-125)
- `is_local_maximum` (L192-208): strict 8-neighbor comparison (strictly greater
  than all neighbors). Boundary pixels treat out-of-bounds as passing.
- Prominence filter: peak must exceed `global_max * min_prominence` (L112, L116-118)
- Separation filter: `add_or_replace_peak` (L213-236) uses squared Euclidean
  distance; replaces nearby dimmer peaks

### Pixel Assignment (L162-168)
- Pure Voronoi: each pixel assigned to nearest peak by Euclidean distance
- `find_nearest_peak` (L240-256): linear scan over peaks (max 8), squared distance
- Accumulates per-peak bounding box and area, conserves total area exactly

### Characteristics
- O(N * P) where N = component pixels, P = num peaks (max 8)
- Zero heap allocation: all storage in `ArrayVec<_, MAX_PEAKS>`
- Neighborhood size: fixed 3x3 (8-connected), not configurable
- Default parameters: `min_separation=3`, `min_prominence=0.3`

## Algorithm 2: Multi-Threshold Tree (multi_threshold/mod.rs)

### Flow
1. Early exits (L381-408): empty component, `min_contrast >= 1.0`, component
   too small for two stars (`area < 2 * min_sep^2`), peak barely above threshold
2. `build_deblend_tree` (L460-557): exponential threshold sweep from detection
   level to peak, tracking connectivity splits
3. `find_significant_branches` (L730-781): recursive contrast criterion on tree
4. `assign_pixels_to_objects` (L833-874): Voronoi assignment to leaf peaks

### Threshold Spacing (L494-496)
```
ratio = (peak_value / detection_threshold).max(1.0)
threshold[i] = detection_threshold * ratio^(i / n_thresholds)
```
This is exponential (logarithmic) spacing: more levels near the faint end where
structure first emerges, fewer near the peak. Matches SExtractor's approach.

### Tree Construction (L494-557)
- For each threshold level (0..=n_thresholds):
  - Filter pixels above threshold into `above_threshold` buffer (L499-505)
  - Find connected regions via grid-based BFS (L515-521)
  - Level 0: create root nodes (L525-526, `process_root_level` L560-580)
  - Level 1+: check if regions split from parent (`process_higher_level` L584-648)
    - `find_single_parent_grid` (L652-666): all pixels in a region must share one parent
    - If parent's above-threshold pixels form multiple regions, create children
    - `create_child_nodes` (L669-715): check min separation between siblings,
      add to tree with flux sum

### Contrast Criterion (L787-826)
```
min_flux = min_contrast * parent_flux
```
A branch is kept as separate object if at least 2 children have flux >= min_flux.
If only 0-1 children pass, the parent becomes a leaf (no split). This is per-node
recursive, not global.

**NOTE**: SExtractor's `DEBLEND_MINCONT` is documented ambiguously, but examination of
both the SExtractor source code (`analyse.c`) and SEP source confirms the contrast is
computed relative to the **total (root) component flux**, not the parent branch flux.
SEP formula: `child_flux - child_thresh * child_npix > DEBLEND_MINCONT * root_flux`.
This implementation uses parent-relative contrast, which is a **deliberate deviation**
that makes the criterion stricter for nested splits (a child must be significant relative
to its immediate parent, not just relative to the whole component). In practice, the
difference mainly affects deeply nested tree structures with 3+ levels of splitting.

photutils takes a different approach: it applies watershed segmentation, then iteratively
removes the faintest peak that fails the contrast criterion, re-applies watershed, and
repeats. This "remove one at a time" strategy handles the case where several faint
sources could combine to meet the contrast criterion individually.

### Grid-Based BFS (L900-1049)
- `PixelGrid` (L50-186): flat arrays indexed by local bbox coordinates with
  1-pixel border guaranteed by `wrapping_sub` on min coordinates
- Generation counters for both values and visited state: O(1) reset instead
  of O(N) clearing between threshold levels
- `bfs_region` (L955-999): BFS with recycled `Vec<Pixel>` from region pool
- `visit_neighbors_grid` (L1013-1031): unchecked 8-connected neighbor access
  using pre-computed flat index offsets. Border cells have NO_PIXEL via generation
  check, preventing out-of-bounds propagation.
- `try_visit_idx` (L1039-1049): combined value check + visited mark in one call

### Node Assignment Grid (L192-299)
- `NodeGrid`: tracks which tree node owns each pixel position
- Generation-counter reset, same pattern as PixelGrid
- Used to detect parent node for each connected region at higher thresholds

### Buffer Reuse (L314-355)
- `DeblendBuffers`: 7 reusable buffers (component_pixels, pixel_to_node,
  above_threshold, parent_pixels_above, bfs_queue, regions, region_pool + pixel_grid)
- Created once per rayon thread via `fold` in detect.rs:184-210
- Region pool: recycled Vec<Pixel> to avoid per-BFS allocation

### Early Termination (L444-446, L542-550)
- `EARLY_TERMINATION_LEVELS = 4`: stops if no new tree nodes created for 4
  consecutive threshold levels. Saves 30-50% iterations in typical cases.

### Tree Size Management (L448-453, L724, L739-746)
- `DeblendTree = SmallVec<[DeblendNode; 16]>`: stack-allocated for typical trees
- `MAX_TREE_SIZE = 128`: stack-allocated `is_child` array for `find_significant_branches`
- Heap fallback for trees exceeding 128 nodes (tested in test L1001-1043)

## Comparison with Industry Standards

### vs SExtractor (Bertin & Arnouts 1996)

| Aspect | SExtractor | This Implementation |
|--------|-----------|-------------------|
| Threshold levels | DEBLEND_NTHRESH (default 32) | `deblend_n_thresholds` (default 32 for crowded_field) |
| Threshold spacing | Exponential (log scale) | Exponential: `low * (high/low)^(i/n)` (L494-496) |
| Contrast criterion | DEBLEND_MINCONT (default 0.005) | `deblend_min_contrast` (default 0.005) |
| Contrast reference | Relative to **root/total** component flux | Relative to **parent branch** flux (L802-804) -- **differs** |
| Connected components | BFS/flood-fill | Grid-based BFS with generation counters (faster) |
| Pixel assignment | Gaussian template weighting | Voronoi (nearest peak) -- see weakness below |
| Connectivity | 8-connected | 8-connected in BFS, configurable 4/8 for initial CCL |

**Key difference -- pixel assignment**: SExtractor assigns ambiguous pixels (below
the splitting isophote) proportionally based on Gaussian model amplitudes at each
pixel position. This gives flux-weighted splitting that tracks the actual light
distribution. The current implementation uses pure Voronoi (nearest-peak), which
creates straight geometric boundaries that do not follow isophotal contours.
For symmetric, equal-brightness pairs this is fine; for asymmetric blends
(bright + faint neighbor), Voronoi over-assigns area to the faint source.

**Matching aspects**: Exponential threshold spacing, 32-level default, 0.005
contrast default, and the hierarchical tree structure closely follow SExtractor's
design. The contrast reference differs (parent-relative vs root-relative).

### vs photutils deblend_sources

| Aspect | photutils | This Implementation |
|--------|----------|-------------------|
| Levels | nlevels=32 | n_thresholds=32 |
| Contrast | contrast=0.001 | min_contrast=0.005 |
| Spacing modes | exponential, linear, sinh | exponential only |
| Post-processing | Watershed segmentation | Voronoi partitioning |
| Peak handling | Iterative removal of faintest failing peaks | Tree pruning (recursive contrast check) |

photutils combines multi-thresholding with watershed segmentation for pixel
assignment, which follows gradient contours and produces more natural boundaries
than Voronoi. photutils also iteratively removes the faintest peaks that fail
the contrast criterion until all remaining sources pass, while this implementation
does a single recursive tree traversal.

### vs SDSS Deblender (Lupton)

The SDSS deblender takes a fundamentally different approach:
1. Identifies peaks in blended parent objects
2. Builds symmetric templates by taking min(pixel, symmetric-pixel) across each peak
3. Assigns flux proportionally to template amplitudes at each pixel
4. Enforces monotonicity constraint on templates

This produces much more accurate flux distributions for overlapping galaxies but
is overkill for point-source (star) deblending. The symmetry + template approach
is similar to what scarlet later generalized.

### vs scarlet / scarlet2 (LSST/Rubin)

scarlet uses constrained matrix factorization: models each source as a product
of a morphological component (spatial profile) and a spectral energy distribution
(SED). Uses proximal gradient descent with symmetry and monotonicity constraints.
scarlet2 replaces non-differentiable constraints with differentiable alternatives.

This is a fundamentally different paradigm -- model-based optimization vs.
threshold-based segmentation. Appropriate for multi-band galaxy deblending;
not applicable to single-band point-source star detection.

### vs Modern ML Approaches

Mask R-CNN, GANs, and transformer-based networks achieve 92%+ precision for
deblending in survey data. These are appropriate for galaxy deblending in
large surveys (LSST, Euclid) but far too heavy for real-time astrophotography
stacking where the current algorithms operate.

## Strengths

1. **Two-tier design**: Local maxima for sparse fields (fast, zero-alloc), multi-threshold
   for crowded fields (more accurate). Pipeline selects via `deblend_n_thresholds` config.
2. **SExtractor-faithful**: Exponential spacing, parent-relative contrast, 32 levels,
   0.005 default contrast closely match the reference implementation.
3. **Memory efficiency**: ArrayVec/SmallVec throughout, generation-counter grids avoid
   clearing, region pool recycles Vec allocations, DeblendBuffers reused across components.
4. **Parallelism**: Rayon fold with per-thread DeblendBuffers (detect.rs:184-210).
5. **Early termination**: 4-level no-split check avoids wasted iterations (L444-550).
6. **Area conservation**: Both algorithms guarantee total pixel count is preserved
   after deblending (verified by tests).
7. **Robust edge handling**: PixelGrid wrapping_sub border handles coordinate-0 pixels
   correctly (regression test L1204-1245).

## Bugs

### ~~P2: PixelGrid Generation Counter Wrap-to-Zero Not Guarded~~ — FIXED
- Added same `if self.current_generation == 0 { self.current_generation = 1; }` guard as NodeGrid.
- Test: `test_pixel_grid_generation_wrap_to_zero_guard`

## Weaknesses and Potential Improvements

### W1: Voronoi Pixel Assignment Instead of Flux-Weighted

Both algorithms assign pixels to the nearest peak by Euclidean distance. SExtractor
and photutils use more sophisticated methods:
- SExtractor: Gaussian model weighting
- photutils: Watershed segmentation (follows intensity gradients)

Voronoi creates straight-line boundaries that cut through the actual light distribution.
For a bright star next to a faint companion, the boundary is equidistant rather than
shifted toward the faint source as the isophotes would dictate.

**Impact**: Moderate. For point sources with similar brightness, Voronoi is adequate.
For high dynamic range blends (>2 mag difference), flux measurements will be biased.

**Fix**: Implement intensity-weighted assignment: assign pixel to peak with highest
`peak_flux * exp(-dist^2 / (2 * sigma^2))` rather than just `min(dist)`. This
approximates Gaussian template weighting without building full templates.

### W2: No Linear/Sinh Threshold Spacing Options

photutils offers three spacing modes (exponential, linear, sinh). The sinh mode
provides better resolution at both ends simultaneously. This implementation only
supports exponential.

**Impact**: Low. Exponential spacing matches SExtractor and works well for the
target use case (astronomical point sources with Gaussian/Moffat profiles).

### W3: Local Maxima 3x3 Neighborhood Is Not Configurable

The `is_local_maximum` function (local_maxima/mod.rs:192-208) uses a fixed 3x3
neighborhood. For large stars (FWHM > 10px), a larger neighborhood (5x5 or 7x7)
would reduce false peak detection from noise spikes on the PSF profile.

**Impact**: Mitigated by the prominence filter (`min_prominence=0.3` eliminates
noise peaks that are <30% of the global max). A potential concern for very noisy
images with large FWHM where noise peaks can approach 30% of the star peak.

### W4: MAX_PEAKS=8 Hard Limit

Both algorithms cap at 8 peaks per component (mod.rs:34). Components with more
peaks silently ignore excess (local_maxima: ArrayVec full, multi_threshold:
MAX_CHILDREN limit). SExtractor has no such limit.

**Impact**: Low for typical astrophotography. Could miss sources in extremely
crowded globular cluster cores. The limit exists because components with >8
overlapping sources likely need model-based deblending (e.g., DAOPHOT PSF fitting)
rather than threshold-based segmentation.

### W5: Separation Check in multi_threshold Uses Chebyshev, Not Euclidean

`create_child_nodes` (multi_threshold/mod.rs:686-691) checks `dx < min_sep && dy < min_sep`
(Chebyshev/infinity-norm), while `local_maxima` uses `dx*dx + dy*dy >= min_sep_sq`
(Euclidean). The multi-threshold check is less strict on diagonals: two peaks at
(0,0) and (2,2) with min_sep=3 pass the multi-threshold check (dx=2 < 3, dy=2 < 3,
so too_close=true) but the check treats diagonal vs axis-aligned separation
differently than the Euclidean version.

**Impact**: Minor inconsistency. Both err on the side of merging close peaks.

### W6: No Iterative Peak Removal (photutils Pattern)

photutils applies watershed, then iteratively removes the faintest peak that fails
the contrast criterion, re-applies watershed, and repeats until all peaks pass.
This implementation does a single tree traversal with recursive contrast checking.

**Impact**: Low. The recursive tree approach achieves similar results for the
tree-shaped splitting patterns that multi-threshold deblending produces. Iterative
removal matters more for watershed where boundaries shift when a peak is removed.

## Performance Characteristics

Multi-threshold is O(N * T) per component where N = pixels, T = threshold levels,
with additional O(N) per BFS at each level. The early termination typically reduces
effective T by 30-50%. Benchmarks filter components with area > 100K pixels
(bench.rs:69, 98-101) because cost is quadratic for very large components due
to repeated pixel scanning.

Local maxima is O(N * P) where P <= 8, with no repeated scanning -- single pass.
Suitable for all component sizes.

## References

- Bertin & Arnouts 1996 (SExtractor): A&AS 117, 393
- Lupton et al. (SDSS deblender): astro.princeton.edu/~rhl/photomisc/deblender.pdf
- photutils deblend_sources: photutils.readthedocs.io
- Melchior et al. 2018 (scarlet): Astronomy and Computing 24, 129
- DRUID (persistent homology): arxiv.org/abs/2410.22508
