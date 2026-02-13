# registration Module - Implementation Notes

Complete astronomical image registration pipeline: triangle asterism matching
via k-d tree, RANSAC with MAGSAC++ scoring and LO-RANSAC, transform models
(Translation through Homography), optional SIP distortion correction, image
warping with Lanczos interpolation and SIMD-optimized row warping.

See submodule NOTES-AI.md files for detailed per-module analysis:
- `triangle/NOTES-AI.md` — matching algorithm, vertex correspondence, invariants
- `ransac/NOTES-AI.md` — MAGSAC++ fidelity, LO-RANSAC, transform estimation
- `distortion/NOTES-AI.md` — SIP correctness, TPS status, fitting analysis
- `interpolation/NOTES-AI.md` — kernels, SIMD, LUT, performance
- `spatial/NOTES-AI.md` — k-d tree correctness, performance

## Architecture

### Pipeline Flow
1. Star positions (sorted by brightness) -> triangle formation via KNN (k-d tree)
2. Triangle invariant ratios indexed in k-d tree for O(log n) lookup
3. Vote matrix accumulates vertex correspondences from similar triangles
4. RANSAC with MAGSAC++ scoring estimates transform from voted matches
5. Post-RANSAC match recovery via k-d tree projection on unmatched stars
6. Optional SIP polynomial fit on residuals, bundled into `WarpTransform`
7. Image warping via `warp()` / `warp_image()` takes `&WarpTransform` (output->input mapping, Lanczos3 default)

## Cross-Cutting Research Summary

### What We Do Correctly
- **All 5 transform estimators** verified correct (translation, euclidean, similarity, affine, homography)
- **MAGSAC++ scoring** is a structurally correct k=2 specialization of the paper
- **SIP polynomial direction** is correct (forward A/B, applied before linear transform)
- **SIP coordinate normalization** improves on the standard (reduces condition number)
- **K-d tree** implementation is clean, correct, well-tested (no bugs found)
- **Lanczos LUT** precision (4096 samples/unit) is adequate for f32 data
- **Kernel normalization** applied correctly in both generic and optimized paths
- **Incremental stepping** optimization is mathematically correct for affine transforms
- **Adaptive iteration count** formula matches standard (Fischler & Bolles 1981)
- **Hartley normalization** for DLT homography is standard and correct
- **Triangle vertex ordering** by geometric role (opposite shortest/middle/longest side)
- **SIP reference_point** configurable via `Config.sip_reference_point` (defaults to centroid, can pass image center)
- **Lanczos deringing** via min/max clamping of source pixels in kernel window (`InterpolationMethod::Lanczos3 { deringing: true }`)
- **`WarpParams` struct** bundles interpolation method and border_value through warp pipeline
- **MAGSAC++ preemptive scoring** exits early when cumulative loss exceeds best score

### Important Missing Features

7. ~~**Single-pass match recovery**~~ **DONE** — `recover_matches` now iterates
   up to 5 passes (ICP-style): project → match → re-validate → refit → repeat
   until convergence. Each pass also re-validates existing matches, removing
   outliers that drifted beyond threshold. Safety fallback ensures we never
   return fewer matches than we started with.

8. **TPS not integrated** — fully implemented and tested (`distortion/tps/`)
   but marked `dead_code`. PixInsight's primary distortion method.

9. **Separable Lanczos3** — explicit AVX2 was tried on the non-separable 6x6 path
   but provided no benefit (LLVM auto-vectorizes well). Fused weight normalization
   gives -47% per-pixel overhead, -5-8% full-image. The remaining big win is
   separable two-pass (6+6 vs 6x6), expected 2-3x improvement.

10. **Missing IRWLS for MAGSAC++** — paper's key contribution for model accuracy.
    We use MAGSAC++ only for scoring, with binary inlier selection for estimation.

### Comparison with Industry Tools

| Aspect | This Crate | PixInsight | Siril | Astrometry.net | Astroalign |
|--------|-----------|------------|-------|----------------|------------|
| Star matching | Triangles (2D) | Polygons (6D) | Triangles | Quads (4D) | Triangles (2D) |
| Vertex ordering | Correct (geometric role) | Correct | Correct | N/A (hash) | Correct (geometric) |
| RANSAC scoring | MAGSAC++ | Standard | OpenCV | Bayesian odds | RANSAC |
| IRWLS polish | No | N/A | N/A | N/A | No |
| Distortion | SIP (forward) | TPS (iterative) | SIP (from WCS) | SIP (full A/B/AP/BP) | None |
| Sigma-clipping SIP | Yes (3-iter, 3-sigma MAD) | N/A | N/A | Yes | N/A |
| Lanczos deringing | Yes (min/max clamping) | Yes (0.3 threshold) | Yes | N/A | N/A |
| SIMD warp | AVX2 bilinear; Lanczos3 auto-vectorized | Full SIMD | OpenCV SIMD | N/A | N/A |
| Match recovery | Iterative (ICP-style) | Iterative | N/A | Bayesian | N/A |
| Output framing | Fixed (=input) | Max/min/COG | Max/min/current/COG | N/A | Fixed |

### Prioritized Improvements

**Larger Effort (performance/features):**
1. Separable SIMD Lanczos3 (two-pass horizontal+vertical with AVX2)
2. Integrate TPS as alternative distortion model
3. IRWLS final polish with MAGSAC++ weights
4. Upgrade to quad descriptors (4D hash, matches Astrometry.net)

## File Map

| File | Purpose |
|------|---------|
| `mod.rs` | Public API: `register()`, `warp()`, match recovery |
| `config.rs` | `Config` struct, presets, `InterpolationMethod` |
| `transform.rs` | `Transform` (3x3 homogeneous), `TransformType`, `WarpTransform` |
| `result.rs` | `RegistrationResult`, `RegistrationError` |
| `triangle/` | Triangle matching: geometry, voting, KNN formation |
| `ransac/` | RANSAC loop, MAGSAC++ scorer, transform estimation |
| `distortion/` | SIP polynomial (integrated), TPS (not integrated) |
| `interpolation/` | Kernels, `warp_image()`, row warping (SIMD + scalar) |
| `spatial/` | K-d tree (flat implicit layout, KNN + radius search) |
