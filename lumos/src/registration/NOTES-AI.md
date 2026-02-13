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

### Bugs Found

1. **Triangle vertex correspondence is broken** (moderate severity)
   `triangle/geometry.rs:48-96`, `triangle/voting.rs:143-149`
   Vertices stored in numeric index order, not geometric-role order. Voting pairs
   `ref_tri.indices[i]` with `target_tri.indices[i]` by position, but positions
   have no geometric meaning. ~2/3 of votes per triangle match are cast for wrong
   correspondences. Mitigated by voting mechanism (correct votes accumulate, wrong
   ones scatter), but requires ~3x more triangles than necessary.
   **Fix:** Reorder `indices` by geometric role in `from_positions()`.

2. **LO-RANSAC runs on every hypothesis** (moderate, performance)
   `ransac/mod.rs:310` — LO triggers when `inlier_buf.len() >= min_samples`,
   not just on new-best models. Standard LO-RANSAC and OpenCV run LO only on
   new best. Wastes significant computation.
   **Fix:** Add `score > best_score` check before LO.

3. **`precise_wide_field` ratio_tolerance is inverted** (config bug)
   `config.rs:199` — `ratio_tolerance: 0.005` is *tighter* than default 0.01.
   For wide-field images with more distortion, it should be *looser* (0.02-0.03).

4. **Auto validation vs min_matches** (config bug)
   `config.rs:237` — When `transform_type == Auto`, `min_points()` returns 2
   (Similarity), but Auto can upgrade to Homography (needs 4). A `min_matches`
   of 3 passes validation but would be insufficient for Homography.

### Dead Code / Unused Config

5. **SIP sigma-clipping fields declared but never used**
   `distortion/sip/mod.rs:66-71` — `SipConfig.clip_sigma` and `clip_iterations`
   exist with defaults but `fit_from_transform` never uses them. False API promise.

6. **Interpolation config fields never wired up**
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
| Vertex ordering | Bug (numeric) | Correct | Correct | N/A (hash) | Correct (geometric) |
| RANSAC scoring | MAGSAC++ | Standard | OpenCV | Bayesian odds | RANSAC |
| IRWLS polish | No | N/A | N/A | N/A | No |
| Distortion | SIP (forward) | TPS (iterative) | SIP (from WCS) | SIP (full A/B/AP/BP) | None |
| Sigma-clipping SIP | Config exists, not impl | N/A | N/A | Yes | N/A |
| Lanczos deringing | Config exists, not impl | Yes (0.3 threshold) | Yes | N/A | N/A |
| SIMD warp | AVX2 bilinear only | Full SIMD | OpenCV SIMD | N/A | N/A |
| Match recovery | Single-pass | Iterative | N/A | Bayesian | N/A |
| Output framing | Fixed (=input) | Max/min/COG | Max/min/current/COG | N/A | Fixed |

### Prioritized Improvements

**Quick Wins (bug fixes, high impact, low effort):**
1. Fix vertex correspondence ordering in triangle geometry (~5 lines)
2. Add `score > best_score` guard before LO-RANSAC (~1 line)
3. Fix `precise_wide_field` ratio_tolerance (1 line)
4. Fix Auto validation to use Homography min_points (1 line)

**Medium Effort (correctness/quality):**
5. Implement SIP sigma-clipping (config fields already exist)
6. Pass image center as SIP reference_point when available
7. Implement Lanczos deringing (clamp_output config exists)
8. Wire up or remove dead Config fields (border_value, etc.)
9. Add preemptive scoring to MAGSAC++ (break early when loss > best)
10. Iterative match recovery (2-3 passes of recover + refit)

**Larger Effort (performance/features):**
11. Separable SIMD Lanczos3 (two-pass horizontal+vertical with AVX2)
12. Integrate TPS as alternative distortion model
13. IRWLS final polish with MAGSAC++ weights
14. Upgrade to quad descriptors (4D hash, matches Astrometry.net)

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
