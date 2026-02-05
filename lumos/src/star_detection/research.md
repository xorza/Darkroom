# Star detection algorithms for a high-precision Rust library

**The highest-precision star detection combines a SExtractor-style background mesh with either wavelet-based or matched-filter detection, connected component segmentation, and Moffat PSF fitting for sub-pixel centroiding.** This pipeline, when implemented with f64 arithmetic for fitting and careful attention to the techniques below, can achieve **<0.05 pixel centroid accuracy** for well-sampled stars with SNR >50. The two foundational references are Bertin & Arnouts 1996 (SExtractor) and Stetson 1987 (DAOPHOT); modern tools like PixInsight, Siril, and SEP build directly on these. What follows is a complete algorithm specification suitable for implementation in Rust, covering every pipeline stage with mathematical formulas, data structures, and precision guidance.

---

## The complete detection pipeline at a glance

The canonical star detection pipeline runs in this order, with each stage feeding the next:

1. **Background modeling** — Estimate and subtract spatially varying sky background; produce per-pixel noise (RMS) map
2. **Pre-detection filtering** — Optionally convolve with matched filter (Gaussian kernel ≈ PSF) to maximize point-source SNR
3. **Thresholding** — Flag pixels exceeding `k × σ_local` above background
4. **Connected component labeling** — Group flagged pixels into candidate objects using 8-connectivity
5. **Deblending** — Split merged detections using multi-threshold tree decomposition
6. **Measurement** — Compute centroid, flux, shape parameters for each candidate
7. **Validation & filtering** — Reject non-stellar detections via sharpness, roundness, FWHM, saturation checks
8. **Sub-pixel refinement** — Refine centroids via windowed centroiding or PSF (Gaussian/Moffat) fitting
9. **Catalog output** — Emit star records with positions, fluxes, shape parameters, SNR, and quality flags

SExtractor processes this in two passes (background estimation, then streaming detection+measurement). DAOFIND takes a different approach—convolving with a zero-sum Gaussian kernel that simultaneously subtracts background—but the output is similar. For maximum precision, **combine SExtractor-style background estimation with PSF-fitting centroiding**.

---

## Background modeling: the foundation of detection accuracy

Accurate background estimation is critical because detection thresholds and flux measurements depend on it. The SExtractor mesh approach is the gold standard.

### SExtractor-style background mesh

**Step 1 — Grid division.** Divide the image into rectangular cells of `BACK_SIZE × BACK_SIZE` pixels. Typical values: **64×64** (default), with 32×32 for images with rapid background variation and 128–256 for smooth backgrounds. Cells must be substantially larger than the PSF FWHM.

**Step 2 — Per-cell robust statistics.** For each cell, compute iterative sigma-clipped mean and standard deviation:

```
repeat:
    μ = mean(pixels)
    σ = stddev(pixels)
    reject pixels where |pixel - μ| > 3σ
until σ changes by <20% between iterations
```

If σ converges (field is uncrowded), use the **clipped mean** as the cell background. If σ does not converge (crowded field), use **mode estimation** via:

```
Mode ≈ 2.5 × Median − 1.5 × Mean    (after clipping)
```

This is SExtractor's modified Pearson formula with α = 2.5. If `|Mean − Median| / σ > 0.3`, the distribution is too skewed—fall back to the simple **median**. Compute the clipped σ in parallel as the cell's RMS value.

**Step 3 — Median filter the mesh.** Apply a 2D median filter of size `BACK_FILTERSIZE` (default **3×3 cells**) to the low-resolution grid. This suppresses cells contaminated by bright objects and reduces bicubic spline ringing.

**Step 4 — Bicubic spline interpolation.** Interpolate the filtered grid to full resolution using natural bicubic splines (second derivatives = 0 at boundaries). Implementation: solve a tridiagonal system per row via the Thomas algorithm (O(n)), then interpolate along columns. This yields two full-resolution maps: **background(x,y)** and **σ_rms(x,y)**.

### Noise estimation methods

The per-cell RMS from sigma-clipping is the primary noise estimate. Three robust alternatives for global noise estimation:

**MAD (Median Absolute Deviation)** is the most robust single estimator:
```
σ = 1.4826 × median(|Xi − median(X)|)
```
The factor **1.4826 = 1/Φ⁻¹(0.75)** makes the estimator consistent for Gaussian distributions. MAD has 50% breakdown point (tolerates up to 50% outliers).

**Biweight midvariance** offers ~98% efficiency for Gaussian while remaining robust:
```
ζ = n × Σ(|ui|<1) (xi − M)²(1 − ui²)⁴ / [Σ(|ui|<1) (1 − ui²)(1 − 5ui²)]²
```
where `ui = (xi − M)/(9 × MAD)` and M = median. Points with |ui| ≥ 1 get zero weight.

**PixInsight's MRS (Multiresolution Support)** noise estimator uses the à trous wavelet transform to classify pixels into significant structures versus noise, then computes σ from non-significant pixels only. This achieves ~1% accuracy for Gaussian noise.

---

## Detection: thresholding and matched filtering

### Matched filter convolution

Before thresholding, convolve the background-subtracted image with a kernel matched to the expected PSF. For point sources, a **Gaussian kernel** with FWHM matching the seeing is optimal. This improves SNR by approximately **√(N_kernel_pixels)**. Detection operates on the convolved image; measurements use the original.

SExtractor supports Gaussian, Mexican hat, tophat, and custom kernels. The kernel should be normalized so that the convolved pixel values represent the amplitude of the best-fitting scaled kernel at each position.

### Threshold computation

The per-pixel detection threshold is:
```
T(x,y) = DETECT_THRESH × σ_rms(x,y)
```
where `DETECT_THRESH` is in units of background σ. Typical values:

- **k = 1.5**: Aggressive detection, many false positives
- **k = 2.0–3.0**: Standard range; k=2.0 with MINAREA=5 yields nominal SNR ≈ 3.35
- **k = 5.0**: Very conservative, bright sources only

A pixel is flagged if `(convolved_image(x,y) − background(x,y)) > T(x,y)`. The `DETECT_MINAREA` parameter (default **5 pixels**) requires a minimum number of connected above-threshold pixels, which dramatically reduces false positives from noise.

### DAOFIND's alternative: zero-sum kernel convolution

DAOFIND takes an elegant shortcut by constructing a **zero-sum Gaussian kernel** that simultaneously performs background subtraction and matched filtering:

```
G(x,y) = exp(−0.5 × (x²/σx² + y²/σy²))
H(x,y) = G(x,y) − c,    where c chosen so Σ H(x,y) = 0
σ = 0.42466 × FWHM
```

Convolving with H produces a "density enhancement map" where values represent the amplitude of the best-fitting lowered Gaussian. The effective detection threshold becomes `threshold × relerr` where `relerr = 1/√(Σ H²)`. Local maxima above this threshold in the convolved image are star candidates.

---

## Segmentation: connected component labeling and deblending

### Two-pass connected component labeling with Union-Find

The standard approach uses **8-connectivity** (including diagonal neighbors) and a two-pass algorithm:

**Pass 1 (forward scan):** For each above-threshold pixel, examine the 4 already-scanned neighbors (NW, N, NE, W). If none are labeled, assign a new label. If one or more are labeled, assign the minimum label and record equivalences in a Union-Find structure.

**Pass 2 (relabel):** Replace each label with its root via `find()` with path compression.

The Union-Find data structure with path compression and union-by-rank achieves nearly O(N) complexity:

```rust
struct UnionFind {
    parent: Vec<u32>,
    rank: Vec<u8>,
}
impl UnionFind {
    fn find(&mut self, x: u32) -> u32 {
        if self.parent[x as usize] != x {
            self.parent[x as usize] = self.find(self.parent[x as usize]);
        }
        self.parent[x as usize]
    }
    fn union(&mut self, a: u32, b: u32) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra == rb { return; }
        if self.rank[ra as usize] < self.rank[rb as usize] {
            self.parent[ra as usize] = rb;
        } else {
            self.parent[rb as usize] = ra;
            if self.rank[ra as usize] == self.rank[rb as usize] {
                self.rank[ra as usize] += 1;
            }
        }
    }
}
```

### SExtractor deblending: multi-threshold tree decomposition

Each connected component is re-thresholded at **DEBLEND_NTHRESH** (default 32) exponentially spaced levels between the detection threshold and the peak value:

```
t_k = t_detect × (peak / t_detect)^(k / DEBLEND_NTHRESH),   k = 0..N
```

This builds a tree (dendrogram). A branch becomes a separate object if its integrated flux exceeds `DEBLEND_MINCONT × total_flux` (default **0.005 = 0.5%**). Pixels below the split threshold are assigned to the nearest progenitor using a bivariate Gaussian probability model.

---

## Sub-pixel centroiding: from basic to maximum precision

Centroiding is where the biggest precision gains are made. Methods range from ~0.3 pixel accuracy (basic center of gravity) to **<0.01 pixel** (ePSF fitting). Here are all major methods ranked by accuracy.

### Center of gravity (barycenter)

The simplest method computes intensity-weighted first moments over above-threshold pixels:
```
x_c = Σ(xi × Ii) / Σ(Ii)
y_c = Σ(yi × Ii) / Σ(Ii)
```
where Ii is background-subtracted intensity. **Typical accuracy: 0.1–0.3 pixels.** Suffers from pixel-phase bias (systematic error depending on fractional pixel position of true center), truncation bias from finite aperture, and noise bias. The center-of-gravity method **never saturates the Cramér-Rao lower bound** (Vakili & Hogg 2016)—it always underperforms optimal estimators.

**Windowed (Gaussian-weighted) centroid** significantly improves on basic CoG. SExtractor's `XWIN_IMAGE/YWIN_IMAGE` uses iterative Gaussian-weighted centroiding:
```
x̄(t+1) = Σ(wi × Ii × xi) / Σ(wi × Ii)
wi = exp(−ri² / (2σ²_win))
```
where ri is distance from current estimate, σ_win = half-flux diameter / 2.355, and iteration continues until convergence. This is initialized with the isophotal centroid and converges in 3–5 iterations.

### 1D Gaussian fitting (DAOFIND approach)

Project the star cutout onto x and y axes (marginal sums), then fit 1D Gaussians to each:
```
S_x(x) = Σ_y I(x,y)    →    fit G(x) = A·exp(−(x−x₀)²/(2σ²)) + B
```
Can be linearized via logarithm for fast fitting. **Accuracy: 0.05–0.1 pixels** for well-sampled PSFs (FWHM > 3 pixels). Degrades significantly for undersampled PSFs positioned near pixel corners (errors >0.7 pixels per STScI testing). The fit also yields marginal heights (hx, hy) used for DAOFIND's roundness criterion: `roundness2 = 2(hx − hy)/(hx + hy)`.

### 2D Gaussian fitting

The full elliptical 2D Gaussian with rotation has **7 parameters**:
```
I(x,y) = A·exp(−a(x−x₀)² − 2b(x−x₀)(y−y₀) − c(y−y₀)²) + B

where:  a = cos²θ/(2σx²) + sin²θ/(2σy²)
        b = −sin(2θ)/(4σx²) + sin(2θ)/(4σy²)
        c = sin²θ/(2σx²) + cos²θ/(2σy²)
```

Fit via **Levenberg-Marquardt**: compute the Jacobian J, then iterate `δβ = (JᵀJ + λ·diag(JᵀJ))⁻¹ · Jᵀr` until relative parameter change < 10⁻⁸. Initialize with: A₀ = peak − median(edges), (x₀, y₀) from CoG, σ₀ = FWHM_est / 2.3548, θ₀ = 0, B₀ = median(edges). **Accuracy: <0.05 pixels** for SNR > 50, potentially **0.01–0.02 pixels** for SNR > 200.

### Moffat PSF fitting: the most physically accurate model

Real stellar PSFs have extended wings that Gaussians cannot model. The **Moffat function** is superior:
```
I(r) = I₀ × (1 + (r/α)²)^(−β) + B
FWHM = 2α√(2^(1/β) − 1)
```

For ground-based seeing, **β ≈ 2.5–4.0** (β → ∞ gives a Gaussian). As Stetson explicitly notes: "A Gaussian function is not a particularly good match to a real stellar profile... a Moffat function would be much better." Moffat's power-law wings (falling as r^(−2β)) correctly model atmospheric scattering. The elliptical 2D Moffat with rotation has **8 parameters**: (I₀, x₀, y₀, αx, αy, θ, β, B). Initialize β₀ = 2.5. **Accuracy: 0.01–0.05 pixels** — the best parametric model for ground-based data.

### Effective PSF fitting (Anderson & King): the gold standard

For undersampled data (FWHM < 2 pixels), Anderson & King (2000) pioneered **effective PSF (ePSF)** fitting. The ePSF is an empirically constructed model that incorporates both the optical PSF and the pixel response function. Built from dithered observations of many stars, it is sampled on a sub-pixel grid (typically 4× oversampled). ePSF fitting achieves **0.008–0.01 pixel** accuracy even on severely undersampled HST images, resolving pixel-phase biases that plague all parametric methods. For a Rust library targeting maximum precision on undersampled data, ePSF construction and fitting is the ultimate technique.

### Accuracy comparison across methods

| Method | Typical accuracy | SNR for <0.1 px | Speed | Best for |
|--------|:---:|:---:|:---:|--------|
| Center of gravity | 0.1–0.3 px | >100 | Fastest | Quick-look, alignment |
| Windowed CoG | 0.05–0.15 px | >50 | Fast | Good all-around default |
| 1D Gaussian (marginal) | 0.05–0.1 px | >30 | Moderate | Well-sampled circular PSFs |
| 2D Gaussian fit | 0.02–0.05 px | >20 | Slow | Well-sampled data |
| 2D Moffat fit | 0.01–0.05 px | >20 | Slowest | Ground-based, highest parametric precision |
| ePSF fitting | 0.008–0.01 px | >10 | Moderate | Undersampled, crowded fields |

The **Cramér-Rao lower bound** for centroid precision (Winick 1986) is `σ_CRLB ≈ σ_spot / √N_photons` for a Gaussian spot in the photon-limited regime. Vakili & Hogg (2016) showed that PSF-convolved polynomial centroiding and maximum-likelihood fitting both saturate this bound, while center-of-gravity never does.

---

## Star validation: separating real stars from artifacts

After detection and initial centroiding, candidates must be filtered. The most effective criteria, drawn from DAOFIND, SExtractor, PixInsight, and Siril:

### Sharpness (DAOFIND)

```
sharpness = (central_pixel_convolved − mean_neighbors_convolved) / H_fit
```

Default bounds: **0.2 ≤ sharpness ≤ 1.0**. Values above 1.0 indicate cosmic rays or hot pixels (too sharp); below 0.2 indicates extended objects or noise (too diffuse). This single criterion is remarkably effective at rejecting hot pixels.

### Roundness (two measures)

**Gaussian-fit roundness (GROUND):** `round2 = 2(hx − hy)/(hx + hy)` from marginal Gaussian fit heights. Circular stars yield ~0; bounds: **−1.0 to 1.0**.

**Symmetry-based roundness (SROUND):** measures bilateral versus four-fold symmetry of the source. Satellite trails and diffraction spikes fail this test.

**SExtractor elongation:** `ELONGATION = A/B` from second-moment eigenvalues. Stars should have elongation **< 2.0**; eccentricity `e = √(1 − (B/A)²)`.

### Additional filters

- **FWHM range:** Reject candidates outside 1.0–20.0 pixels (cosmic rays are too small, galaxies too large)
- **Minimum area:** ≥ 5 connected pixels (the DETECT_MINAREA criterion eliminates single-pixel noise)
- **Saturation detection:** Reject stars with peak value > `SATUR_LEVEL` (or PixInsight's `upperLimit`)
- **Hot pixel filter:** Median filter pre-processing (3×3) kills isolated bright pixels. Alternatively, HFD < 2 pixels indicates hot pixel
- **Edge rejection:** Reject stars within FWHM of image boundary (flag 8 in SExtractor)
- **Peak response** (PixInsight): requires star profiles to have a sharp central peak; flat/saturated profiles are rejected if below the `peakResponse` threshold (default 0.75)
- **SNR minimum:** Require SNR ≥ 5–10 for reliable measurements

### SExtractor quality flags (bitmask)

```
0x01  Neighbors/bad pixels bias photometry
0x02  Originally blended with another object
0x04  At least one saturated pixel
0x08  Truncated at image boundary
0x10  Aperture data incomplete
0x20  Isophotal data incomplete
```

---

## Wavelet-based detection: the à trous starlet transform

PixInsight and several professional tools use wavelets for multi-scale structure detection. The **à trous ("with holes") algorithm** is an undecimated wavelet transform that produces coefficient arrays at the same resolution as the input.

### Mathematical definition

The B3 cubic spline produces a separable 5-tap low-pass filter:
```
h = [1/16, 1/4, 3/8, 1/4, 1/16]
```

The 2D kernel is the outer product h ⊗ hᵀ (a 5×5 matrix with central value 36/256). At each scale j, the kernel samples are spaced **2ʲ pixels apart** (the "holes"), effectively doubling the support without changing the kernel size:

```
c_{j+1}[k,l] = Σ_m Σ_n h[m] · h[n] · c_j[k + 2^j·m, l + 2^j·n]
w_{j+1}[k,l] = c_j[k,l] − c_{j+1}[k,l]
```

This yields J wavelet planes (w₁...w_J) plus a smooth residual c_J. **Perfect reconstruction** is trivial: `c₀ = c_J + Σ w_j`. Each scale isolates structures at characteristic size **2ʲ pixels**: w₁ captures ~1-pixel structures (noise, hot pixels), w₂ captures ~2-pixel structures, w₃ captures ~4-pixel structures, and so on.

### Thresholding in wavelet space

For each scale j, coefficients are significant if:
```
|w_j[k,l]| > K × σ_j
```
where K ≈ 3 (3-sigma detection) and σ_j is the noise level at scale j. For Gaussian noise, σ_j at each scale is a known fraction of the image noise σ₀:

- Scale 1: σ₁ ≈ 0.8908 × σ₀
- Scale 2: σ₂ ≈ 0.2007 × σ₀
- Scale 3: σ₃ ≈ 0.0856 × σ₀

These normalization constants decrease because each smoothing step reduces noise. The **multiresolution support** M_j is a binary mask of significant coefficients. Iterative refinement (recompute σ from non-significant pixels, rebuild support, repeat) improves accuracy to ~1%.

### Star detection via wavelets

Stars produce strong positive coefficients in scales matching their FWHM. The detection pipeline: (1) compute 3–5 wavelet scales, (2) threshold at K·σ_j per scale, (3) combine significant coefficients across relevant scales, (4) find local maxima, (5) apply morphological validation. The key advantage is **automatic multi-scale detection** — faint small stars and bright large stars are detected simultaneously without FWHM-dependent parameter tuning.

**Important caveat:** Bright stars "bleed" across all scales because they contain structure at every scale. The **Multiscale Median Transform** (nonlinear) better isolates structures per scale but is more computationally expensive.

### PixInsight's current StarDetector

PixInsight's latest StarDetector is **not wavelet-based for structure detection** (per PCL documentation), though it still uses wavelet-scale terminology for parameters. The current algorithm is scale-invariant and faster, with parameters including `structureLayers` (scale range), `sensitivity` (threshold, default 0.1), `peakResponse` (sharpness, default 0.75), `maxDistortion` (roundness, default 0.5), and optional Levenberg-Marquardt PSF fitting for centroid refinement.

---

## Shape analysis: FWHM, eccentricity, and second moments

### FWHM computation

From a **Gaussian fit**: `FWHM = 2√(2 ln 2) × σ = 2.3548 × σ`. For elliptical: `FWHM_x = 2.3548 × σ_x`, `FWHM_y = 2.3548 × σ_y`, geometric mean `FWHM = √(FWHM_x × FWHM_y)`.

From a **Moffat fit**: `FWHM = 2α√(2^(1/β) − 1)`.

From **second moments** (SExtractor): `FWHM = 2√(ln 2 × (A² + B²))` where A, B are semi-major/minor axes from eigenvalue decomposition of the intensity-weighted covariance matrix.

**Half-Flux Diameter (HFD):** Find radius r enclosing half the total flux. For a Gaussian, HFD = FWHM by definition. ASTAP uses HFD as its primary quality metric—hot pixels have HFD ≈ 1–2 pixels while real stars have HFD > 2–3 pixels.

### Second-order moments and ellipse parameters

SExtractor computes intensity-weighted second moments over above-threshold pixels:
```
x̄² = Σ Ii(xi − x̄)² / Σ Ii
ȳ² = Σ Ii(yi − ȳ)² / Σ Ii
x̄ȳ = Σ Ii(xi − x̄)(yi − ȳ) / Σ Ii
```

Eigenvalue decomposition yields:
```
A² = (x̄² + ȳ²)/2 + √[((x̄² − ȳ²)/2)² + x̄ȳ²]     semi-major axis²
B² = (x̄² + ȳ²)/2 − √[((x̄² − ȳ²)/2)² + x̄ȳ²]     semi-minor axis²
θ = ½ arctan(2x̄ȳ / (x̄² − ȳ²))                       position angle
```

Derived quantities: `ELONGATION = A/B`, `ELLIPTICITY = 1 − B/A`, `ECCENTRICITY = √(1 − B²/A²)`.

### SNR estimation

The CCD equation gives star SNR:
```
SNR = N_star / √(N_star + n_pix × (N_sky + N_dark + R²))
```
where N_star = star electrons, n_pix = aperture pixels, N_sky = sky e⁻/pixel, N_dark = dark e⁻/pixel, R = read noise in e⁻. In the bright-star (photon-limited) regime, `SNR ≈ √N_star`. The optimal aperture radius for maximum SNR in the background-limited regime is approximately **0.67 × FWHM**.

---

## Data structures for a Rust implementation

### The detected star record

```rust
pub struct DetectedStar {
    pub id: u32,
    // Sub-pixel position (f64 for maximum precision)
    pub x: f64,
    pub y: f64,
    // Photometry
    pub flux: f64,
    pub flux_err: f64,
    pub peak: f32,
    // Local background
    pub background: f32,
    pub background_rms: f32,
    // Shape (from PSF fit or moments)
    pub fwhm: f32,
    pub fwhm_x: f32,
    pub fwhm_y: f32,
    pub elongation: f32,       // A/B, ≥ 1.0
    pub ellipticity: f32,      // 1 − B/A
    pub theta: f32,            // position angle (radians)
    // Quality
    pub snr: f32,
    pub sharpness: f32,
    pub roundness1: f32,       // symmetry-based
    pub roundness2: f32,       // Gaussian-fit-based
    pub npix: u32,             // isophotal footprint pixels
    pub flags: u16,            // bitmask quality flags
    // Fit diagnostics (when PSF fitting used)
    pub chi2: f32,
    pub moffat_beta: f32,      // Moffat β (if Moffat fit)
}
```

### Background map

```rust
pub struct BackgroundMap {
    pub grid_bg: Vec<f32>,         // background at grid nodes
    pub grid_rms: Vec<f32>,        // RMS at grid nodes
    pub grid_nx: usize,
    pub grid_ny: usize,
    pub cell_size: usize,          // pixels per cell
    pub spline_coeffs: Vec<f64>,   // precomputed bicubic coefficients
    // Methods: background_at(x,y) -> f32, rms_at(x,y) -> f32
}
```

### Segmentation map

For typical astronomical images where sources cover <1% of pixels, **run-length encoding** saves significant memory:

```rust
// Dense representation (simple, cache-friendly)
pub struct SegmentationMap {
    pub labels: Vec<u32>,   // 0 = background, row-major
    pub width: usize,
    pub height: usize,
}
// RLE representation (memory-efficient for sparse sources)
pub struct Run { pub row: u32, pub col_start: u16, pub col_end: u16, pub label: u32 }
pub struct RleSegmentationMap { pub runs: Vec<Run> }
```

---

## Numerical precision and implementation guidance

### Floating-point strategy

Use **f64 for all fitting and accumulation** operations. Centroid computation, Levenberg-Marquardt iterations, Jacobian computation, and statistical accumulators all require f64 to avoid catastrophic cancellation. Raw pixel storage can remain f32 or u16. Final centroid results must be f64 to preserve sub-pixel precision. FWHM, shape diagnostics, and flags can be f32.

### Achieving <0.1 pixel centroid accuracy

Five techniques are essential: (1) **Use Moffat or ePSF fitting** rather than CoG—Gaussian fitting is second-best. (2) **Integrate the model over pixel area** when FWHM < 3 pixels rather than evaluating at pixel centers. (3) **Initialize LM fitting from CoG** to ensure convergence. (4) **Use f64 throughout the fitting** pipeline. (5) **Apply robust fitting** with sigma-clipped residuals to reject cosmic rays and bad pixels.

For the Levenberg-Marquardt solver, normalize parameters to order ~1 (divide positions by cutout size, amplitudes by peak value), use `diag(JᵀJ)` damping (not identity), and solve via **QR or SVD decomposition** rather than direct matrix inversion. Monitor the condition number—if `cond(JᵀJ) > 10¹⁰`, the fit is unreliable.

### Recommended Rust crates

The `levenberg-marquardt` crate (port of MINPACK, nalgebra-based) and `rmpfit` (pure Rust CMPFIT port) both provide production-quality LM solvers. The `varpro` crate implements Variable Projection for separable nonlinear least-squares—ideal for PSF models where amplitude and background are linear parameters. Use `nalgebra` for matrix operations, `rayon` for parallel tile processing, and `rustfft` for FFT-based convolution with larger kernels.

### Performance for large images

For images up to 60000×40000 pixels (~9.6 GB at f32): process background estimation in parallel tiles with `rayon`, store segmentation as RLE, and never materialize unnecessary full-resolution copies. Direct convolution beats FFT for small kernels (≤7×7). The à trous transform is highly parallelizable—each scale is independent separable convolutions. Use memory-mapped FITS I/O and process the detection pass in streaming fashion (row by row) following SExtractor's architecture.

---

## Conclusion: recommended architecture for maximum precision

The highest-precision pipeline for a Rust star detection library should combine the **SExtractor background mesh** (iterative sigma-clipped statistics, median-filtered grid, bicubic spline interpolation) with **matched-filter detection** (Gaussian kernel convolution at estimated PSF FWHM), **8-connected CCL with Union-Find**, and **two-stage centroiding**: fast windowed CoG for initial positions, followed by **2D Moffat fitting via Levenberg-Marquardt** for sub-pixel refinement. Validation should include DAOFIND-style sharpness and roundness, plus FWHM range, elongation, and saturation checks.

For multi-scale detection without FWHM assumptions, implement the **à trous starlet transform** with k-sigma thresholding per scale—this excels at simultaneously detecting stars across a wide size range. For undersampled data, invest in **ePSF construction** following Anderson & King (2000). The key insight across all state-of-the-art tools is that centroid accuracy depends far more on the fitting model (Moffat >> Gaussian >> CoG) and numerical care (f64, pixel integration, robust weighting) than on any single algorithmic trick. A well-implemented Moffat fitter with proper initialization will achieve <0.05 pixel accuracy on typical amateur astrophotography data, and <0.02 pixels on well-sampled high-SNR data.

### Key academic references

- **Bertin & Arnouts 1996** (A&AS 117, 393) — SExtractor: the complete background-mesh, detection, deblending, and measurement pipeline
- **Stetson 1987** (PASP 99, 191) — DAOPHOT/DAOFIND: zero-sum kernel detection, sharpness/roundness, empirical PSF fitting
- **Anderson & King 2000** — Effective PSF for undersampled images; pixel-phase bias correction achieving 0.01 px accuracy
- **Vakili & Hogg 2016** (arXiv:1610.05873) — Proves CoG never saturates the Cramér-Rao bound; polynomial centroiding near-optimal
- **Starck, Murtagh & Bijaoui** — Multiresolution Support and à trous wavelet transform for astronomical image analysis
- **Moffat 1969** — Original Moffat function for stellar profile modeling
- **Winick 1986** (JOSA A 3, 1809) — Cramér-Rao lower bound for CCD centroid estimation