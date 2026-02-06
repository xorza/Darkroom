# Star registration for high-precision astrophotography stacking

**Star registration—finding the geometric transform between two star lists—relies on a pipeline of invariant-based pattern matching, RANSAC-family robust estimation, and iterative least-squares refinement.** The best implementations (PixInsight, Astrometry.net) achieve sub-0.03-pixel accuracy by combining scale-invariant geometric hashes (triangles, quads, or higher polygons), k-d tree searches in invariant space, voting-based correspondence extraction, PROSAC/MSAC outlier rejection, and optional thin plate spline local correction. This document provides every mathematical formulation, algorithm, and data structure needed to build a production-quality Rust library rivaling state-of-the-art tools.

---

## 1. How pattern matching establishes star correspondences

The fundamental challenge: two star lists share no coordinate system. Stars differ by unknown translation, rotation, scale, and possibly distortion. Pattern matching algorithms solve this by forming small geometric figures (asterisms) from star subsets, computing scale/rotation-invariant descriptors, and searching for matching descriptors between lists.

### Triangle matching (Groth 1986)

The foundational algorithm. For each triplet of stars, form a triangle and characterize it by two invariants immune to similarity transforms.

**Vertex ordering convention.** Label vertices A, B, C such that side BC is the **longest**, AB is the **shortest**, and AC is intermediate. This canonical labeling eliminates the factor-of-6 ambiguity from vertex permutations.

**Triangle invariants (2D descriptor):**

```
x_t = BC / AB     (ratio of longest to shortest side, ≥ 1)
y_t = cos(∠B)     (cosine of angle at vertex connecting longest and shortest sides)
```

These two values fully characterize triangle shape up to translation, rotation, scaling, and mirror reflection.

**Triangle filtering** reduces combinatorial explosion. Reject triangles where the longest/shortest side ratio exceeds ~8–10 (elongated, high positional uncertainty) and where cos(∠B) > 0.99 (near-collinear vertices).

**Voting scheme.** For each triangle in list A, find geometrically similar triangles in list B (within tolerance in the 2D invariant space). Each matched triangle pair casts one vote for each of its three vertex-to-vertex correspondences. After processing all triangles, star pairs with the highest vote counts are accepted as true matches. An orientation check separates same-sense (n+) and opposite-sense (n−) matches to reject reflections.

**Complexity:** O(N³) for triangle construction, O(N^4.5) overall. This scales poorly—motivating the improved methods below.

### Astroalign algorithm (Beroiz et al. 2020)

Astroalign dramatically reduces complexity through nearest-neighbor triangulation and k-d tree search in invariant space.

**Star selection.** Take the **N = 50 brightest** stars (the `max_control_points` parameter). Build a 2D spatial k-d tree over their (x, y) positions.

**Nearest-neighbor triangulation.** For each star, find its **K = 4 nearest spatial neighbors** (5 including itself). From each group of 5, form all C(5,3) = **10 triangles**. This generates ~10N triangles total—orders of magnitude fewer than the brute-force N(N−1)(N−2)/6.

**The exact invariant map M.** For a triangle with sorted side lengths L₂ ≥ L₁ ≥ L₀:

```
M = (L₂/L₁,  L₁/L₀)
```

Both components are ≥ 1. Equilateral triangles map to (1, 1). The curve L₁/L₀ = 1/(L₂/L₁ − 1) corresponds to collinear vertices—a natural boundary of the valid invariant space.

**K-d tree matching.** Build a **second k-d tree** (2D, in invariant space) from all triangle descriptors of the reference image. For each target-image triangle invariant, query this tree for nearest neighbors within tolerance (~0.03–0.1 in invariant units). Matched triangles yield candidate vertex correspondences.

**Optimistic matching (RANSAC-like verification).** For each matched triangle pair, propose a similarity transform from the 3-point correspondence. Test whether this transform maps ≥80% of remaining matched triangles correctly (within **2 pixels**). Accept the first transform that passes; reject and try the next candidate otherwise. The final transform is re-estimated from all inliers.

**Overall complexity: O(N log N)**, dominated by k-d tree construction and queries.

### Quad matching (Astrometry.net, Lang et al. 2010)

Designed for blind astrometric solving across the entire sky, this algorithm uses 4-star figures ("quads") with a 4D geometric hash.

**Canonical ABCD labeling:**
1. Identify the **most widely separated pair** → stars A and B
2. Define a local coordinate system: A = origin, B = (1, 1)
3. Compute positions of remaining stars C and D in this coordinate frame
4. Distinguish C from D by a canonical rule (e.g., C closer to the AB midpoint)

**Geometric hash code** = (x_C, y_C, x_D, y_D), a point in continuous 4D space. This hash is **invariant to translation, rotation, and scaling** (but not mirroring—a deliberate design choice that improves discrimination).

**Why quads over triangles:** the 4D code space provides far better discrimination than 2D triangle invariants. Over the full 41,253 square degrees of sky, triangles produce too many near-misses. Quads achieve **zero false positives** in practice.

**Bayesian verification.** For each candidate quad match, compute a hypothesized WCS. Predict where other catalog stars should appear. Accept only if a **Bayesian log-odds ratio** against the null hypothesis (uniformly distributed stars) exceeds a threshold (~10⁹). This virtually eliminates false matches.

### Tabur 2007 optimistic pattern matching

Two algorithms (OPMA, OPMB) that achieve **near-constant-time matching** through search ordering by rarity.

**Key innovation:** instead of exhaustively processing all triangles then voting, assume a match is likely and test candidates in order of decreasing distinctiveness. Triangles formed by a close pair with a distant third star are highly distinctive and processed first. The algorithm exits as soon as one verified match is found.

**OPMA triangle space** differs from Groth's:
```
x_t = CB⃗ · CA⃗   (dot product, encodes both shape and perimeter)
y_t = a/c          (longest/shortest side ratio)
```

The dot-product metric separates triangles with identical side ratios but different sizes—a common source of false positives in Groth's space.

**Performance:** tested on 10,063 wide-field images, **100% match rate** with mean elapsed time of **6 ms** on modest hardware. The matching phase varies by <1 ms regardless of list size.

### ASTAP tetrahedron patterns

ASTAP uses 4-star quads with **5 distance ratios** as the hash code. For each star, find its 3 closest neighbors to form a quad. Compute all 6 inter-star distances, sort them d₁ ≥ d₂ ≥ ... ≥ d₆, and store the 5 ratios:

```
hash = (d₂/d₁, d₃/d₁, d₄/d₁, d₅/d₁, d₆/d₁)
```

These ratios are invariant to rotation, scaling, and flipping. Matching uses tolerance ~0.007 per ratio. The median scale ratio d₁_catalog/d₁_image across matching quads yields the plate scale. ASTAP selects the **500 brightest** stars and generates ~300 tetrahedron figures.

### PixInsight polygonal descriptors

PixInsight generalizes to N-star polygons (triangles through octagons). For an N-star polygon:

1. The two most distant stars become A and B, defining a local coordinate frame (A = origin, B = (1,1))
2. Remaining N−2 stars are encoded in this frame
3. **Hash dimensionality = 2(N−2):** quads → 4D, pentagons → 6D, hexagons → 8D, octagons → 12D

**Uncertainty reduction** is the key advantage. A polygon of N vertices constrains N−2 triangles simultaneously. An octagon has **1/6th** the matching uncertainty of a single triangle. Higher polygons produce dramatically fewer false matches—critical for achieving PixInsight's sub-0.03-pixel accuracy.

**Limitation:** polygon descriptors cannot handle mirrored images (unlike triangle similarity descriptors, which are mirror-invariant).

### How many stars and which triangulation method

| Software | Default N | Triangulation | Invariant dimensions |
|---|---|---|---|
| Astroalign | **50** | 4 nearest neighbors → 10 triangles/star | 2D |
| Groth 1986 | 40–100 | Brute-force all triplets | 2D |
| Tabur OPMA | 10 image, 50 reference | Brute-force, sorted by rarity | 2D |
| ASTAP | **500** | 3 nearest neighbors → 1 quad/star | 5D |
| Astrometry.net | ~100 | Brightest within HEALPix cells | 4D |
| PixInsight | Auto | Configurable, ~20 descriptors/star | 2(N−2)D |

Nearest-neighbor triangulation (Astroalign, ASTAP) generates O(N) asterisms vs O(N³) for brute-force—an essential optimization when N > ~50.

---

## 2. Transform models with complete mathematical formulations

### Similarity transform (4 DOF)

Models uniform scaling s, rotation θ, and translation (t_x, t_y). Appropriate for narrow-field imaging on a well-aligned equatorial mount.

**Matrix form** in homogeneous coordinates:

```
    ⎡ a  -b  t_x ⎤       where a = s·cos(θ)
H = ⎢ b   a  t_y ⎥             b = s·sin(θ)
    ⎣ 0   0   1  ⎦
```

**Minimal sample: 2 point pairs** (4 equations, 4 unknowns).

**Least-squares system.** For N correspondences {(xᵢ, yᵢ) → (x′ᵢ, y′ᵢ)}, stack:

```
⎡ x₁  -y₁  1  0 ⎤ ⎡ a  ⎤   ⎡ x′₁ ⎤
⎢ y₁   x₁  0  1 ⎥ ⎢ b  ⎥   ⎢ y′₁ ⎥
⎢ x₂  -y₂  1  0 ⎥ ⎢ t_x⎥ = ⎢ x′₂ ⎥
⎢ y₂   x₂  0  1 ⎥ ⎣ t_y⎦   ⎢ y′₂ ⎥
⎢ ⋮              ⎥           ⎢  ⋮   ⎥
⎣ yₙ   xₙ  0  1 ⎦           ⎣ y′ₙ ⎦
```

This 2N × 4 system **A·p = b** is solved via QR decomposition: p = R⁻¹·Qᵀ·b. Parameter recovery: s = √(a² + b²), θ = atan2(b, a).

**Alternative (Umeyama/SVD method):**
1. Compute centroids μ_src, μ_dst and center the points
2. Covariance H = (1/N) Σ x̃ᵢ·ỹᵢᵀ
3. SVD: H = U·D·Vᵀ
4. R = U·diag(1, sign(det(U)·det(V)))·Vᵀ
5. Scale c = tr(D·S) / σ²_src
6. Translation t = μ_dst − c·R·μ_src

### Affine transform (6 DOF)

Adds shear and anisotropic scaling. Appropriate for medium FOV (1°–5°) with differential atmospheric effects.

**Matrix form:**

```
    ⎡ a₁₁  a₁₂  t_x ⎤
H = ⎢ a₂₁  a₂₂  t_y ⎥
    ⎣  0    0    1   ⎦
```

**Minimal sample: 3 non-collinear point pairs.** The 6-parameter system decouples into two independent 3-parameter systems sharing the same design matrix:

```
⎡ x₁  y₁  1 ⎤ ⎡ a₁₁ ⎤   ⎡ x′₁ ⎤        ⎡ x₁  y₁  1 ⎤ ⎡ a₂₁ ⎤   ⎡ y′₁ ⎤
⎢ x₂  y₂  1 ⎥ ⎢ a₁₂ ⎥ = ⎢ x′₂ ⎥   and   ⎢ x₂  y₂  1 ⎥ ⎢ a₂₂ ⎥ = ⎢ y′₂ ⎥
⎢ ⋮         ⎥ ⎣ t_x  ⎦   ⎢  ⋮   ⎥        ⎢ ⋮         ⎥ ⎣ t_y  ⎦   ⎢  ⋮   ⎥
⎣ xₙ  yₙ  1 ⎦            ⎣ x′ₙ ⎦        ⎣ xₙ  yₙ  1 ⎦            ⎣ y′ₙ ⎦
```

One QR factorization of the N × 3 design matrix P, then two triangular solves. **QR is preferred over normal equations** because cond(PᵀP) = cond(P)²—normal equations square the condition number.

### Projective transform / homography (8 DOF)

Essential for wide-field imaging (>5°) where gnomonic projection introduces perspective-like distortion.

**Matrix form** (defined up to scale, hence 8 DOF):

```
    ⎡ h₁  h₂  h₃ ⎤
H = ⎢ h₄  h₅  h₆ ⎥
    ⎣ h₇  h₈  h₉ ⎦
```

**Mapping:** x′ = (h₁x + h₂y + h₃)/(h₇x + h₈y + h₉), y′ = (h₄x + h₅y + h₆)/(h₇x + h₈y + h₉).

**Each point pair yields 2 equations** (cross-multiplying to eliminate the denominator):

```
Row 1:  [-xᵢ  -yᵢ  -1   0    0    0   xᵢx′ᵢ  yᵢx′ᵢ  x′ᵢ]
Row 2:  [ 0    0    0  -xᵢ  -yᵢ  -1   xᵢy′ᵢ  yᵢy′ᵢ  y′ᵢ]
```

**The DLT algorithm** stacks these into a 2N × 9 matrix A. The homogeneous system A·h = 0 is solved via SVD: A = UΣVᵀ, and **h = last column of V** (the right singular vector corresponding to the smallest singular value). Reshape h into the 3 × 3 matrix H.

**Minimal sample: 4 point pairs** (8 equations for 8 DOF).

### Hartley normalization (critical for numerical stability)

Without normalization, pixel coordinates (~1000) and their products (~10⁶) create severe ill-conditioning. **Hartley normalization reduces the condition number of A from ~10⁸ to ~10².**

**Procedure for a set of points {(xᵢ, yᵢ)}:**
1. Translate so centroid is at origin: x̄ = mean(xᵢ), ȳ = mean(yᵢ)
2. Scale so mean distance from origin = √2: s = √2 / mean(√((xᵢ − x̄)² + (yᵢ − ȳ)²))

**Normalization matrix:**

```
    ⎡ s  0  -s·x̄ ⎤
T = ⎢ 0  s  -s·ȳ ⎥
    ⎣ 0  0    1   ⎦
```

Compute T for source points, T′ for destination points. Normalize both sets, run DLT on normalized points to get H̃, then **denormalize: H = T′⁻¹ · H̃ · T**.

### Nonlinear refinement via Levenberg-Marquardt

The DLT minimizes algebraic error, not geometric reprojection error. After obtaining H from DLT, minimize the **symmetric transfer error:**

```
E(H) = Σᵢ [ ‖x′ᵢ − H(xᵢ)‖² + ‖xᵢ − H⁻¹(x′ᵢ)‖² ]
```

Parameterize H by 8 free parameters (fix h₉ = 1). The Jacobian entries for projected point u = (h₁x + h₂y + h₃)/w, v = (h₄x + h₅y + h₆)/w where w = h₇x + h₈y + 1:

```
∂u/∂h₁ = x/w,  ∂u/∂h₂ = y/w,  ∂u/∂h₃ = 1/w,  ∂u/∂h₇ = −xu/w,  ∂u/∂h₈ = −yu/w
```

The LM update solves (JᵀJ + λ·diag(JᵀJ))·δ = −Jᵀr at each iteration, adjusting damping factor λ based on whether the cost decreases.

### Thin plate splines for local distortion correction

TPS models arbitrary smooth deformations using radial basis functions. Two independent TPS functions handle x′ and y′:

```
f(x, y) = a₀ + a₁x + a₂y + Σᵢ wᵢ · U(‖pᵢ − (x,y)‖)
```

**Radial basis function:** U(r) = r²·ln(r) in 2D (with U(0) = 0 by convention). This is the Green's function that minimizes the thin-plate bending energy:

```
J[f] = ∬ [(∂²f/∂x²)² + 2(∂²f/∂x∂y)² + (∂²f/∂y²)²] dx dy
```

**The linear system.** Given N control point correspondences:

**Kernel matrix K** (N × N): K_ij = U(‖pᵢ − pⱼ‖) = ‖pᵢ − pⱼ‖² · ln(‖pᵢ − pⱼ‖). Symmetric with K_ii = 0.

**Polynomial matrix P** (N × 3): row i = [1, xᵢ, yᵢ].

**Augmented system** (size (N+3) × (N+3)):

```
⎡ K    P ⎤ ⎡ w ⎤   ⎡ v ⎤
⎢       ⎥ ⎢   ⎥ = ⎢   ⎥
⎣ Pᵀ   0 ⎦ ⎣ a ⎦   ⎣ 0 ⎦
```

where w = [w₁,...,wₙ]ᵀ are kernel weights, a = [a₀, a₁, a₂]ᵀ are affine coefficients, v = target coordinate values, and the bottom rows enforce orthogonality: Σwᵢ = 0, Σwᵢxᵢ = 0, Σwᵢyᵢ = 0.

**Approximating (smoothing) splines** replace K with (K + λI):

- λ = 0: exact interpolation (risk of oscillation between noisy centroids)
- λ → ∞: pure affine fit
- Optimal λ balances fidelity with smoothness; typical range **0.01–0.1** for star registration

**PixInsight's surface simplifiers** reduce the N control points (e.g., from 6,986 Gaia stars to ~970), achieving **~86% reduction** while preserving accuracy. This addresses O(N³) complexity and ill-conditioning for large N. The iterative approach starts with a projective model, then refines with TPS through up to **20–100 distortion correction iterations**.

### When to use which model

| Transform | DOF | Min pairs | Best for | Typical FOV |
|---|---|---|---|---|
| Similarity | 4 | 2 | Rigid tracking, narrow field | < ~1° |
| Affine | 6 | 3 | Medium field, differential refraction | 1°–5° |
| Projective | 8 | 4 | Wide field, gnomonic distortion | 5°–30° |
| TPS | N+3/axis | ≥10 practical | Extreme distortion, mosaics, lens aberrations | Any |

**Rules of thumb:** if similarity residuals are < 0.1 px, use similarity. If residuals show spatially correlated structure after projective fit, add TPS. Wide-field (> 5°) almost always needs at least projective.

---

## 3. RANSAC and robust estimation in detail

After pattern matching produces putative correspondences (typically 70–90% correct), RANSAC-family algorithms reject the remaining outliers.

### Classic RANSAC algorithm

```
function RANSAC(correspondences, n, threshold, confidence):
    best_model ← null
    best_inliers ← ∅
    k ← initial_max_iterations

    for iteration = 1 to k:
        sample ← randomly select n correspondences
        if sample is degenerate: continue

        model ← fit_transform(sample)

        inliers ← {p : ‖dst_p − model(src_p)‖ < threshold}

        if |inliers| > |best_inliers|:
            best_model ← fit_transform(inliers)    // re-estimate with ALL inliers
            best_inliers ← inliers
            // Adaptive update:
            w ← |inliers| / |correspondences|
            k ← min(k, ⌈log(1 − confidence) / log(1 − wⁿ)⌉)

    return best_model, best_inliers
```

**Iteration count formula:** k = log(1 − p) / log(1 − wⁿ), where p = desired confidence (typically 0.99), w = inlier ratio, n = minimal sample size. Concrete values for p = 0.99:

| Inlier ratio w | n=2 (similarity) | n=3 (affine) | n=4 (homography) |
|---|---|---|---|
| 0.50 | 16 | 35 | 72 |
| 0.70 | 7 | 11 | 16 |
| 0.80 | 5 | 7 | 9 |
| 0.90 | 3 | 4 | 5 |

With typical star-matching inlier ratios >70%, **RANSAC converges in fewer than 20 iterations** for similarity/affine transforms.

**Inlier threshold:** **2–3 pixels** for typical star registration. Tighter (1.0–1.5 px) for high-precision astrometry. OpenCV's `findHomography` defaults to 3.0 px; its USAC framework defaults to 1.5 px.

### PROSAC (quality-sorted sampling)

PROSAC exploits the fact that brighter stars have more accurate centroids and higher-confidence matches. Stars sorted by descending brightness/SNR are sampled from progressively larger subsets.

```
function PROSAC(sorted_correspondences, n):
    pool_size ← n          // start with top-n correspondences only
    t ← 0

    while not terminated:
        t ← t + 1
        if t exceeds growth threshold for pool_size:
            pool_size ← pool_size + 1

        // Sample: mandatory newest point + (n-1) from previous pool
        sample ← {correspondence[pool_size]} ∪ random(pool[1..pool_size-1], n-1)
        model ← fit(sample)
        inliers ← evaluate_all(model)     // test against ALL correspondences
        update_best_and_termination(inliers)
```

**Speedup: up to 100× over uniform RANSAC** when quality ordering is reliable. For star registration, brightness-based ordering is a natural and effective quality metric.

### MSAC (better scoring)

Replaces RANSAC's binary inlier counting with truncated quadratic loss:

```
Score_RANSAC(e) = { 0  if e² < t²;  1  if e² ≥ t² }     // counts inliers
Score_MSAC(e)   = { e² if e² < t²;  t² if e² ≥ t² }     // penalizes large residuals
```

MSAC differentiates between "barely inlier" and "strongly inlier" points, producing more accurate models. **MSAC is the default scoring in OpenCV's USAC framework.**

### MAGSAC (threshold-free)

Marginalizes over all possible noise scales σ ∈ [0, σ_max], eliminating the need for a manual inlier threshold. Implemented as iteratively reweighted least squares (IRLS):

```
repeat:
    w_i ← marginalized_weight(residual_i, σ_max)
    model ← weighted_least_squares(correspondences, weights)
until convergence
```

**MAGSAC++ is recommended when noise characteristics are unknown**, as it requires the least tuning. OpenCV developers note it is "the only method whose optimal threshold is the same across all datasets."

### How Siril chains triangle matching into RANSAC

Siril's pipeline: detect up to 2000 stars → sort by brightness → triangle matching on **brightest ~20 stars** → pass candidate correspondences to OpenCV RANSAC. The OpenCV call by transform type:

| Transform | OpenCV function | Parameters |
|---|---|---|
| Similarity (4 DOF) | `estimateAffinePartial2D` | RANSAC, threshold=3.0, confidence=0.99 |
| Affine (6 DOF) | `estimateAffine2D` | RANSAC, threshold=3.0 |
| Homography (8 DOF) | `findHomography` | RANSAC, threshold=3.0, maxIters=2000, confidence=0.995 |

The final model is always **refined by Levenberg-Marquardt on inliers only** (done automatically inside OpenCV).

### Sigma-clipping refinement after RANSAC

```
function sigma_clip_refit(pairs, transform, k=2.5, max_iters=5):
    for iter = 1 to max_iters:
        residuals ← [‖dst_i − transform(src_i)‖ for each pair]
        μ ← median(residuals)
        σ ← MAD(residuals) × 1.4826       // robust dispersion estimate
        survivors ← {pair : |residual − μ| < k·σ}

        if |survivors| == |pairs|: break   // converged
        pairs ← survivors
        transform ← weighted_least_squares(pairs)

    return transform, pairs
```

**Typical k = 2.5–3.0 sigma.** The MAD (Median Absolute Deviation) multiplied by 1.4826 provides a robust estimate of σ that is not corrupted by remaining outliers. Convergence typically occurs within **2–3 iterations**.

### Early termination strategies

**Adaptive iteration count** (standard): after each new best model, update the estimated inlier ratio w and recompute k. This is the single most important optimization—it often cuts iterations by 5–10×.

**SPRT (Sequential Probability Ratio Test):** instead of evaluating all N points per model, evaluate sequentially and reject bad models after checking just a few points. Reports **2–10× speedup** over standard consensus evaluation. All OpenCV USAC methods use SPRT.

**Degenerate sample detection:** check for collinear points (for affine/homography). In star registration, degeneracies are rare since stars are typically well-distributed, but the check costs negligible time.

---

## 4. Achieving sub-pixel precision

### PSF-fitted centroids provide the foundation

Simple intensity-weighted centroids achieve ~0.3–0.5 pixel accuracy. **Gaussian PSF fitting achieves 0.01–0.1 pixel accuracy** by fitting I(x,y) = A·exp(−((x−x_c)² + (y−y_c)²)/(2σ²)) + B to the pixel data around each star. This is the starting point—all downstream accuracy depends on centroid quality.

### Weighted least squares by star brightness

After RANSAC identifies inliers, refine the transform using weights proportional to star flux or SNR:

```
minimize Σ wᵢ · ‖T(srcᵢ) − dstᵢ‖²    where wᵢ = fluxᵢ / max_flux
```

Brighter stars have lower photon noise in their centroids, so weighting them more strongly improves the final transform accuracy.

### Quality metrics for the final result

**RMS reprojection error** = √(Σ ‖dstᵢ − T(srcᵢ)‖² / N). For well-registered deep-sky images, expect **0.1–0.5 pixels** with a global model and **< 0.03 pixels** with TPS correction.

**Condition number** of the estimation matrix (max/min singular value ratio from SVD). Without Hartley normalization: ~10⁸. With normalization: ~10¹–10³. Monitor this; high condition numbers signal numerical instability.

**Inlier ratio** should be > 70% for a credible registration. Below 40% suggests a matching failure.

### How PixInsight reaches < 0.03 pixel accuracy

PixInsight's documented approach combines several techniques: **PSF-fitted star detection** with configurable tolerance, **polygon descriptors** (quads through octagons) for high-confidence matching with reduced uncertainty, **iterative distortion correction** through up to 100 predictor-corrector TPS cycles, **surface simplification** that reduces control point count by ~86% while preserving accuracy, and **approximating splines** with tuned smoothness to prevent overfitting. The result: "we have reduced uncertainty in coordinate evaluation to less than **0.03 pixels**" at 3.658 arcsec/pixel.

---

## 5. The complete registration pipeline

### Stage 1: Star selection and preprocessing

Sort detected stars by flux (descending). Select the top **N = 50–200** brightest. Too few stars risks matching failure; too many increases computation without improving accuracy. **N = 100 is a robust default.** Reject stars too close to image edges (within ~10 pixels) or with FWHM outside 1–3× the median FWHM (likely hot pixels or galaxies).

### Stage 2: Asterism generation and invariant computation

Build a 2D spatial k-d tree over star positions. For each star, find its K = 4 nearest neighbors. Form all C(K+1, 3) = 10 triangles per star (or C(K+1, 4) quads for higher-discrimination matching). Deduplicate by sorting star IDs within each asterism and inserting into a HashSet.

For each triangle, sort side lengths L₂ ≥ L₁ ≥ L₀ and compute invariant **(L₂/L₁, L₁/L₀)**. For quads, compute the 4D hash (x_C, y_C, x_D, y_D) in the canonical A-B coordinate frame.

### Stage 3: Pattern matching via k-d tree queries

Build an **invariant-space k-d tree** from the reference image's descriptors. For each target image descriptor, query for nearest neighbors within tolerance (**0.005–0.01** in invariant units for triangles; **0.01–0.02** for quads). The k-d tree provides O(log N) per query—essential for performance.

### Stage 4: Voting and correspondence extraction

For each matched asterism pair, cast one vote for each vertex correspondence. Accumulate in a HashMap<(src_id, tgt_id), vote_count>. **Filter: require ≥ 3 votes** per star pair. This eliminates most false matches.

Map vertex correspondences correctly using side-length ordering: the vertex opposite the longest side in triangle A corresponds to the vertex opposite the longest side in triangle B, and so forth.

### Stage 5: RANSAC robust estimation

Use PROSAC if stars are brightness-sorted (strongly recommended). Set minimal sample size by transform type (2/3/4 for similarity/affine/homography). Threshold: **2.0 pixels** for standard registration, **1.0 pixels** for high-precision work. Confidence: **0.99**. Apply adaptive iteration count.

For star registration with typically high inlier ratios (>70%), RANSAC converges rapidly. The main purpose is to clean up the few remaining false matches from the voting stage.

### Stage 6: Refinement

Re-estimate the transform from all RANSAC inliers using **weighted least squares** (weight by flux). Apply **sigma-clipping at 2.5σ** using MAD-based robust dispersion. Iterate 3–5 times. Optionally apply Levenberg-Marquardt nonlinear refinement for homography models.

### Stage 7: Optional local distortion correction (TPS)

If residuals after the global model show spatially correlated patterns (common for wide-field or lens-distorted images), fit a TPS using inlier correspondences as control points. Use **approximating splines** with λ = 0.01–0.1 to prevent overfitting. For large control point sets (>500), apply surface simplification to reduce to ~100–200 representative points.

### Fallback strategy

If matching fails at any stage (too few votes, RANSAC inlier ratio < 30%), try: (1) increase N from 100 to 200, (2) relax invariant tolerance by 2×, (3) fall back to a simpler transform model, (4) try brute-force triangle generation instead of nearest-neighbor. Report failure with diagnostics if all fallbacks fail.

---

## 6. Rust implementation architecture

### Core data structures

```rust
use nalgebra::{Matrix3, Vector2, DMatrix, DVector};

/// A detected star with PSF-fitted centroid and photometric properties.
#[derive(Debug, Clone, Copy)]
pub struct Star {
    pub id: u32,
    pub x: f64,
    pub y: f64,
    pub flux: f64,
    pub snr: f64,
    pub fwhm: f64,
}

/// Triangle descriptor with scale-invariant hash.
#[derive(Debug, Clone)]
pub struct TriangleDescriptor {
    pub star_ids: [u32; 3],          // sorted by role, not ID
    pub invariant: [f64; 2],         // (L₂/L₁, L₁/L₀)
    pub side_lengths: [f64; 3],      // L₂ ≥ L₁ ≥ L₀
}

/// Quad descriptor (Astrometry.net style).
#[derive(Debug, Clone)]
pub struct QuadDescriptor {
    pub star_ids: [u32; 4],          // A, B, C, D in canonical order
    pub hash_code: [f64; 4],         // (x_C, y_C, x_D, y_D)
}

/// A confirmed star-to-star correspondence.
#[derive(Debug, Clone, Copy)]
pub struct StarMatch {
    pub src_id: u32,
    pub dst_id: u32,
    pub src_pos: Vector2<f64>,
    pub dst_pos: Vector2<f64>,
    pub votes: u32,
    pub residual: f64,
}

/// Geometric transform model variants.
#[derive(Debug, Clone)]
pub enum TransformModel {
    Similarity(Matrix3<f64>),
    Affine(Matrix3<f64>),
    Projective(Matrix3<f64>),
    ThinPlateSpline {
        control_points: Vec<Vector2<f64>>,
        weights_x: DVector<f64>,
        weights_y: DVector<f64>,
        affine_x: [f64; 3],  // [a₀, a₁, a₂]
        affine_y: [f64; 3],
    },
}

/// Complete registration result with quality diagnostics.
#[derive(Debug, Clone)]
pub struct RegistrationResult {
    pub transform: TransformModel,
    pub inliers: Vec<StarMatch>,
    pub outliers: Vec<StarMatch>,
    pub rms_error: f64,
    pub max_residual: f64,
    pub inlier_ratio: f64,
    pub condition_number: f64,
    pub iterations: usize,
}
```

### The `GeometricTransform` trait

```rust
pub trait GeometricTransform: Send + Sync {
    /// Apply transform to a point.
    fn apply(&self, p: &Vector2<f64>) -> Vector2<f64>;
    /// Minimum point pairs for estimation.
    fn min_samples(&self) -> usize;
    /// Estimate from N ≥ min_samples correspondences.
    fn estimate(src: &[Vector2<f64>], dst: &[Vector2<f64>]) -> Result<Self>
        where Self: Sized;
    /// Compute per-point reprojection errors.
    fn residuals(&self, src: &[Vector2<f64>], dst: &[Vector2<f64>]) -> Vec<f64>;
}
```

### Recommended crate dependencies

```toml
[dependencies]
nalgebra = "0.33"          # Matrix ops, SVD, QR — the backbone
kiddo = "5.2"              # High-performance k-d tree (2D/4D, f64, ImmutableKdTree)
levenberg-marquardt = "0.15" # Nonlinear refinement (MINPACK port on nalgebra)
rand = "0.8"               # RANSAC sampling (use SmallRng for speed)
rayon = "1.10"             # Data parallelism via .par_iter()
thiserror = "2"            # Error types
```

**kiddo** is the recommended k-d tree: 3.4M downloads, supports `ImmutableKdTree<f64, 2>` for 2D spatial queries and `ImmutableKdTree<f64, 4>` for 4D quad hash queries, with `SquaredEuclidean` distance and `within` range queries. Alternative: `kd-tree` crate with nalgebra integration and rayon parallel build.

For RANSAC, the **arrsac** crate (from rust-cv) provides adaptive RANSAC with the `sample-consensus` trait framework, but implementing RANSAC from scratch (~100 lines) gives full control over PROSAC sampling and MSAC scoring.

**Use f64 everywhere.** f32 is categorically insufficient for sub-pixel astrometric accuracy—condition numbers regularly exceed 10⁴ even with normalization.

### TPS kernel implementation

```rust
/// TPS radial basis function: U(r) = r²·ln(r), U(0) = 0
#[inline]
fn tps_kernel(r: f64) -> f64 {
    if r > 0.0 { r * r * r.ln() } else { 0.0 }
}

/// Build and solve the TPS system for one coordinate axis.
fn solve_tps_axis(
    ctrl: &[Vector2<f64>],
    target: &[f64],
    lambda: f64,
) -> (DVector<f64>, [f64; 3]) {
    let n = ctrl.len();
    let m = n + 3;
    let mut sys = DMatrix::<f64>::zeros(m, m);

    // Fill K + λI
    for i in 0..n {
        for j in 0..n {
            if i != j {
                let r = (ctrl[i] - ctrl[j]).norm();
                sys[(i, j)] = tps_kernel(r);
            }
        }
        sys[(i, i)] += lambda;
    }
    // Fill P and Pᵀ
    for i in 0..n {
        sys[(i, n)] = 1.0;     sys[(n, i)] = 1.0;
        sys[(i, n+1)] = ctrl[i].x; sys[(n+1, i)] = ctrl[i].x;
        sys[(i, n+2)] = ctrl[i].y; sys[(n+2, i)] = ctrl[i].y;
    }

    let mut rhs = DVector::<f64>::zeros(m);
    for i in 0..n { rhs[i] = target[i]; }

    let solution = sys.lu().solve(&rhs).expect("TPS system singular");
    let weights = solution.rows(0, n).into_owned();
    let affine = [solution[n], solution[n+1], solution[n+2]];
    (weights, affine)
}
```

### Parallelization opportunities with rayon

```rust
use rayon::prelude::*;

// Triangle generation: embarrassingly parallel
let descriptors: Vec<TriangleDescriptor> = stars.par_iter()
    .flat_map(|star| generate_neighbor_triangles(star, &spatial_kdtree, k))
    .collect();

// Invariant-space matching: parallel batch queries
let matches: Vec<_> = target_descriptors.par_iter()
    .filter_map(|desc| {
        let nearest = ref_kdtree.nearest_one::<SquaredEuclidean>(&desc.invariant);
        (nearest.distance < tolerance_sq).then(|| (desc, nearest))
    })
    .collect();

// RANSAC: parallel independent trials (aggregate best afterward)
let best = (0..max_iterations).into_par_iter()
    .map(|_| {
        let mut rng = SmallRng::from_entropy();
        let sample = draw_sample(&candidates, n, &mut rng);
        let model = fit(&sample);
        let inlier_count = count_inliers(&model, &candidates, threshold);
        (model, inlier_count)
    })
    .max_by_key(|(_, count)| *count);
```

**Note on parallel RANSAC:** parallel trials are non-deterministic (thread scheduling affects which model wins ties). For reproducibility, use sequential RANSAC with a fixed seed, or run parallel trials and deterministically select the best.

---

## 7. Key references and their contributions

| Reference | Contribution | Key technique |
|---|---|---|
| **Groth 1986** (AJ 91, 1244) | Foundational triangle matching | Side-ratio invariants + voting scheme |
| **Tabur 2007** (PASA 24, 189) | Optimistic pattern matching | Rarity-ordered search, ~O(1) matching phase |
| **Lang et al. 2010** (AJ 139, 1782) | Astrometry.net blind solving | 4-star quad hash + Bayesian verification |
| **Beroiz et al. 2020** (A&C 32, 100384) | Astroalign | NN triangulation + k-d tree invariant matching |
| **Fischler & Bolles 1981** (CACM 24, 381) | RANSAC | Random sample consensus for robust estimation |
| **Chum & Matas 2005** (CVPR) | PROSAC | Quality-sorted progressive sampling |
| **Hartley 1997** (IEEE TPAMI) | Point normalization | Mean distance = √2 normalization for DLT |
| **Hartley & Zisserman 2004** | Multiple View Geometry | DLT, cost functions, RANSAC for homography |
| **Bookstein 1989** | Thin plate splines | r²·ln(r) radial basis + bending energy minimization |

---

## Conclusion

The optimal pipeline for a high-precision Rust star registration library combines **Astroalign-style nearest-neighbor triangulation** (for O(N log N) efficiency) with **quad descriptors** (for stronger discrimination in crowded fields), **PROSAC with MSAC scoring** (exploiting brightness-ordered star quality for fast convergence), and a **progressive model hierarchy** that starts with similarity and upgrades through affine/projective/TPS as residual structure demands.

Three architectural decisions will most impact final quality. First, always apply **Hartley normalization** before any DLT—this single step can improve condition numbers by six orders of magnitude. Second, use **approximating TPS** (λ > 0) rather than interpolating splines for the local correction stage—PixInsight's experience demonstrates that surface simplification with smoothing is essential to prevent centroid noise from propagating into inter-star oscillations. Third, implement the full **RANSAC → weighted-least-squares → sigma-clip → refit** loop rather than stopping after RANSAC; the iterative refinement consistently reduces RMS error by 30–50% over the raw RANSAC output.

The algorithm-level choice that most distinguishes industry-best results from adequate ones is the asterism descriptor. Triangle invariants in 2D are sufficient for most stacking workflows, but **polygon descriptors in 4D+ invariant spaces** (quads or higher) dramatically reduce false match rates and enable reliable operation with fewer bright stars, larger field rotations, and more crowded fields. A library offering both triangle and quad modes—with automatic fallback—will cover the widest range of real-world conditions.