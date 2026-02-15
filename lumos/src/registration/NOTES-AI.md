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

### Auto Transform Selection (mod.rs:165-187)
`TransformType::Auto` starts with Similarity, upgrades to Homography if
RMS > 0.5px. The 0.5px threshold is absolute. FWHM-adaptive scaling would
be more robust (e.g., 0.25 * median_fwhm).

### Match Recovery (mod.rs:377-459)
ICP-style iterative recovery: project unmatched ref stars through transform,
find nearest target in k-d tree, refit, repeat up to 5 iterations. Ensures
result never has fewer matches than the input seed. Matches PixInsight's
iterative refinement approach.

### max_sigma Derivation (mod.rs:130-132)
`max_sigma = max(median_fwhm * 0.5, 0.5)`. Effective threshold is ~3 * max_sigma
(from chi-squared quantile). For typical seeing (FWHM 2-4px), this gives
threshold ~3-6px. Floor at 0.5px prevents too-tight thresholds on sharp optics.

## Cross-Cutting Research Summary

Research conducted against: PixInsight, Siril 1.4, Astrometry.net, Astroalign,
AstroPixelProcessor, ASTAP, OpenCV USAC, SupeRANSAC (Barath 2025),
MAGSAC++ paper (Barath et al. 2020), Piedade et al. 2025, Groth 1986,
Valdes 1995, Tabur 2007, Lang 2010, SWarp, LSST pipeline, nanoflann,
scipy KDTree, kiddo, Mitchell-Netravali 1988, Hartley & Zisserman 2003,
Umeyama 1991, Shupe et al. 2005, starmatch (PyPI 2025), GMTV 2022,
Bookstein 1989, GNU Astronomy Utilities, AVIR, Intel IPP.

### What We Do Correctly
- **All 5 transform estimators** verified correct (translation, euclidean, similarity, affine, homography)
- **MAGSAC++ scoring** is a correct k=2 specialization; confirmed equivalent to Gaussian-Uniform likelihood (Piedade et al. 2025); adopted by SupeRANSAC (Barath 2025) as the default scoring function
- **SIP polynomial direction** correct (forward A/B, applied before linear transform)
- **SIP coordinate normalization** improves on the standard (reduces condition number vs Astrometry.net's raw pixel offsets)
- **SIP sigma-clipping** correct (MAD-based, 3 iterations, 3-sigma, one-sided rejection)
- **SIP linear term exclusion** correct (only generates p+q >= 2 terms, linear absorbed by homography)
- **SIP dual solver** Cholesky + LU fallback with condition monitoring (1e5 threshold on L diagonal ratio)
- **TPS kernel** correct (`r^2 * ln(r)` matches Bookstein 1989, PixInsight, ALGLIB)
- **TPS coordinate normalization** correct (bbox center + scale prevents kernel amplification)
- **K-d tree** implementation clean, correct, well-tested (no bugs found in any of 5 verifications)
- **KNN pruning** textbook correct (near-first traversal, bounded max-heap, strict `<` on far prune)
- **Radius search pruning** correct (handles on-plane case, `diff == 0` searches both subtrees)
- **nearest_one** dedicated 1-NN with scalar tracker (no allocation, matches kiddo optimization)
- **Lanczos kernel** matches standard `sinc(pi*x) * sinc(pi*x/a)` exactly (verified vs Wikipedia, PixInsight PCL, Mazzo)
- **Per-pixel weight normalization** prevents flux loss (NASA SkyView had ~0.3% error without it)
- **Soft deringing** matches PixInsight's algorithm exactly (quadratic fade, threshold 0.3, identical accumulation logic)
- **SIMD deringing** branchless SSE mask splitting is correct (verified accumulation equivalence)
- **Bicubic Catmull-Rom** correct (a=-0.5, K(0)=1, K(1)=0, K(2)=0, continuity at x=1)
- **f64 incremental stepping** worst-case error ~9e-13 pixels over 4096 columns (no Kahan needed)
- **Adaptive iteration count** matches standard formula (Fischler & Bolles 1981)
- **Hartley normalization** for both affine and homography (confirmed identical to SupeRANSAC's normalization)
- **DLT homography** direct SVD on rectangular A (preserves kappa vs kappa^2, matches H&Z Algorithm 4.2)
- **Procrustes estimation** correct closed-form (Translation, Euclidean, Similarity match Umeyama 1991)
- **Triangle vertex ordering** by geometric role (opposite shortest/middle/longest side)
- **Groth R=10 elongation filter** and Heron's formula degeneracy check
- **Pipeline architecture** matches industry standard (detect -> match -> estimate -> warp)
- **Flat implicit k-d tree** more memory-efficient than nanoflann (8 vs 24-32 bytes/node)
- **Stack-allocated BoundedMaxHeap** for k<=32 (eliminates allocation in hot KNN path)
- **Buffer-reuse radius API** `radius_indices_into(&mut Vec)` avoids per-query allocation
- **Const-generic Lanczos SIMD** unified kernel for L2/L3/L4 (unusual; most tools have separate code)

### Prioritized Issues Across All Submodules

#### Quick Fixes (easy, localized changes)
1. ~~**Replace GammaLut with `1-exp(-x)`** (ransac/magsac.rs)~~ -- **FIXED**
2. ~~**Fix L2/L-inf tolerance mismatch** (triangle/voting.rs)~~ -- **FIXED**
3. ~~**Fix orientation test comments** (triangle/tests.rs)~~ -- **FIXED**
4. ~~**Check target point degeneracy** (ransac/mod.rs)~~ -- **FIXED**
5. ~~**Align confidence defaults**~~ -- **FIXED**
6. ~~**Add `nearest_one()`** to k-d tree~~ -- **FIXED**
7. ~~**Use `f64::total_cmp()`** in k-d tree~~ -- **FIXED**
8. **Fix stale README.md files** (distortion/README.md references nonexistent methods; sip/README.md says "no sigma-clipping" but it's implemented)
9. ~~**`#[allow(dead_code)]` on pub fields**~~ -- **FIXED** (targeted per-field `#[allow(dead_code)] // Used in tests`)
10. **Fix misleading MAGSAC++ continuity comment** (ransac/magsac.rs:46) -- says "ensuring continuity" but outlier_loss has ~3.6% discontinuity at threshold boundary
11. **Fix `capacity + 1` in BoundedMaxHeap::Large** (spatial/mod.rs:349) -- the `+ 1` is unused
12. **Move heap allocation after empty check** (spatial/mod.rs:128-132) -- `k_nearest` creates heap before checking empty tree

#### Medium Effort (moderate changes, meaningful impact)
13. ~~**Soft deringing threshold**~~ -- **DONE** (matches PixInsight exactly)
14. ~~**Direct SVD for homography DLT**~~ -- **DONE**
15. ~~**Use actual FMA intrinsics in Lanczos3 SIMD kernel**~~ -- **DONE** (~2.5% improvement)
16. ~~**Generic incremental stepping** (interpolation)~~ -- **DONE** (L2 -29.3%, L4 -45.4%, L3 -6.3%)
17. ~~**Add point normalization to affine estimation**~~ -- **FIXED** (Hartley normalization + denormalize)
18. **Remove duplicate bilinear** (interpolation/mod.rs vs warp/mod.rs) -- consolidate, keep `fast_floor_i32`
19. **L-infinity radius search** (spatial/mod.rs) -- eliminate sqrt(2) workaround in voting, reduce candidates ~22%, simpler pruning
20. **Extract `dim_value()` helper** (spatial/mod.rs) -- `if dim == 0 { p.x } else { p.y }` repeated 8 times
21. **Weighted triangle voting** (triangle/voting.rs) -- weight by inverse density in invariant space (GMTV 2022)
22. **SIP fit quality return type** -- return rms_residual, points_rejected, condition_number
23. **Spatial coverage in quality score** (result.rs) -- prevent degenerate registrations with clustered matches

#### Architecture (larger, revisit when needed)
24. **IRWLS final polish for MAGSAC++** -- sigma-marginalized weights for sub-pixel accuracy (SupeRANSAC uses Cauchy-weighted IRWLS; postponed: clean inlier/outlier separation makes this low-impact)
25. **Star quality filtering** before registration -- eccentricity, SNR, saturation
26. **FWHM-adaptive auto-upgrade threshold** (mod.rs:175) -- current 0.5px is absolute, should scale
27. **TPS integration** into pipeline -- fully implemented/tested but dead code; needs: outlier rejection, non-zero regularization default (PixInsight uses 0.25), point simplification for N>1000
28. **Inverse SIP polynomial (AP/BP)** -- needed for FITS header export (standard approach: grid-sample, fit inverse at AP_ORDER = A_ORDER + 1)
29. **Quad descriptors** -- only needed for blind all-sky solving or very dense fields
30. **Output framing options** -- PixInsight and Siril both support max/min/COG framing; we only support fixed (=input)
31. **Multi-pass matching with different tolerances** -- starmatch (2024) uses different pixel tolerances for initial/secondary matching

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
| SIMD warp | AVX2 bilinear; SSE FMA Lanczos | Full SIMD | OpenCV SIMD | N/A | N/A | N/A | N/A |
| Match recovery | Iterative (ICP-style) | Iterative | N/A | Bayesian | N/A | N/A | N/A |
| K-d tree | Custom flat implicit | N/A | N/A | libkd (FITS I/O) | scipy | N/A | N/A |
| Output framing | Fixed (=input) | Max/min/COG | Max/min/current/COG | N/A | Fixed | Fixed | Configurable |

### Key Research Conclusions

- **Pipeline architecture is sound** -- matches the detect-match-estimate-warp flow used by all major tools
- **MAGSAC++ scoring is the right choice** -- validated by Piedade et al. (2025) as equivalent to optimal GaU likelihood; adopted as default by SupeRANSAC (Barath et al. 2025), the state-of-the-art unified RANSAC framework
- **SIP over TPS for default** is reasonable -- more constrained, less overfitting risk, FITS compatible; Siril also uses SIP since v1.3; Siril 1.4 (Dec 2025) added SIP undistortion during registration, validating our approach
- **Triangle matching works for our scale** (50-200 stars) -- quads/polygons only needed for dense fields or blind solving
- **Soft deringing matches PixInsight exactly** -- same formula, same threshold, same quadratic fade
- **No correctness bugs found across any submodule** -- all open issues are improvements or code quality, not algorithmic failures
- **K-d tree is solid** -- no bugs, 5 independent correctness verifications passed, appropriate design tradeoffs for our scale
- **SupeRANSAC (2025) validates our approach**: MAGSAC++ scoring + LO-RANSAC covers the most impactful components; missing features (GC-RANSAC, SPRT, DEGENSAC) provide diminishing returns at our 50%+ inlier ratio
- **Interpolation at parity with PixInsight** -- kernel correctness, deringing, SIMD optimization all match or exceed
- **SIP implementation well-conditioned** -- avg-distance normalization is better than Astrometry.net (raw pixel offsets); dual solver handles edge cases
- **TPS implementation correct but not integrated** -- needs outlier rejection, regularization default, point limit before pipeline use

### Per-Module Summary

| Module | Correctness | Open Issues | Top Priority |
|--------|------------|-------------|-------------|
| triangle/ | All correct (Groth, Valdes, vertex ordering, degeneracy filters) | Missing C<0.99 cosine filter (low), no configurable k | Weighted voting by rarity (GMTV) |
| ransac/ | All 5 estimators verified, MAGSAC++ confirmed by SupeRANSAC 2025 | Misleading continuity comment, `dead_code` annotations, phase boundaries don't adapt | Fix comments + annotations |
| distortion/ | SIP direction/coords/solver all correct, TPS matches Bookstein 1989 | Stale README.md files, no inverse SIP, TPS not integrated | Fix stale READMEs |
| interpolation/ | All kernels correct, deringing matches PixInsight exactly | Duplicate bilinear implementations | Consolidate bilinear |
| spatial/ | No bugs (5 verifications), all algorithms textbook correct | capacity+1, heap before empty check, repeated dim_value pattern | L-infinity radius search |

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
- Chum, Matas, Kittler 2003 -- LO-RANSAC
- Chum & Matas 2005 -- PROSAC
- Umeyama 1991 -- Procrustes transform estimation
- Hartley 1997 -- In Defense of the Eight-Point Algorithm (normalization)
- Hartley & Zisserman 2003 -- Multiple View Geometry (DLT)
- Shupe et al. 2005 -- SIP distortion convention
- Bookstein 1989 -- Principal Warps (TPS)
- Mitchell & Netravali 1988 -- Cubic filter parameter space
- GMTV 2022 -- Global Multi-Triangle Voting (weighted by rarity)
- Khuong & Morin 2015 -- Array Layouts for Comparison-Based Searching
- Amenta et al. -- K-D Trees Better Cut on Longest Side

### Software
- PixInsight StarAlignment (polygon descriptors, TPS distortion, soft Lanczos clamping)
- Siril 1.4 (triangle matching, OpenCV RANSAC, SIP since v1.3, distortion correction since v1.4)
- Astrometry.net (quad descriptors, Bayesian verification, SIP A/B/AP/BP)
- Astroalign (Python triangle matching, scipy k-d tree)
- AstroPixelProcessor 2.0 (DDC distortion, Lanczos/Mitchell-Netravali, drizzle)
- ASTAP (triangle/quad matching, SIP, reverse mapping bilinear)
- starmatch v1.0 (PyPI 2025, multi-pass matching with adaptive tolerances)
- OpenCV USAC (MAGSAC++, GC-RANSAC, PROSAC, SPRT)
- SupeRANSAC (danini/superansac, unified RANSAC 2025)
- nanoflann (C++ k-d tree, leaf_max_size, max-spread split)
- scipy KDTree (sliding midpoint, L-infinity support)
- kiddo (Rust k-d tree, Eytzinger/vEB layouts)
- SWarp (Lanczos-4, flux conservation via Jacobian)
- GNU Astronomy Utilities (area resampling / pixel mixing)
- LSST pipeline (WCS grid interpolation, Lanczos warp)
