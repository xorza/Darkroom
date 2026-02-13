# registration Module - Implementation Notes

Complete astronomical image registration pipeline: triangle asterism matching
via k-d tree, RANSAC with MAGSAC++ scoring and LO-RANSAC, transform models
(Translation through Homography), optional SIP distortion correction, image
warping with Lanczos interpolation and SIMD-optimized row warping.

## Architecture

### Pipeline Flow
1. Star positions (sorted by brightness) -> triangle formation via KNN (k-d tree)
2. Triangle invariant ratios indexed in k-d tree for O(log n) lookup
3. Vote matrix accumulates vertex correspondences from similar triangles
4. RANSAC with MAGSAC++ scoring estimates transform from voted matches
5. Post-RANSAC match recovery via k-d tree projection on unmatched stars
6. Optional SIP polynomial fit on residuals, bundled into `WarpTransform`
7. Image warping via `warp()` / `warp_image()` takes `&WarpTransform` (output→input mapping, Lanczos3 default)

### Strengths
- Triangle formation via k-d tree O(n*k^2) vs O(n^3) brute-force
- K-d tree on invariant ratios (Groth 1986 / Valdes 1995 approach)
- MAGSAC++ scoring (Barath & Matas 2020) - eliminates manual threshold tuning
- FWHM-adaptive sigma derived from median star FWHM
- DLT with Hartley normalization for homography estimation
- Post-RANSAC match recovery (recovers 10-30% more matches)
- Progressive 3-phase sampling: high-confidence matches sampled first
- Dense/sparse vote matrix auto-switch at 250K entries
- Flat implicit k-d tree layout (cache-friendly, no pointers)
- AVX2/SSE4.1 bilinear warp, optimized Lanczos3 row warp with interior fast-path
- Lanczos LUT: 4096 samples/unit, 48KB for Lanczos3 (fits L1 cache)
- 5 presets (fast, precise, wide_field, mosaic, precise_wide_field)

## Comparison with Industry Standards

### Triangle Matching vs Alternatives
This implementation uses the Groth (1986) triangle voting approach, same
as Astroalign. Industry alternatives:
- **Astrometry.net**: Uses quad (4-star) geometric hashing. Two most distant
  stars define a local coordinate system; other two stars' positions in that
  frame form a hash code. More distinctive than triangles, fewer false matches.
  Verified via Bayesian decision process.
- **PixInsight StarAlignment**: Upgraded from triangles to polygonal descriptors
  (quadrilaterals through octagons). Two most distant stars define coordinate
  frame, remaining N-2 star positions form hash. More robust for distorted fields.
- **Siril**: Triangle similarity (same as this crate), based on Michael
  Richmond's `match` program. Uses RANSAC for outlier rejection.
- **Astroalign**: Nearest-neighbor triangulation with 10 triangles per star
  (4 nearest neighbors). Same invariant ratio approach as this crate.

Current implementation is comparable to Siril/Astroalign. Upgrading to quad
descriptors would improve robustness for large star fields.

### RANSAC/MAGSAC++ Quality
MAGSAC++ implementation is faithful to Barath & Matas 2020:
- Marginalizes over noise scales via lower incomplete gamma function (LUT)
- Continuous loss function instead of binary inlier/outlier
- LO-RANSAC refinement loop
- Adaptive iteration count based on inlier ratio
- Progressive NAPSAC-like sampling (3-phase: top 25%, top 50%, uniform)

The original MAGSAC++ paper proposes iteratively re-weighted least squares
(IRWLS) for model polishing. This implementation uses standard least-squares
on inliers instead. Weighted LS using MAGSAC++ weights would improve accuracy.

### Transform Estimation
- **Similarity**: Closed-form via centered covariance (standard approach). Correct.
- **Affine**: Normal equations with direct 3x3 inverse (Cramer-like). Correct.
- **Homography**: DLT with Hartley normalization via SVD. Standard, correct.
  Uses A^T*A decomposition (computing A^T*A then SVD on that) rather than
  SVD directly on A. Slightly less numerically stable but adequate.
- **Translation**: Average displacement. Correct.
- **Euclidean**: Constrained Procrustes with scale=1. Cross-covariance rotation
  via atan2, translation computed with unit scale. Correct.

### Interpolation Quality
- Lanczos3 as default matches PixInsight/Siril defaults
- LUT-based kernel evaluation (4096 samples/unit precision ~0.00024)
- Kernel normalization prevents DC bias
- Bicubic uses Catmull-Rom (a=-0.5), standard choice
- No anti-aliasing filter for downscaling (minor for astro, typically upscaling)
- SIMD: AVX2 bilinear processes 8 pixels/cycle on x86_64

## Important Issues

### Triangle Vertex Correspondence is Approximate
**File:** `triangle/voting.rs:143-149`

When voting, `indices[i]` from ref matches `indices[i]` from target. But
`Triangle.indices` retains the sorted-by-index order (from `tri.sort()` in
matching.rs:126), not geometric-role order (e.g., vertex opposite shortest
side). Two similar triangles from different star sets may have different
geometric roles at the same index position.

This is mitigated by the voting mechanism itself (Groth 1986 design): correct
correspondences accumulate many votes across multiple triangles while incorrect
ones scatter. Works well in practice but could be improved by reordering
indices based on the side sorting in `from_positions()`.

### No Sigma-Clipping in SIP Fitting
Marginal RANSAC inliers can disproportionately affect polynomial coefficients.
LSST pipeline uses 3 iterations of 3-sigma clipping during SIP fitting.

### TPS Not Integrated Into Pipeline
**File:** `distortion/tps/mod.rs` - marked `#![allow(dead_code)]`

Implementation exists and passes tests but is not accessible from the public
API. PixInsight's StarAlignment uses 2D surface splines (thin plate splines)
as its primary distortion correction method when distortion correction is
enabled. They iteratively refine with "surface simplifiers" for accuracy.
Integrating TPS would close the gap with PixInsight for wide-field work.

## Minor Issues

- No weighted least-squares in final refinement. MAGSAC++ weights are available
  but unused for the final LS step.
- Bicubic kernel not normalized (Catmull-Rom sums to 1.0 only at integer
  positions; at fractional positions the sum deviates slightly).
- No anti-aliasing filter for scale-down warps (rare in astrophotography).

## Missing Features vs Industry Tools

| Feature | This Crate | PixInsight | Siril | Astrometry.net |
|---------|-----------|------------|-------|----------------|
| Star matching | Triangles | Polygons (4-8) | Triangles | Quads (4-star) |
| Distortion correction | SIP (forward, applied in warp) | TPS/surface splines | SIP via WCS | SIP (full A/B/AP/BP) |
| Drizzle integration | No | Yes | Yes | N/A |
| Phase correlation fallback | No | No | Yes | N/A |
| Multi-channel registration | Per-channel warp | Per-channel warp | Global | N/A |
| Plate solving | No | Yes | Yes | Yes |

## File Map

| File | Purpose |
|------|---------|
| `mod.rs` | Public API: `register()`, `warp()`, match recovery |
| `config.rs` | `Config` struct, presets, `InterpolationMethod` |
| `transform.rs` | `Transform` (3x3 homogeneous), `TransformType` enum, `WarpTransform` (bundles Transform + optional SIP) |
| `result.rs` | `RegistrationResult`, `RegistrationError` |
| `triangle/mod.rs` | Triangle matching params and re-exports |
| `triangle/geometry.rs` | `Triangle` struct, invariant ratios, orientation |
| `triangle/voting.rs` | Vote matrix (dense/sparse), correspondence voting |
| `triangle/matching.rs` | Triangle formation via KNN, `match_triangles()` |
| `ransac/mod.rs` | RANSAC loop, progressive sampling, LO-RANSAC |
| `ransac/transforms.rs` | Transform estimation (all types), DLT, normalization |
| `ransac/magsac.rs` | MAGSAC++ scorer, gamma LUT |
| `distortion/mod.rs` | Re-exports for SIP and TPS |
| `distortion/sip/mod.rs` | SIP polynomial fitting via Cholesky/LU |
| `distortion/tps/mod.rs` | Thin-plate spline (WIP, not integrated) |
| `interpolation/mod.rs` | Interpolation kernels, `warp_image(&WarpTransform)` |
| `interpolation/warp/mod.rs` | Optimized row warping (AVX2, SSE, scalar), takes `&WarpTransform` |
| `spatial/mod.rs` | K-d tree (flat implicit layout) |
