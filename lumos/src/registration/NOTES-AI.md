# registration Module - Implementation Notes

Complete astronomical image registration pipeline: triangle asterism matching
via k-d tree, RANSAC with MAGSAC++ scoring and LO-RANSAC, transform models
(Translation through Homography), optional SIP distortion correction, image
warping with Lanczos interpolation and SIMD-optimized row warping.

See submodule NOTES-AI.md files for detailed per-module analysis:
- `triangle/NOTES-AI.md` -- matching algorithm, vertex correspondence, invariants
- `ransac/NOTES-AI.md` -- MAGSAC++ fidelity, LO-RANSAC, transform estimation
- `distortion/NOTES-AI.md` -- SIP correctness, TPS status, fitting analysis
- `interpolation/NOTES-AI.md` -- kernels, SIMD, LUT, deringing, performance
- `spatial/NOTES-AI.md` -- k-d tree correctness, performance

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
MAGSAC++ paper (Barath et al. 2020), Groth 1986, Valdes 1995, Tabur 2007, Lang 2010,
SWarp, LSST pipeline, nanoflann, scipy KDTree, kiddo, Mitchell-Netravali 1988,
Hartley & Zisserman 2003, Umeyama 1991, Shupe et al. 2005 (SIP standard).

### What We Do Correctly
- **All 5 transform estimators** verified correct (translation, euclidean, similarity, affine, homography)
- **MAGSAC++ scoring** is a correct k=2 specialization; confirmed equivalent to Gaussian-Uniform likelihood (Piedade et al. 2025)
- **SIP polynomial direction** correct (forward A/B, applied before linear transform)
- **SIP coordinate normalization** improves on the standard (reduces condition number)
- **SIP sigma-clipping** correct (MAD-based, 3 iterations, 3-sigma, one-sided rejection)
- **SIP minimum point count** correct (3x terms, matches Astrometry.net recommendation)
- **TPS kernel** correct (`r^2 * ln(r)` with proper zero-limit handling)
- **TPS coordinate normalization** correct (bbox center + scale)
- **K-d tree** implementation clean, correct, well-tested (no bugs found)
- **KNN pruning** textbook correct (near-first traversal, bounded max-heap)
- **Radius search pruning** correct (handles on-plane case properly)
- **Lanczos kernel** matches standard `sinc(pi*x) * sinc(pi*x/a)` exactly
- **Lanczos LUT** precision (4096 samples/unit) adequate for f32 data
- **Kernel normalization** correct in both generic and optimized paths
- **Soft deringing** matches PixInsight's algorithm exactly (quadratic fade, threshold 0.3)
- **SIMD deringing** branchless SSE mask splitting is correct
- **Bicubic Catmull-Rom** correct (a=-0.5, verified K(0)=1, K(1)=0, K(2)=0)
- **Incremental stepping** mathematically correct for affine transforms
- **Adaptive iteration count** matches standard formula (Fischler & Bolles 1981)
- **Hartley normalization** for DLT homography standard and correct; direct SVD on 2n x 9 A
- **Triangle vertex ordering** by geometric role (opposite shortest/middle/longest side)
- **Groth R=10 elongation filter** and Heron's formula degeneracy check
- **Pipeline architecture** matches industry standard (detect -> match -> estimate -> warp)

### Prioritized Issues Across All Submodules

#### Quick Fixes (easy, localized changes)
1. ~~**Replace GammaLut with `1-exp(-x)`** (ransac/magsac.rs)~~ -- **FIXED**
2. ~~**Fix L2/L-inf tolerance mismatch** (triangle/voting.rs)~~ -- **FIXED**
3. ~~**Fix orientation test comments** (triangle/tests.rs:365,1050)~~ -- **FIXED** (clarified: symmetric pattern creates ambiguous correspondences)
4. ~~**Check target point degeneracy** (ransac/mod.rs)~~ -- **FIXED**
5. ~~**Align confidence defaults**~~ -- **FIXED**
6. ~~**Add `nearest_one()`** to k-d tree~~ -- **FIXED**
7. ~~**Use `f64::total_cmp()`** in k-d tree~~ -- **FIXED**
8. ~~**Fix stale README.md files** (distortion/README.md, distortion/sip/README.md)~~ -- **FIXED** (removed nonexistent inverse methods, fixed test counts, fixed outlier rejection status)
9. **`#[allow(dead_code)]` on pub fields** (ransac/mod.rs:126-131) -- use `pub(crate)` instead

#### Medium Effort (moderate changes, meaningful impact)
10. ~~**Soft deringing threshold**~~ -- **DONE** (matches PixInsight exactly)
11. ~~**Direct SVD for homography DLT**~~ -- **DONE**
12. **Use actual FMA intrinsics in Lanczos3 SIMD kernel** (interpolation/warp/sse.rs:338-341) -- function requires FMA but uses mul+add instead of fmadd. ~5-10% speedup + accuracy improvement.
13. **Generic incremental stepping** (interpolation) -- benefit Lanczos2/4/Bicubic with ~38% speedup
14. **Remove duplicate bilinear** (interpolation/mod.rs:155-170 vs warp/mod.rs:109-127) -- consolidate
15. **LO-RANSAC buffer replacement** (ransac/mod.rs:335) -- `inlier_buf = lo_inliers` defeats pre-allocation
16. **Increase default ratio_tolerance to 0.02** (config.rs) -- **DECLINED**: 0.01 is conservative but `precise_wide_field()` preset already offers 0.02 for noisy data
17. **Add point normalization to affine estimation** (ransac/transforms.rs:166) -- Hartley-style, improves conditioning
18. **Weighted triangle voting** (triangle/voting.rs) -- weight by inverse density in invariant space (GMTV 2022)
19. **SIP fit quality return type** -- return rms_residual, points_rejected, condition_number instead of just Option
20. **Spatial coverage in quality score** (result.rs:150) -- prevent degenerate registrations with clustered matches
21. **L-infinity radius search** (spatial/mod.rs) -- eliminate sqrt(2) workaround in voting, reduce candidates ~22%

#### Architecture (larger, revisit when needed)
22. **IRWLS final polish for MAGSAC++** -- sigma-marginalized weights for sub-pixel accuracy (postponed: clean inlier/outlier separation makes this low-impact)
23. **Star quality filtering** before registration -- eccentricity, SNR, saturation
24. **FWHM-adaptive auto-upgrade threshold** (mod.rs:168) -- current 0.5px is absolute, should scale
25. **TPS integration** into pipeline -- fully implemented/tested but dead code
26. **Inverse SIP polynomial (AP/BP)** -- needed for FITS header export
27. **Quad descriptors** -- only needed for blind all-sky solving or very dense fields
28. **Reduce adaptive k or benchmark k=8** (triangle/matching.rs:60) -- k=20 generates excessive triangles for 150 stars

### Comparison with Industry Tools

| Aspect | This Crate | PixInsight | Siril | Astrometry.net | Astroalign |
|--------|-----------|------------|-------|----------------|------------|
| Star matching | Triangles (2D) | Polygons (6D) | Triangles | Quads (4D) | Triangles (2D) |
| Vertex ordering | Correct (geometric role) | Correct | Correct | N/A (hash) | Correct (geometric) |
| RANSAC scoring | MAGSAC++ | Standard | OpenCV | Bayesian odds | RANSAC |
| LO-RANSAC | Iterative LS | N/A | N/A | N/A | No |
| IRWLS polish | No | N/A | N/A | N/A | No |
| Distortion | SIP (forward) | TPS (iterative) | SIP (from WCS) | SIP (full A/B/AP/BP) | None |
| Sigma-clipping SIP | Yes (3-iter, 3-sigma MAD) | N/A | N/A | Yes | N/A |
| Lanczos deringing | Soft (PixInsight-style) | Soft threshold (0.3) | Binary toggle | N/A | N/A |
| SIMD warp | AVX2 bilinear; SSE FMA Lanczos3 | Full SIMD | OpenCV SIMD | N/A | N/A |
| Match recovery | Iterative (ICP-style) | Iterative | N/A | Bayesian | N/A |
| K-d tree | Custom flat implicit | N/A | N/A | libkd (FITS I/O) | scipy |
| Output framing | Fixed (=input) | Max/min/COG | Max/min/current/COG | N/A | Fixed |

### Key Research Conclusions
- **Pipeline architecture is sound** -- matches the detect-match-estimate-warp flow used by all major tools
- **MAGSAC++ scoring is the right choice** -- validated by Piedade et al. (2025) as equivalent to optimal GaU likelihood
- **SIP over TPS for default** is reasonable -- more constrained, less overfitting risk, FITS compatible
- **Triangle matching works for our scale** (50-200 stars) -- quads/polygons only needed for dense fields
- **Soft deringing matches PixInsight exactly** -- previously the biggest gap, now resolved
- **No critical bugs found across any submodule** -- all issues are improvements, not correctness failures
- **Most impactful open improvement**: use actual FMA intrinsics in Lanczos3 SIMD kernel (interpolation/warp/sse.rs)
- **K-d tree is solid** -- no bugs, appropriate for our scale, only potential improvement is L-infinity search

### Per-Module Summary

| Module | Correctness | Issues | Top Priority |
|--------|------------|--------|-------------|
| triangle/ | All correct | Tight default tolerance (0.01), excessive triangles at k=20, misleading test comments | Increase tolerance to 0.02 |
| ransac/ | All 5 estimators verified, MAGSAC++ correct | LO buffer replacement, dead_code annotations, affine lacks normalization | Fix LO buffer, add affine normalization |
| distortion/ | SIP direction/coords/solver all correct, TPS correct | Stale READMEs, no inverse SIP, no fit diagnostics, TPS not integrated | Fix stale docs, add fit quality return |
| interpolation/ | All kernels correct, deringing matches PixInsight | SIMD kernel doesn't use FMA, duplicate bilinear | Use actual FMA intrinsics |
| spatial/ | No bugs found, all algorithms textbook correct | None (minor: could add L-inf radius search) | L-infinity radius search |

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
| `spatial/` | K-d tree (flat implicit layout, KNN + radius + nearest_one) |
