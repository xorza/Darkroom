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

Research conducted against: PixInsight, Siril, Astrometry.net, Astroalign, OpenCV USAC,
MAGSAC++ paper, Groth 1986, Valdes 1995, Tabur 2007, Lang 2010, SWarp, LSST pipeline.

### What We Do Correctly
- **All 5 transform estimators** verified correct (translation, euclidean, similarity, affine, homography)
- **MAGSAC++ scoring** is a structurally correct k=2 specialization of the paper
- **SIP polynomial direction** is correct (forward A/B, applied before linear transform)
- **SIP coordinate normalization** improves on the standard (reduces condition number)
- **SIP sigma-clipping** implemented and active (MAD-based, 3 iterations, 3-sigma)
- **K-d tree** implementation is clean, correct, well-tested (no bugs found)
- **Lanczos LUT** precision (4096 samples/unit) is adequate for f32 data
- **Kernel normalization** applied correctly in both generic and optimized paths
- **Incremental stepping** optimization is mathematically correct for affine transforms
- **Adaptive iteration count** formula matches standard (Fischler & Bolles 1981)
- **Hartley normalization** for DLT homography is standard and correct; direct SVD on 2n×9 A (not A^T A)
- **Triangle vertex ordering** by geometric role (opposite shortest/middle/longest side)
- **Lanczos3 FMA SIMD kernel** with fused deringing: -59% single-threaded, -52% MT 4k
- **Pipeline architecture** matches industry standard (detect → match → estimate → warp)

### Prioritized Issues Across All Submodules

#### Quick Fixes (easy, localized changes)
1. ~~**Replace GammaLut with `1-exp(-x)`** (ransac/magsac.rs)~~ — **FIXED**: replaced ~70-line LUT with 6-line `gamma_k2()` closed-form
2. ~~**Fix L2/L-inf tolerance mismatch** (triangle/voting.rs)~~ — **FIXED**: search radius multiplied by √2
3. **Fix orientation test comments** (triangle/tests.rs:365,1050) — rotation preserves orientation
4. ~~**Check target point degeneracy** (ransac/mod.rs)~~ — **FIXED**: added `is_sample_degenerate(&sample_target)`
5. ~~**Align confidence defaults**~~ — **FIXED**: RansacParams::default() now uses 0.995 to match Config::default()
6. ~~**Add `nearest_one()`** to k-d tree~~ — **FIXED**: dedicated method with scalar tracker, used in match recovery
7. ~~**Use `f64::total_cmp()`** in k-d tree~~ — **FIXED**: eliminates NaN panic path in build and k_nearest sort

#### Medium Effort (moderate changes, meaningful impact)
8. ~~**Soft deringing threshold**~~ — **DONE.** PixInsight-style soft clamping with `f32` threshold (default 0.3), `sp/sn/wp/wn` accumulation, SIMD branchless splitting
9. ~~**Direct SVD for homography DLT**~~ — **DONE.** Builds 2n×9 design matrix A, direct SVD (zero-padded to 9×9 for 4-point minimum samples)
10. **TPS coordinate normalization** (distortion/tps/mod.rs) — center+scale before building matrix
11. **SIP condition number monitoring** (distortion/sip/mod.rs:406) — detect ill-conditioned systems
12. **SIP order validation** (distortion/sip/mod.rs:138) — require `n >= 3 * terms.len()` not just `n >= terms.len()`
13. **Generic incremental stepping** (interpolation) — benefit Lanczos2/4/Bicubic with ~38% speedup
14. **Weighted triangle voting** (triangle/voting.rs) — weight by inverse density in invariant space
15. **Spatial coverage in quality score** (result.rs:150) — prevent degenerate registrations with clustered matches

#### Architecture (larger, revisit when needed)
16. **IRWLS final polish for MAGSAC++** — sigma-marginalized weights for sub-pixel accuracy
17. **Star quality filtering** before registration — eccentricity, SNR, saturation
18. **FWHM-adaptive auto-upgrade threshold** (mod.rs:168) — current 0.5px is absolute, should scale
19. **TPS integration** into pipeline — fully implemented/tested but dead code
20. **Inverse SIP polynomial (AP/BP)** — needed for FITS header export
21. **Quad descriptors** — only needed for blind all-sky solving or very dense fields

### Comparison with Industry Tools

| Aspect | This Crate | PixInsight | Siril | Astrometry.net | Astroalign |
|--------|-----------|------------|-------|----------------|------------|
| Star matching | Triangles (2D) | Polygons (6D) | Triangles | Quads (4D) | Triangles (2D) |
| Vertex ordering | Correct (geometric role) | Correct | Correct | N/A (hash) | Correct (geometric) |
| RANSAC scoring | MAGSAC++ | Standard | OpenCV | Bayesian odds | RANSAC |
| IRWLS polish | No | N/A | N/A | N/A | No |
| Distortion | SIP (forward) | TPS (iterative) | SIP (from WCS) | SIP (full A/B/AP/BP) | None |
| Sigma-clipping SIP | Yes (3-iter, 3-sigma MAD) | N/A | N/A | Yes | N/A |
| Lanczos deringing | Hard min/max | Soft threshold (0.3) | Binary toggle | N/A | N/A |
| SIMD warp | AVX2 bilinear; Lanczos3 FMA | Full SIMD | OpenCV SIMD | N/A | N/A |
| Match recovery | Iterative (ICP-style) | Iterative | N/A | Bayesian | N/A |
| Output framing | Fixed (=input) | Max/min/COG | Max/min/current/COG | N/A | Fixed |

### Key Research Conclusions
- **Pipeline architecture is sound** — matches the detect-match-estimate-warp flow used by all major tools
- **MAGSAC++ scoring is the right choice** — better than Siril (basic RANSAC) and Astroalign
- **SIP over TPS for default** is reasonable — more constrained, less overfitting risk
- **Triangle matching works for our scale** (50-200 stars) — quads/polygons only needed for dense fields
- **No critical bugs found** — issues are improvements, not correctness failures
- **Biggest gap is soft deringing** — PixInsight's threshold approach preserves more sharpness

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
