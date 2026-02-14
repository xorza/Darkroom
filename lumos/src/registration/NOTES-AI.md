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

### Auto Transform Selection (mod.rs:158-180)
`TransformType::Auto` starts with Similarity, upgrades to Homography if
RMS > 0.5px. The 0.5px threshold is absolute. FWHM-adaptive scaling would
be more robust (e.g., 0.25 * median_fwhm).

### Match Recovery (mod.rs:353-430)
ICP-style iterative recovery: project unmatched ref stars through transform,
find nearest target in k-d tree, refit, repeat up to 5 iterations. Ensures
result never has fewer matches than the input seed. This is a correct and
effective technique -- matches PixInsight's iterative refinement approach.

### max_sigma Derivation (mod.rs:130-132)
`max_sigma = max(median_fwhm * 0.5, 0.5)`. Effective threshold is ~3 * max_sigma
(from chi-squared quantile). For typical seeing (FWHM 2-4px), this gives
threshold ~3-6px. Floor at 0.5px prevents too-tight thresholds on sharp optics.

## Cross-Cutting Research Summary

Research conducted against: PixInsight, Siril, Astrometry.net, Astroalign,
AstroPixelProcessor, ASTAP, OpenCV USAC, SupeRANSAC (Barath 2025),
MAGSAC++ paper (Barath et al. 2020), Piedade et al. 2025, Groth 1986,
Valdes 1995, Tabur 2007, Lang 2010, SWarp, LSST pipeline, nanoflann,
scipy KDTree, kiddo, Mitchell-Netravali 1988, Hartley & Zisserman 2003,
Umeyama 1991, Shupe et al. 2005 (SIP standard), starmatch (PyPI 2025).

### What We Do Correctly
- **All 5 transform estimators** verified correct (translation, euclidean, similarity, affine, homography)
- **MAGSAC++ scoring** is a correct k=2 specialization; confirmed equivalent to Gaussian-Uniform likelihood (Piedade et al. 2025); adopted by SupeRANSAC (Barath 2025) as the default scoring function
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
- **Hartley normalization** for both affine and homography estimation; direct SVD on 2n x 9 A for homography
- **Triangle vertex ordering** by geometric role (opposite shortest/middle/longest side)
- **Groth R=10 elongation filter** and Heron's formula degeneracy check
- **Pipeline architecture** matches industry standard (detect -> match -> estimate -> warp)
- **Homography as default for wide-field** matches Siril recommendation (homography is "well-suited for the general case and strongly recommended for wide-field images")
- **Double precision for SIP** matches SIP standard recommendation ("single precision strongly discouraged")

### Prioritized Issues Across All Submodules

#### Quick Fixes (easy, localized changes)
1. ~~**Replace GammaLut with `1-exp(-x)`** (ransac/magsac.rs)~~ -- **FIXED**
2. ~~**Fix L2/L-inf tolerance mismatch** (triangle/voting.rs)~~ -- **FIXED**
3. ~~**Fix orientation test comments** (triangle/tests.rs:365,1050)~~ -- **FIXED** (clarified: symmetric pattern creates ambiguous correspondences)
4. ~~**Check target point degeneracy** (ransac/mod.rs)~~ -- **FIXED**
5. ~~**Align confidence defaults**~~ -- **FIXED**
6. ~~**Add `nearest_one()`** to k-d tree~~ -- **FIXED**
7. ~~**Use `f64::total_cmp()`** in k-d tree~~ -- **FIXED**
8. ~~**Fix stale README.md files** (distortion/README.md, distortion/sip/README.md)~~ -- **FIXED**
9. **`#[allow(dead_code)]` on pub fields** (ransac/mod.rs:126-131) -- use `pub(crate)` instead

#### Medium Effort (moderate changes, meaningful impact)
10. ~~**Soft deringing threshold**~~ -- **DONE** (matches PixInsight exactly)
11. ~~**Direct SVD for homography DLT**~~ -- **DONE**
12. ~~**Use actual FMA intrinsics in Lanczos3 SIMD kernel**~~ -- **DONE** (no-dering path uses _mm_fmadd_ps, ~2.5% improvement)
13. **Generic incremental stepping** (interpolation) -- benefit Lanczos2/4/Bicubic with ~38% speedup
14. **Remove duplicate bilinear** (interpolation/mod.rs:155-170 vs warp/mod.rs:109-127) -- consolidate
15. **LO-RANSAC buffer replacement** (ransac/mod.rs:335) -- `inlier_buf = lo_inliers` defeats pre-allocation
16. **Increase default ratio_tolerance to 0.02** (config.rs) -- **DECLINED**: 0.01 is conservative but `precise_wide_field()` preset already offers 0.02 for noisy data
17. ~~**Add point normalization to affine estimation** (ransac/transforms.rs:166)~~ -- **FIXED** (Hartley normalization + denormalize via compose)
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
29. **Output framing options** -- PixInsight and Siril both support max/min/COG framing; we only support fixed (=input)
30. **Multi-pass matching with different tolerances** -- starmatch (2024) uses different pixel tolerances for initial/secondary matching, improving success rate

### Comparison with Industry Tools

| Aspect | This Crate | PixInsight | Siril | Astrometry.net | Astroalign | ASTAP | APP |
|--------|-----------|------------|-------|----------------|------------|-------|-----|
| Star matching | Triangles (2D) | Polygons (6D) | Triangles | Quads (4D) | Triangles (2D) | Triangles/Quads | Proprietary |
| Vertex ordering | Correct (geometric role) | Correct | Correct | N/A (hash) | Correct (geometric) | Hash-based | N/A |
| RANSAC scoring | MAGSAC++ | Standard | OpenCV RANSAC | Bayesian odds | RANSAC | N/A | N/A |
| LO-RANSAC | Iterative LS | N/A | N/A | N/A | No | N/A | N/A |
| Transform types | Trans/Eucl/Sim/Aff/Homo | All + TPS | Shift/Sim/Aff/Homo | SIP+CD | Affine | Affine+SIP | DDC |
| Distortion | SIP (forward) | TPS (iterative) | SIP (from WCS) | SIP (full A/B/AP/BP) | None | SIP | DDC |
| Sigma-clipping SIP | Yes (3-iter, 3-sigma MAD) | N/A | N/A | Yes | N/A | N/A | N/A |
| Lanczos deringing | Soft (PixInsight-style) | Soft threshold (0.3) | Binary toggle | N/A | N/A | Bilinear only | Lanczos+M-N |
| SIMD warp | AVX2 bilinear; SSE FMA Lanczos3 | Full SIMD | OpenCV SIMD | N/A | N/A | N/A | N/A |
| Match recovery | Iterative (ICP-style) | Iterative | N/A | Bayesian | N/A | N/A | N/A |
| K-d tree | Custom flat implicit | N/A | N/A | libkd (FITS I/O) | scipy | N/A | N/A |
| Output framing | Fixed (=input) | Max/min/COG | Max/min/current/COG | N/A | Fixed | Fixed | Configurable |
| Inverse warp | Forward mapping | Reverse mapping | Reverse mapping | N/A | Forward | Reverse | Reverse |

### Key Research Conclusions

- **Pipeline architecture is sound** -- matches the detect-match-estimate-warp flow used by all major tools
- **MAGSAC++ scoring is the right choice** -- validated by Piedade et al. (2025) as equivalent to optimal GaU likelihood; adopted as default by SupeRANSAC (Barath et al. 2025), the state-of-the-art unified RANSAC framework
- **SIP over TPS for default** is reasonable -- more constrained, less overfitting risk, FITS compatible; Siril also uses SIP since v1.3
- **Triangle matching works for our scale** (50-200 stars) -- quads/polygons only needed for dense fields or blind solving; ASTAP supports both triangles and quads, noting quads work best for uniqueness
- **Soft deringing matches PixInsight exactly** -- previously the biggest gap, now resolved
- **No critical bugs found across any submodule** -- all issues are improvements, not correctness failures
- **Most impactful open improvement**: generic incremental stepping for Lanczos2/4/Bicubic (~38% estimated speedup for non-Lanczos3 methods)
- **K-d tree is solid** -- no bugs, appropriate for our scale, only potential improvement is L-infinity search
- **SupeRANSAC (2025) validates our approach**: their comprehensive evaluation found MAGSAC++ achieves "the best trade-off in terms of average model accuracy and robustness to parameter choices" -- the same scorer we use
- **Multi-pass tolerance** (starmatch 2024) is a potentially useful refinement not yet implemented
- **Block homography** (Li et al. 2020) for anisoplanatism is beyond current scope but relevant for very wide fields

### Per-Module Summary

| Module | Correctness | Issues | Top Priority |
|--------|------------|--------|-------------|
| triangle/ | All correct | Tight default tolerance (0.01), excessive triangles at k=20 | Benchmark k=8-10 |
| ransac/ | All 5 estimators verified, MAGSAC++ correct (confirmed by SupeRANSAC 2025) | LO buffer replacement, dead_code annotations | Fix LO buffer |
| distortion/ | SIP direction/coords/solver all correct, TPS correct | No inverse SIP, no fit diagnostics, TPS not integrated | Add fit quality return |
| interpolation/ | All kernels correct, deringing matches PixInsight | Duplicate bilinear, no generic incremental stepping | Generic incremental stepping |
| spatial/ | No bugs found, all algorithms textbook correct | None (minor: could add L-inf radius search) | L-infinity radius search |

## Detailed Analysis: What We Have vs What Industry Tools Offer

### Star Matching Quality

**Our approach (Groth/Valdes triangles)**: 2D invariant space (s0/s2, s1/s2). O(n*k^2)
triangle formation via KNN k-d tree. Vote accumulation + greedy one-to-one resolution.

**PixInsight (polygon descriptors)**: 6D invariant space for default pentagon. 3x lower
uncertainty than our triangles. Cannot handle mirror transforms. The paper notes that
"less uncertainty leads to less false star pair matches" and "the overall image
registration process is considerably more robust and efficient even under difficult
conditions."

**Astrometry.net (quad descriptors)**: 4D hash codes from pairs of stars in a local
coordinate frame defined by the two most distant stars. 2x discriminating power over
triangles. Designed for blind solving against full-sky catalogs.

**Assessment**: Our triangle approach is adequate for image-to-image registration with
50-200 stars. PixInsight's polygon approach would improve robustness in dense fields
but adds significant complexity. For our use case, the downstream RANSAC handles the
higher false-positive rate from triangles effectively.

### RANSAC Quality

**Our approach**: MAGSAC++ scoring (k=2 closed-form), progressive 3-phase sampling,
LO-RANSAC (iterative LS on inliers), adaptive early termination, plausibility checks
(rotation/scale bounds).

**SupeRANSAC (Barath 2025)**: The state-of-the-art unified RANSAC integrates MAGSAC++
scoring, GC-RANSAC for local optimization, progressive sampling (NAPSAC), SPRT for
early model rejection, DEGENSAC for homography degeneracy. Their extensive evaluation
confirms MAGSAC++ as the best scoring function.

**OpenCV USAC**: MAGSAC++, GC-RANSAC, PROSAC, SPRT, DEGENSAC -- the full toolbox.

**Assessment**: We have the most important components: MAGSAC++ scoring, LO-RANSAC,
adaptive termination, and domain-specific plausibility checks. The missing features
(GC-RANSAC, SPRT, DEGENSAC, true PROSAC) matter more for general computer vision
(thousands of SIFT matches at 20% inlier ratio) than for star registration (200 matches
at 50%+ inlier ratio after triangle voting). Our ~18-iteration convergence for typical
workloads makes sampling strategy irrelevant.

### Transform Estimation Quality

All 5 estimators are mathematically correct:
- **Translation**: exact average displacement
- **Euclidean**: constrained Procrustes (Umeyama 1991)
- **Similarity**: Procrustes + scale (Umeyama 1991)
- **Affine**: normal equations with Hartley normalization + 3x3 explicit inverse
- **Homography**: DLT with Hartley normalization + direct SVD (Hartley & Zisserman 2003)

All estimators now use appropriate normalization: homography uses Hartley normalization
for DLT, and affine uses the same normalize_points() + denormalize pattern for numerical
stability with large coordinate ranges.

### Distortion Correction Quality

**Our SIP**: Forward polynomial (A/B), orders 2-5, Cholesky+LU solver, MAD sigma-clipping,
avg-distance normalization. No inverse polynomial (AP/BP), no FITS I/O.

**Siril SIP**: Since v1.3, applies SIP correction before linear transform (same direction).
Reads from WCS headers. Since v1.4 (Dec 2025), supports distortion correction during
alignment using WCS SIP data with cubic SIP as default. The correction follows the same
convention we implement.

**Astrometry.net SIP**: Full A/B/AP/BP with QR solver, no normalization (raw pixel offsets).
Higher order support (up to 9). Grid-sampled inverse fitting.

**PixInsight TPS**: Non-parametric, handles arbitrary local distortions. Iterative
successive approximation scheme. More flexible than SIP for complex optics.

**Assessment**: Our SIP implementation is correct and well-conditioned. The main gaps are
(1) no inverse polynomial for FITS interoperability, (2) no FITS header I/O, and
(3) TPS not integrated for complex distortion patterns. For the registration-only use
case, these gaps have no practical impact.

### Interpolation Quality

**Our Lanczos3 + soft deringing**: Matches PixInsight's algorithm exactly (same formula,
same default threshold 0.3, same quadratic fade). SIMD-optimized with incremental stepping
for linear transforms. LUT-based kernel evaluation (4096 samples/unit).

**PixInsight**: Lanczos-3 default with clamping threshold 0.3. Same approach.

**Siril**: Lanczos-4 default with optional clamping. Slightly wider kernel, marginal
quality difference at the cost of more compute per pixel.

**SWarp**: Lanczos-4 default, no deringing. Flux conservation via Jacobian.

**AstroPixelProcessor**: Supports Lanczos and Mitchell-Netravali. Drizzle integration.

**ASTAP**: Bilinear only (simpler but lower quality). 2025 update added "full implementation
of reverse mapping with bilinear interpolation for less background noise."

**Assessment**: Our interpolation is at parity with PixInsight, the gold standard. FMA
intrinsics are now used in the Lanczos3 SIMD kernel (no-dering path). The main remaining
open item is generic incremental stepping for Lanczos2/4/Bicubic (~38% estimated speedup).

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

## References

### Papers
- Groth 1986 -- Original triangle matching algorithm
- Valdes et al. 1995 -- FOCAS compressed triangle ratios
- Tabur 2007 -- Optimistic Pattern Matching
- Lang et al. 2010 -- Astrometry.net blind calibration with quads
- Beroiz et al. 2020 -- Astroalign triangle matching
- Barath et al. 2020 -- MAGSAC++ (CVPR 2020)
- Piedade et al. 2025 -- RANSAC scoring functions equivalence (arXiv:2512.19850)
- Barath et al. 2025 -- SupeRANSAC unified framework (arXiv:2506.04803)
- Fischler & Bolles 1981 -- Original RANSAC
- Umeyama 1991 -- Procrustes transform estimation
- Hartley & Zisserman 2003 -- Multiple View Geometry (DLT, normalization)
- Shupe et al. 2005 -- SIP distortion convention
- Mitchell & Netravali 1988 -- Cubic filter parameter space
- Li et al. 2020 -- Block homography for astronomical registration
- GMTV 2022 -- Global Multi-Triangle Voting

### Software
- PixInsight StarAlignment (polygon descriptors, TPS distortion, soft Lanczos clamping)
- Siril 1.4.1 (triangle matching, OpenCV RANSAC, SIP since v1.3, distortion correction since v1.4)
- Astrometry.net (quad descriptors, Bayesian verification, SIP A/B/AP/BP)
- Astroalign (Python triangle matching, scipy k-d tree)
- AstroPixelProcessor 2.0 (DDC distortion, Lanczos/Mitchell-Netravali, drizzle)
- ASTAP (triangle/quad matching, SIP, reverse mapping bilinear)
- starmatch v1.0 (PyPI 2025, multi-pass matching with adaptive tolerances)
- OpenCV USAC (MAGSAC++, GC-RANSAC, PROSAC, SPRT)
- SupeRANSAC (danini/superansac, unified RANSAC 2025)
