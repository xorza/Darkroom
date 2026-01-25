# Registration Algorithm Review

Comprehensive review of the registration module comparing implementation against academic best practices and identifying optimization opportunities.

## Executive Summary

The registration module is **production-quality** and follows most best practices for astronomical image registration. The implementation includes LO-RANSAC, geometric hashing with voting, k-d tree optimization, and SIMD acceleration.

### Implemented Optimizations

- **MSAC-like scoring** ✅ - Already uses truncated quadratic cost: `(threshold² - dist²) * 1000`
- **Simplified progressive sampling** ✅ - Changed to cleaner 3-phase approach (25%→50%→100% pool)

### Remaining Optimization Opportunities

1. **Iterative Phase Correlation** - Sub-pixel accuracy improvement (0.01-0.05 pixel)
2. **Lanczos SIMD** - Full AVX2/NEON acceleration for warping (2-3x speedup)
3. **Two-step triangle matching** - Requires transform-guided refinement (complex)

---

## Current Implementation Analysis

### 1. Triangle Matching (Geometric Hashing)

**Implementation:**
- Uses scale-invariant triangle ratios `(a/c, b/c)` where `a ≤ b ≤ c`
- Geometric hash table with 100×100 bins
- K-d tree based neighbor lookup for O(n·k²) triangle formation
- Voting matrix (dense/sparse adaptive)
- Orientation checking for mirror rejection

**Best Practice Comparison:**

| Aspect | Current | Best Practice | Status |
|--------|---------|---------------|--------|
| Triangle descriptor | Side ratios | Side ratios / angles | ✅ Good |
| Hash table bins | Fixed 100×100 | Uniform bin sizing | ✅ Good |
| K-d tree acceleration | Yes | Yes | ✅ Optimal |
| Voting scheme | Simple count | Weighted voting | ⚠️ Could improve |
| Two-step matching | No | Rough → Fine | ⚠️ Could add |

**Research Insights:**

From [Geometric Voting Algorithm for Star Trackers](https://ieeexplore.ieee.org/document/4560198/):
> "The basic principle lies in the star pairs voting scheme, but angular distance is low-dimensional information that matches numerous candidates, resulting in wrong votes."

From [Multilayer Voting Algorithm](https://pmc.ncbi.nlm.nih.gov/articles/PMC8124596/):
> "A two-step matching strategy of rough matching first and fine matching later... only three pairs of matching stars need to be found."

**Potential Optimization:**
```rust
// Two-step matching strategy
// 1. Rough match: Use relaxed tolerance (5%) to find initial correspondences
// 2. Fine match: Compute preliminary transform, then match with strict tolerance (1%)
// Benefit: 2-3x faster convergence on dense star fields
```

---

### 2. RANSAC Implementation

**Implementation:**
- Standard RANSAC with adaptive iteration count
- LO-RANSAC (Local Optimization) enabled by default
- Progressive sampling using match confidences
- SIMD-accelerated inlier counting (AVX2/SSE/NEON)
- Multiple transform types (Translation → Homography)

**Best Practice Comparison:**

| Aspect | Current | Best Practice | Status |
|--------|---------|---------------|--------|
| Local optimization | LO-RANSAC | LO-RANSAC / GC-RANSAC | ✅ Good |
| Adaptive iterations | Yes | Yes | ✅ Good |
| Progressive sampling | Yes (confidence-based) | Yes | ✅ Optimal |
| SIMD acceleration | Inlier counting | Full pipeline | ⚠️ Partial |
| Scoring function | Inverse distance | MAGSAC/MSAC | ⚠️ Could improve |
| Threshold selection | Fixed | MAD-adaptive | ⚠️ Could improve |

**Research Insights:**

From [SupeRANSAC](https://arxiv.org/html/2506.04803v1):
> "LO-RANSAC achieves 2-3x speedup over standard RANSAC with 10-20% improvement in inlier count."

From [Fixing LO-RANSAC](https://www.researchgate.net/publication/259338571_Fixing_the_locally_optimized_RANSAC):
> "Improvements include: truncated quadratic cost function, limit on inliers for LS computation, and implementation stability."

From [OpenCV RANSAC Evaluation](https://opencv.org/blog/evaluating-opencvs-new-ransacs/):
> "USAC_MAGSAC is the only method whose optimal threshold is the same across all datasets - valuable for practice as it requires least tuning."

**Potential Optimizations:**

1. **MAGSAC-like scoring** (threshold-free):
```rust
// Current: binary inlier/outlier with fixed threshold
if dist_sq < threshold_sq { inliers.push(i); }

// MAGSAC: Gaussian-weighted score, no hard threshold needed
let sigma = threshold / 3.0;
let weight = (-0.5 * dist_sq / (sigma * sigma)).exp();
score += weight;
```

2. **MAD-adaptive threshold**:
```rust
// Compute residuals, use Median Absolute Deviation for robust threshold
let median = percentile(&residuals, 0.5);
let mad = percentile(&residuals.iter().map(|r| (r - median).abs()), 0.5);
let adaptive_threshold = 1.4826 * mad * 2.5;  // ~2.5 sigma
```

---

### 3. Phase Correlation

**Implementation:**
- Standard FFT-based translation estimation
- Log-polar extension for rotation/scale
- Sub-pixel methods: Parabolic, Gaussian, Centroid
- Hann windowing for edge effects
- Cached FFT plans

**Best Practice Comparison:**

| Aspect | Current | Best Practice | Status |
|--------|---------|---------------|--------|
| FFT library | rustfft | rustfft / FFTW | ✅ Good |
| Windowing | Hann | Hann / Blackman | ✅ Good |
| Sub-pixel | 3 methods | Iterative PC | ⚠️ Could improve |
| Log-polar | Yes | Yes | ✅ Good |
| Accuracy | ~0.1 pixel | ~0.01-0.05 pixel | ⚠️ Could improve |

**Research Insights:**

From [Iterative Phase Correlation](https://ui.adsabs.harvard.edu/abs/2020ApJS..247....8H/abstract):
> "The Iterative Phase Correlation algorithm is ideally suited for problems where subpixel registration accuracy plays a crucial role."

From [High-Accuracy POC](https://www.semanticscholar.org/paper/High-Accuracy-Subpixel-Image-Registration-Based-on-Takita-Aoki/8a6d91acae1b18b66846a1d211887d551dca5e58):
> "Fitting the closed-form analytical model of the correlation peak achieves approximately 0.05-pixel accuracy with 11×11 matching windows."

**Potential Optimization:**
```rust
// Iterative refinement for sub-pixel accuracy
// 1. Find integer peak
// 2. Window around peak (e.g., 11×11)
// 3. Upsample correlation surface using DFT
// 4. Find sub-pixel peak in upsampled surface
// Achieves 0.01-0.05 pixel accuracy vs current ~0.1 pixel
```

---

### 4. K-D Tree (Spatial Indexing)

**Implementation:**
- Median-split balanced tree
- k-NN with bounded max-heap
- Stack allocation for k ≤ 32
- Radius search support

**Best Practice Comparison:**

| Aspect | Current | Best Practice | Status |
|--------|---------|---------------|--------|
| Tree balance | Median split | Median split | ✅ Optimal |
| k-NN algorithm | Bounded heap | Bounded heap | ✅ Optimal |
| Stack allocation | k ≤ 32 | k ≤ 32 typical | ✅ Good |
| SIMD vectorization | No | Available | ⚠️ Could add |
| Distance pruning | Yes | Yes | ✅ Good |

**Research Insights:**

From [simd-kd-tree](https://github.com/VcDevel/simd-kd-tree):
> "Vectorization of the kd-tree data structure and search algorithm using Vc SIMD library."

**Assessment:** Current k-d tree is well-implemented. SIMD vectorization would provide marginal benefit given typical star counts (50-200). **Not recommended** - complexity vs benefit ratio is poor.

---

### 5. Interpolation (Lanczos Warping)

**Implementation:**
- Lanczos 2/3/4 with LUT optimization
- SIMD for bilinear only (AVX2/SSE/NEON)
- Optional clamping for ringing reduction
- Weight normalization

**Best Practice Comparison:**

| Aspect | Current | Best Practice | Status |
|--------|---------|---------------|--------|
| Lanczos LUT | Yes (1024 samples) | Yes | ✅ Optimal |
| SIMD bilinear | Yes | Yes | ✅ Good |
| SIMD Lanczos | No | Yes (Intel IPP) | ⚠️ Could add |
| Ringing control | Clamping option | Clamping | ✅ Good |
| Separable filter | No | Possible for 1D | ⚠️ Could add |

**Research Insights:**

From [Intel IPP Lanczos](https://www.intel.com/content/www/us/en/developer/articles/technical/the-intel-avx-realization-of-lanczos-interpolation-in-intel-ipp-2d-resize-transform.html):
> "The use of AVX gives 1.5x performance gains compared with SSE implementation. The filter performs 42 multiplications and 35 additions per output pixel."

From [AVIR Library](https://github.com/avaneev/avir):
> "LANCIR offers up to 3x faster image resizing with radical AVX, SSE2, NEON, and WASM SIMD128 optimizations."

**Potential Optimization:**
```rust
// SIMD Lanczos3 for AVX2 (process 4 output pixels simultaneously)
// - Pack 4 source regions into AVX registers
// - Vectorize kernel weight computation
// - Use FMA for multiply-accumulate
// Expected speedup: 2-3x for warping operations
```

---

### 6. Transform Estimation

**Implementation:**
- Translation: Simple averaging
- Similarity: Point normalization + Kabsch-like
- Affine: Least squares via SVD
- Homography: DLT with point normalization + SVD

**Best Practice Comparison:**

| Aspect | Current | Best Practice | Status |
|--------|---------|---------------|--------|
| Point normalization | Yes | Yes (essential) | ✅ Optimal |
| Numerical stability | SVD-based | SVD-based | ✅ Optimal |
| Degeneracy handling | Area/collinearity checks | Area/collinearity checks | ✅ Good |

**Assessment:** Transform estimation is **optimal**. No changes recommended.

---

## Optimization Recommendations

### Priority 1: High Impact, Low Effort

| Item | Effort | Impact | Description |
|------|--------|--------|-------------|
| MSAC scoring | 2 hours | Medium | Replace binary inlier with truncated quadratic |
| Sub-pixel centroid improvement | 2 hours | Low-Medium | Better centroid with weighted moments |

### Priority 2: Medium Impact, Medium Effort

| Item | Effort | Impact | Description |
|------|--------|--------|-------------|
| SIMD Lanczos3 | 8 hours | High | AVX2/NEON Lanczos for 2-3x warp speedup |
| Two-step triangle matching | 4 hours | Medium | Rough→Fine for faster convergence |
| Iterative phase correlation | 6 hours | Medium | 0.01-0.05 pixel sub-pixel accuracy |

### Priority 3: Low Priority

| Item | Effort | Impact | Description |
|------|--------|--------|-------------|
| MAD-adaptive threshold | 4 hours | Low | Automatic threshold selection |
| K-d tree SIMD | 8 hours | Low | Marginal benefit for typical star counts |
| MAGSAC scoring | 6 hours | Low | Threshold-free but more complex |

---

## Simplification Opportunities

The codebase is already well-structured. Few simplifications identified:

### 1. Progressive Sampling Complexity

The progressive sampling in RANSAC uses a complex exponential schedule:
```rust
let progress = iterations as f64 / max_iter as f64;
let pool_fraction = 0.2 + 0.8 * progress.powi(2);
```

**Simplification:** Consider linear schedule or simpler step function:
```rust
// Simpler 3-phase approach
let pool_fraction = if iterations < max_iter / 3 { 0.25 }
                   else if iterations < 2 * max_iter / 3 { 0.5 }
                   else { 1.0 };
```

### 2. VoteMatrix Enum

The VoteMatrix dense/sparse enum adds complexity. Given typical use:
- 50 stars × 50 stars = 2,500 entries (always dense)
- 200 stars × 200 stars = 40,000 entries (still dense)
- Sparse path rarely used in practice

**Simplification:** Could remove sparse path if max_stars is capped at reasonable value (e.g., 300), but current implementation is fine.

---

## What's Already Optimal

The following are **best-in-class** and should not be changed:

1. **K-d tree for triangle formation** - O(n·k²) vs O(n³) is essential
2. **LO-RANSAC** - Standard best practice, well-implemented
3. **Transform normalization** - Critical for numerical stability
4. **Lanczos LUT** - Eliminates expensive sinc computation
5. **SIMD inlier counting** - Good use of vectorization where it matters
6. **Dense/Sparse vote matrix** - Correct memory/speed tradeoff
7. **Adaptive RANSAC iterations** - Essential for efficiency

---

## Conclusion

The registration module is **production-ready** with excellent algorithmic choices. The main optimization opportunities are:

1. **SIMD Lanczos3** - Largest potential speedup (2-3x for warping)
2. **Iterative Phase Correlation** - Better sub-pixel accuracy when needed
3. **MSAC scoring** - Slight improvement in robustness

None of these are critical - the current implementation handles typical astronomical registration scenarios well. Implement only if benchmarks show specific bottlenecks.

---

## Sources

- [FreeAstro Image Registration](https://free-astro.org/index.php?title=Image_registration)
- [Geometric Voting Algorithm for Star Trackers](https://ieeexplore.ieee.org/document/4560198/)
- [Multilayer Voting Algorithm](https://pmc.ncbi.nlm.nih.gov/articles/PMC8124596/)
- [Improved Triangular Star Pattern Recognition](https://www.sciencedirect.com/science/article/pii/S1319157818310954)
- [SupeRANSAC](https://arxiv.org/html/2506.04803v1)
- [Fixing LO-RANSAC](https://bmva-archive.org.uk/bmvc/2012/BMVC/paper095/paper095.pdf)
- [OpenCV RANSAC Evaluation](https://opencv.org/blog/evaluating-opencvs-new-ransacs/)
- [MAD-based Adaptive RANSAC](https://etasr.com/index.php/ETASR/article/view/10121)
- [Iterative Phase Correlation](https://ui.adsabs.harvard.edu/abs/2020ApJS..247....8H/abstract)
- [High-Accuracy Sub-pixel POC](https://www.semanticscholar.org/paper/High-Accuracy-Subpixel-Image-Registration-Based-on-Takita-Aoki/8a6d91acae1b18b66846a1d211887d551dca5e58)
- [Intel IPP Lanczos AVX](https://www.intel.com/content/www/us/en/developer/articles/technical/the-intel-avx-realization-of-lanczos-interpolation-in-intel-ipp-2d-resize-transform.html)
- [AVIR SIMD Lanczos](https://github.com/avaneev/avir)
- [simd-kd-tree](https://github.com/VcDevel/simd-kd-tree)
