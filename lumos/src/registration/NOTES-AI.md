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

### Dead Code / Unused Config

2. **Interpolation config fields never wired up**
   `config.rs:108-112` — `border_value`, `normalize_kernel`, `clamp_output` are
   defined but never passed to `warp_image()`. Either implement or remove.

### Important Missing Features

7. **No Lanczos deringing/clamping**
   PixInsight uses clamping threshold (default 0.3) to prevent dark halos around
   bright stars. `clamp_output` config exists but is unimplemented.

8. **Pipeline always uses star centroid as SIP reference, never image center**
   `registration/mod.rs:318` passes `reference_point: None`. SIP standard uses
   CRPIX (image center). Radial distortion is centered on optical axis, not
   star centroid.

9. **Single-pass match recovery**
   `mod.rs:300-307` — Recovery + refit runs once. PixInsight iterates predict ->
   re-match -> refit until convergence, recovering more matches.

10. **TPS not integrated** — fully implemented and tested (`distortion/tps/`)
    but marked `dead_code`. PixInsight's primary distortion method.

11. **No SIMD for Lanczos3** — the default interpolation method has no SIMD path.
    Separable two-pass AVX2 could yield 2-4x speedup (Intel IPP approach).

12. **Missing IRWLS for MAGSAC++** — paper's key contribution for model accuracy.
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
| Lanczos deringing | Config exists, not impl | Yes (0.3 threshold) | Yes | N/A | N/A |
| SIMD warp | AVX2 bilinear only | Full SIMD | OpenCV SIMD | N/A | N/A |
| Match recovery | Single-pass | Iterative | N/A | Bayesian | N/A |
| Output framing | Fixed (=input) | Max/min/COG | Max/min/current/COG | N/A | Fixed |

### Prioritized Improvements

**Medium Effort (correctness/quality):**
1. Pass image center as SIP reference_point when available
2. Implement Lanczos deringing (clamp_output config exists)
3. Wire up or remove dead Config fields (border_value, etc.)
4. Add preemptive scoring to MAGSAC++ (break early when loss > best)
5. Iterative match recovery (2-3 passes of recover + refit)

**Larger Effort (performance/features):**
6. Separable SIMD Lanczos3 (two-pass horizontal+vertical with AVX2)
7. Integrate TPS as alternative distortion model
8. IRWLS final polish with MAGSAC++ weights
9. Upgrade to quad descriptors (4D hash, matches Astrometry.net)

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
