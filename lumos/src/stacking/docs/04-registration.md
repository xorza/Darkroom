# Stage 4 — Registration: Best Practices & Algorithms

## Scope & Goal

Registration is the step that brings every frame of a multi-frame exposure into a
single, shared pixel grid so the frames can be stacked, drizzled, or differenced.
Unlike generic computer-vision registration, the astrophotography problem has a
peculiar structure that dictates the whole algorithm:

- **The features are point sources with no internal structure.** Stars are
  near-identical blobs; corner/SIFT-style descriptors do not work because two
  stars are locally indistinguishable. Astroalign's docstring states this plainly:
  "stars have very little stable structure and so, in general, indistinguishable
  from each other" (`/.tmp/refs/astroalign/astroalign.py:32`). The *only* stable,
  matchable signal is the **relative geometric configuration** of the stars — the
  asterism — which is why triangle/quad matching is the universal approach.
- **The transform between frames is geometrically simple.** For a tracked mount on
  the same optical train, consecutive frames differ by a small translation plus a
  tiny field rotation; across a session, plate scale is fixed. The full perspective
  / lens-distortion machinery is only needed for wide-field lenses, mosaics, or
  cross-instrument work.
- **Sub-pixel accuracy is the entire point.** The final stacked resolution is
  limited by the registration RMS. A registration RMS of 0.3 px already softens a
  2 px-FWHM star; an RMS approaching the FWHM destroys the resolution gain of
  stacking. Everything downstream (centroiding precision, robust fitting, choice of
  resampling kernel) exists to push that RMS below a fraction of a pixel.

The pipeline therefore decomposes into: (1) **asterism matching** to obtain a
coarse set of point correspondences without knowing the transform; (2) **robust
transform estimation** to fit a geometric model and reject the inevitable spurious
matches; (3) **match recovery/refinement** to grow and tighten the inlier set;
(4) optional **distortion modeling** for the residual non-linear field warp; and
(5) **warping/resampling** into the common grid. This document treats each in
turn, separating best practice from anti-patterns, and ends with how lumos
implements it (`src/registration/`) plus concrete gaps.

---

## 1. Feature / asterism matching

### 1.1 The invariant-triangle idea (Groth 1986)

The foundational method is Groth's 1986 pattern-matching algorithm for 2-D
coordinate lists (Groth, *AJ* 91, 1244, [ADS 1986AJ.....91.1244G](https://ui.adsabs.harvard.edu/abs/1986AJ.....91.1244G/abstract)).
Form triangles from triplets of stars; compute features of each triangle that are
**invariant under translation, rotation, uniform scaling, and reflection**; match
triangles between the two lists by comparing those invariants; then each matched
triangle votes for three point correspondences. The invariance properties are
exactly what we need because a tracked-mount frame pair differs by precisely
translation + rotation + (fixed) scale.

**Groth's exact feature pair (pass 2, parsed from the PDF).** Groth does *not* use
two side ratios; he uses **`(R, C)`** where `R = r3/r2` is the ratio of the longest to
the shortest side (`groth1986.txt:128-139`) and `C` is the **cosine of the angle at
vertex 1** (the vertex opposite the longest side, `groth1986.txt:149-150`,
`C = (Δx3·Δx2 + Δy3·Δy2)/r3r2`). He also carries each invariant's *tolerance*, derived
by propagating a per-coordinate matching error `ε` through `R` and `C` (his Eqs. 2–6).
Crucially the cosine, unlike a pure side ratio, can encode the **sense of vertex
traversal** if measured as a signed angle — Groth instead stores the CW/CCW
orientation separately so the algorithm is "independent of coordinate inversion"
(`groth1986.txt:95-102`). Two triangles match when `(R_A−R_B)² < t_RA + t_RB` and
`(C_A−C_B)² < t_CA + t_CB` (Eqs. 7–8), found efficiently by a sort-merge sweep over
the ratio-sorted lists. The two-side-ratio parameterization (astroalign, lumos) is an
algebraically equivalent shape descriptor — any two independent length ratios fully
characterize a triangle up to similarity (astroalign states this explicitly,
`astroalign_beroiz2020.txt`: "knowing two independent ratios of the side lengths is
also sufficient … any function of two independent length ratios will suffice") — and
trades Groth's `(R, cosine)` for a more uniform 2-D code space.

**False-match probability (pass 2).** Groth derives the expected number of *spurious*
triangle matches analytically (`groth1986.txt:230-260`): with `n` points per list
there are `n_t = n(n−1)(n−2)/6` triangles and `12 n_t²` potential matches (6 vertex
permutations × 2 orientations). A false match occurs when the transformed third
vertex lands within a `2ε`-square of its counterpart — probability `4ε²` — so the
expected count is `≈ 48 n_t² ε²`, written `n_f = 4f² n_t² ε²` with an empirical
`f ∈ [2,3]` absorbing edge/tolerance effects. The decisive consequence: **as `n`
grows, `ε` must shrink as `n^{−3/2}`** to keep true matches from being swamped by
false ones — the formal justification for both keeping the matching tolerance tight
(§1.5) and capping the star count (brightest-N).

A triangle has three side lengths `s0 ≤ s1 ≤ s2`. Two **ratios** of side lengths
fully describe its shape up to similarity:

- Astroalign uses `(s2/s1, s1/s0)` — `_invariantfeatures()` returns
  `[sides[2]/sides[1], sides[1]/sides[0]]` (`/.tmp/refs/astroalign/astroalign.py:106-115`).
- lumos uses `(s0/s2, s1/s2)` — `Triangle::from_positions` computes
  `ratios = (sides[0]/longest, sides[1]/longest)` (`src/registration/triangle/geometry.rs:75`).

These are algebraically equivalent parameterizations of the same 2-D shape space;
both are dimensionless and hence scale-invariant, and both are unchanged by
rotation/translation since they are built only from distances.

**Critical filter — reject elongated triangles.** Groth observed that triangles
with a large longest/shortest side ratio `R` have unstable invariants: a tiny
perturbation of the short side produces a large ratio change, so such triangles
falsely match many others. The standard fix is to discard triangles with `R` above
~8–10. lumos applies exactly this: `if longest / sides[0] > 10.0 { return None }`
with the comment citing "Groth 1986, R=10 threshold"
(`src/registration/triangle/geometry.rs:71`). It additionally rejects near-collinear
triangles via Heron's-formula area (`MIN_TRIANGLE_AREA_SQ`, line 81) and degenerate
cross-products (line 102). This matches the web-verified Groth practice ("triangles
whose length ratio is above a set number are removed, often R=10 or R=8").

**Reflection / orientation.** A triangle's two side ratios are identical for a
mirror image, so ratios alone cannot tell a frame from its reflection. To
distinguish (or to allow) mirroring, store the signed orientation (CW/CCW) from the
cross product. lumos stores `Orientation` (`geometry.rs:11`) and optionally enforces
it during voting (`check_orientation`, `voting.rs:175`); disabling the check allows
mirrored images (e.g. a meridian flip or a diagonal mirror in the optical path).

### 1.2 Limiting the combinatorics: k-nearest-neighbor triangles

The number of triangles from `n` stars is `C(n,3) = O(n³)`, which is prohibitive
for hundreds of stars. The universal trick is to build triangles only from each
star and its **k nearest neighbors**, giving `O(n·C(k,2)) = O(n·k²)` triangles:

- Astroalign: `NUM_NEAREST_NEIGHBORS = 5`, builds all `C(5,3)=10` triangles per
  star via a `scipy.spatial.KDTree` query, then de-duplicates
  (`astroalign.py:97-190`).
- lumos: `form_triangles_from_neighbors` queries a k-d tree for `k+1` neighbors
  (the `+1` because the point itself is returned), forms all neighbor pairs, sorts
  and dedups (`src/registration/triangle/matching.rs:102-138`). `k` adapts:
  `(n.min/3).clamp(5,10)` (`matching.rs:63`), so `C(10,2)=45` triangles/star at the
  top end. The code comment correctly notes astroalign uses k=5 and that higher k
  grows triangle count quadratically with diminishing returns.

The kNN restriction is also a *robustness* feature: a spurious detection (hot
pixel, cosmic ray, satellite glint) only contaminates the triangles of its local
neighborhood, not the whole set, so the global vote still converges.

### 1.3 Matching invariants: range search + voting

With triangles reduced to 2-D invariant points, matching is a nearest-neighbor
problem in invariant space:

- Astroalign builds a KDTree of source invariants and a KDTree of target
  invariants, then `query_ball_tree(r=0.1)` to find all pairs within radius 0.1 in
  ratio space (`astroalign.py:343-356`). The comment notes `r=0.1` is empirical,
  "returns about the same number of matches as inputs."
- lumos builds a k-d tree on the *reference* triangle invariants
  (`build_invariant_tree`, `voting.rs:129`) and, for each target triangle, does a
  radius query and then an exact per-axis (L∞) tolerance check
  (`is_similar`, tolerance `ratio_tolerance`, default 0.01 = 1%). Note the careful
  detail at `voting.rs:160`: the k-d tree uses L2 distance but the similarity test
  is L∞ (per-axis max), so the search radius is inflated by `√2` to circumscribe
  the L∞ square — otherwise corner candidates would be missed.

Each surviving triangle pair casts **votes**: because both triangles have their
vertices ordered by opposite-side length, vertex *i* of the reference triangle
corresponds to vertex *i* of the target triangle, so the pair votes for three
specific point correspondences (`voting.rs:181-185`). A correct correspondence
accumulates votes from *many* shared triangles; a coincidental ratio match scores
one or two. The vote matrix is dense (`Vec<u16>`) for small problems and a sparse
`HashMap` above 250 K cells (`voting.rs:22`). Final correspondences are extracted
greedily: filter by `min_votes` (default 3), sort by vote count descending, and
assign each ref/target point at most once (`resolve_matches`, `voting.rs:196`).
The `min_votes ≥ 3` threshold is what separates real matches from noise — a single
matching triangle is never trusted.

This **voting/accumulator** scheme is the key robustness amplifier: it converts a
soft per-triangle similarity into a hard per-point consensus, so a handful of
spurious stars cannot manufacture a correspondence.

**Groth's full vote pipeline (pass 2).** The voting idea is Groth's, and his version
adds two filters lumos folds into RANSAC instead. (1) **Log-perimeter
(log-magnification) pruning** before voting: each matched triangle implies a
magnification `log M = log p_A − log p_B` between the lists; *true* matches all share
essentially the same `M` while false matches scatter, so Groth iteratively discards
matches whose `log M` is more than `f·σ` from the mean (`f` set to 1/2/3 by the ratio
of estimated true-to-false matches `m_t/m_f`), and discards the minority orientation
class (`groth1986.txt:260-300`). (2) **Vote termination by factor-of-2 drop:** after
sorting the vote array, assignments proceed until "the vote drops by a factor of 2, an
attempt is made to assign an already-assigned point, or the vote drops to zero"
(`groth1986.txt:300-330`). lumos's `min_votes ≥ 3` cutoff and greedy one-to-one
resolution is a simpler stand-in for this gap-detection rule. (3) **Spurious-assignment
guard:** Groth re-runs the *entire* matcher restricted to the points matched in the
first pass; if fewer survive, the original matches were false and the lists are
declared unmatchable — the conceptual ancestor of RANSAC's "does the model explain the
rest of the data?" verification.

### 1.4 Quad hashing (astrometry.net) — the blind-solve alternative

When there is *no* prior on scale or position (blind plate solving against a sky
catalog), 4-star **quads** are preferred over triangles because a 4-point code is a
4-D descriptor with far higher discriminating power, enabling a pre-built
hash index. astrometry.net (Lang et al. 2010, parsed pass 2) computes a geometric
hash: pick the two most-widely-separated stars A, B of a quad; define a local
coordinate frame where A=(0,0), B=(1,1); express the remaining stars C, D in that
frame; the 4 numbers `(xC,yC,xD,yD)` are the **code**, invariant to
translation/rotation/scale (`astrometry_lang2010.txt:258-280`, Fig. 1;
`.tmp/refs/astrometry.net/solver/codefile.c:49-65`: `codefile_compute_field_code`
builds `scale=|AB|²`, `costheta=(ABy+ABx)/scale`, `sintheta=(ABy−ABx)/scale` and
rotates C, D into the AB frame via `x = Cx·cosθ + Cy·sinθ`, `y = −Cx·sinθ + Cy·cosθ`).

**Why quads, not triangles (pass 2).** The paper is explicit that the *standard*
geometric-hashing recipe would use triangles, but "the positional noise level in
typical astronomical images is sufficiently high that triangles are not distinctive
enough" — a quad "can be thought of as two triangles that share a common edge," so a
quad "nearly squares the distinctiveness" and each code occupies a much smaller
fraction of the 4-D code space, yielding far fewer coincidental matches
(`astrometry_lang2010.txt:300-330`). This is the opposite tradeoff from the
frame-to-frame case: blind solving has a *huge* hypothesis space and needs maximum
discrimination per feature; frame-to-frame matching has a near-identity prior, so the
cheaper triangle + vote suffices.

**Symmetry breaking and the circle (pass 2).** The code has two symmetries — swapping
A↔B sends `(xC,yC,xD,yD)→(1−xC,1−yC,1−xD,1−yD)`, swapping C↔D sends it to
`(xD,yD,xC,yC)`. astrometry.net breaks both by *requiring* `xC ≤ xD` **and**
`xC + xD ≤ 1`, and requires C, D to lie inside the circle with AB as diameter, so each
physical quad maps to exactly one code (`astrometry_lang2010.txt:268-275`). Matching
is a kd-tree range query in 4-D code space, and every candidate is **verified** by a
Bayesian decision test.

**The Bayesian verification, quantified (pass 2).** The verify step chooses between a
foreground model `F` (alignment true) and background `B` (false) via the Bayes factor
`K = p(D|F)/p(D|B)` (`astrometry_lang2010.txt:460-475`). It accepts iff
`K > [p(B)/p(F)]·[u(TN)−u(FP)]/[u(TP)−u(FN)]`. astrometry.net uses a deliberately
conservative prior `p(F)/p(B) = 10⁻⁶` (because it examines many false alignments before
the first true one) and a strongly asymmetric utility table `u(TP)=+1, u(FN)=−1,
u(TN)=+1, u(FP)=−1999` — a false positive is treated as ~2000× worse than any other
outcome, so the system "would much rather fail to produce a result rather than produce
a false result." Operationally it asks "if this alignment were correct, where else
would we expect stars?" and scores the implied WCS against the *other* field stars,
accounting for a distractor ratio and per-star positional noise `verify_pix`
(`solver/solver.c`, `verify.c`). The verify step is essential: a single quad code match
is just a hypothesis; only the full-field agreement confirms it. (Interestingly the
paper notes they found it *faster* to verify every hypothesis immediately than to
accumulate votes across codes — the opposite of Groth's vote-then-fit, justified by
the cheap per-hypothesis WCS and the huge candidate count.)

**When to use which.** Triangles (lumos, astroalign) are ideal for the
frame-to-frame case where the two star lists are drawn from nearly the same field
with a near-identity transform — fast, no index, robust. Quad hashing is for blind
absolute astrometry against a catalog. The verification idea — *score a hypothesis
by how many other points it explains* — is universal and is exactly what RANSAC
(below) does implicitly.

### 1.5 Robustness checklist

- **Use the brightest N, not all stars.** Faint detections are dominated by noise,
  drift in centroid, and include spurious sources; matching on them pollutes the
  vote. Astroalign caps `max_control_points=50` (`astroalign.py:258`); lumos caps
  `max_stars` (default 200) and takes the brightest, since `register()` assumes
  flux-sorted input (`src/registration/mod.rs:131-141`).
- **Reject degenerate triangles** (elongated, flat, collinear) — see §1.1.
- **Require multiple votes** before trusting a correspondence — see §1.3.
- **Keep tolerance tight.** A 1% ratio tolerance (lumos default) is conservative;
  loosening it inflates spurious matches super-linearly.

---

## 2. Transform models

The geometric model must match the physics of the frame difference. Too few DOF
leaves systematic residuals (under-fitting); too many DOF absorb noise and reduce
extrapolation accuracy outside the star convex hull (over-fitting). The hierarchy,
with degrees of freedom and minimal sample size `s` (the number of point pairs
needed to solve exactly):

| Model | DOF | Min pairs `s` | Captures | Typical use |
|-------|-----|--------------|----------|-------------|
| Translation | 2 | 1 | dx, dy | Same mount, no rotation, dithered subs |
| Euclidean (rigid) | 3 | 2 | + rotation | Tracked mount with field rotation, fixed scale |
| Similarity | 4 | 2 | + uniform scale | Different focal length / binning; default for tracked DSO |
| Affine | 6 | 3 | + shear + differential scale | Mild anisotropy, sensor tilt |
| Homography (projective) | 8 | 4 | + perspective | Wide-field lens, planetary mosaics, off-axis |
| TPS / SIP | many | — | smooth non-linear lens distortion | Wide-field, on top of a linear base |

lumos encodes this exact ladder in `TransformType` (`src/registration/transform.rs:14`),
with `min_points()` returning `{1,2,2,3,4}` (line 35) and `degrees_of_freedom()`
returning `{2,3,4,6,8}` (line 47), all matching the table.

**The optics that set the DOF (pass 2 depth).** Each DOF maps to a physical cause:

- **Translation** = dither/guiding drift and periodic-error residual; always present.
- **Rotation** = *field rotation*. An alt-az mount rotates the field at the parallactic
  rate (up to ~15″/s·sin(alt-az geometry) near the zenith); a German equatorial after a
  *meridian flip* rotates the field 180°; even a polar-aligned mount drifts a fraction
  of a degree over a session from polar-misalignment cone error. So Euclidean (rigid)
  is the *minimum* honest model for anything but a single short equatorial run.
- **Uniform scale** = plate-scale change. On one rig at fixed focus the plate scale is
  *fixed* to ~10⁻⁴, so the Similarity scale DOF is ≈1.0 and harmless — but it becomes
  essential across different focal lengths, focal reducers, binning modes, or a
  refocus that shifts the effective focal length (temperature-dependent in refractors).
- **Shear + differential scale** (affine) = sensor non-orthogonality / tilt, anamorphic
  optics, or rectangular-pixel mismatch — a *few*×10⁻³ effect, usually below the noise.
- **Perspective** (homography) = the field is genuinely projected from a tilted plane:
  off-axis pointing in a mosaic, or a fast wide lens where the gnomonic (TAN) projection
  itself is non-affine across the frame. The TAN projection (WCS Paper II,
  `wcs2_greisen2002.txt`) maps the tangent plane to the sky non-linearly; for a narrow
  field the linear approximation is excellent, but a several-degree field needs the
  projective (or distortion) terms.
- **Smooth non-linear** (SIP/TPS) = lens radial distortion (barrel/pincushion), field
  curvature, the residual the TAN+linear model leaves at the corners.

**Choosing by field.**

- For a guided/tracked deep-sky session on one rig, **Similarity** (or even
  Euclidean) is correct and is what astroalign always uses
  (`estimate_transform("similarity", ...)`, `astroalign.py:209`). The plate scale is
  fixed, so fitting scale is harmless but rotation and translation are essential.
- **Affine** rarely helps for clean optics — its extra shear/anisotropy DOF mostly
  absorb noise. Reach for it only when you have evidence of anisotropic scale
  (e.g. anamorphic optics, rectangular-pixel mismatch).
- **Homography** is required when the mapping is genuinely projective: a fast
  camera lens with significant field curvature, or mosaicking tiles imaged at
  different pointings. But homography on a near-rigid pair is dangerous: the two
  perspective DOF are weakly constrained by stars in a narrow field and will fit
  noise. Hence the **upgrade-by-residual** policy (lumos `Auto`, §3.5).
- **SIP / TPS** model the *residual* smooth distortion after the best linear fit —
  the radial barrel/pincushion of a lens that no homography can capture. They sit
  *on top of* a linear base, not in place of it (§5).

**Minimal sample sizes drive RANSAC cost.** Smaller `s` means dramatically fewer
RANSAC iterations for a given inlier ratio (the iteration count grows as `w^{-s}`,
§3.3). This is a strong reason to use the lowest-DOF model the physics allows:
Similarity (`s=2`) needs orders of magnitude fewer iterations than Homography
(`s=4`) at the same outlier fraction.

---

## 3. Robust estimation

Even after voting, the correspondence set contains outliers (mismatched stars,
asymmetric detections, moving objects). Robust estimation fits the model while
rejecting them. The lineage is RANSAC → MSAC → LO-RANSAC → MAGSAC++, all of which
sit inside the **USAC** "universal sample consensus" framework
([OpenCV USAC tutorial](https://docs.opencv.org/4.x/de/d3e/tutorial_usac.html)).

### 3.1 RANSAC and why a hard threshold is bad

Plain RANSAC (Fischler & Bolles 1981): repeatedly draw a minimal sample of `s`
correspondences, fit the model, and **count inliers** — points whose residual is
below a fixed threshold `t`. Keep the model with the most inliers; refit on its
inliers.

The weakness is the hard threshold `t`. Too small and the true model is rejected
because seeing/centroid noise pushes genuine inliers past `t`; too large and a
wrong model accrues outliers as "inliers." There is no single correct `t` because
the noise scale σ is unknown and varies with seeing, star brightness, and position.
Astroalign's `_ransac` uses exactly this hard-threshold counting with `PIXEL_TOL=2`
(`astroalign.py:589-644`) and the brittle "fit from 1 sample, then count" loop — it
works only because the triangle vote has already cleaned the data heavily.

### 3.2 MSAC: soft scoring

MSAC (Torr & Zisserman) keeps the binary threshold for *inlier selection* but
changes the *score*: instead of counting inliers, it sums a truncated quadratic
loss `ρ(r) = min(r², t²)` (equivalently, score `Σ min(r², t²)` to *minimize*), so a
model that fits its inliers *tightly* beats one that barely scrapes them under the
threshold. Plain RANSAC is the degenerate case `ρ(r) = [r² > t²]` (0/1 indicator);
MSAC just replaces the flat inlier plateau with the quadratic `r²` so residual
magnitude *inside* the band still discriminates. MLESAC generalizes further to a
maximum-likelihood mixture score. This is a strictly better objective at no extra cost
and is the conceptual bridge to MAGSAC: MAGSAC++ keeps the truncation (`outlier_loss`
beyond `kσ_max`) but replaces the hard inner `r²` with the σ-marginalized ρ (§3.5), so
the only remaining "threshold" is the soft upper bound `σ_max`, not a crisp inlier
cutoff.

### 3.3 Adaptive iteration count

The number of iterations `N` needed to draw at least one all-inlier sample with
probability `p`, given inlier ratio `w` and sample size `s`, is:

```
N = log(1 − p) / log(1 − w^s)
```

This is *the* RANSAC efficiency formula. It is recomputed each time a better model
raises the estimated `w`, so RANSAC terminates early once enough good samples are
statistically guaranteed. lumos implements it verbatim in `adaptive_iterations`
(`src/registration/ransac/transforms.rs:13-33`): `w_n = w^s`,
`N = ceil(log(1−p) / log(1−w^s))`, with guards for `w∈{0,1}`. The main loop
recomputes `adaptive_max` whenever the best inlier ratio improves and breaks once
`iterations ≥ adaptive_max` (`ransac/mod.rs:341-349`). MAGSAC's source builds the
same termination from its marginalized inlier count (verified pass 2,
`.tmp/refs/magsac/src/pymagsac/include/magsac.h:939-940`:
`last_iteration_number = log_confidence / log(1 − (inlier/N)^sample_size)`).

The practical lesson: with `s=2` (Similarity) and `w=0.5`, `N ≈ log(0.005)/log(0.75)
≈ 18`; with `s=4` (Homography) and the same `w`, `N ≈ log(0.005)/log(1−0.0625)
≈ 82`. Low-DOF models converge far faster — another argument for §2's
"use the simplest model."

### 3.4 LO-RANSAC: local optimization

LO-RANSAC (Chum, Matas & Kittler 2003; parsed pass 2,
[CMP PDF](https://cmp.felk.cvut.cz/~matas/papers/chum-dagm03.pdf)) observes that a
minimal sample, even when all-inlier, gives a noisy model because it ignores all
the *other* inliers. The fix: whenever a new best model is found, run a local
optimization — re-estimate using **all** current inliers (non-minimal least
squares), find the new inlier set, and iterate, optionally with a narrowing
threshold. The paper's decisive rule is **"if a new maximum has occurred (`I_k > I_j`
for all `j < k`), run local optimization"** (`loransac_chum2003.txt:99`) — i.e.
LO only on so-far-the-best samples, since the expected number of new maxima over a
run is only `O(log N)`, making LO nearly free. The paper tests four inner methods
(`loransac_chum2003.txt:221-230`): **(1) Standard** (no LO), **(2) Simple** (one
least-squares refit on the inliers), **(3) Iterative** — take points within `K·θ`,
refit, *reduce* the threshold, and iterate down (the narrowing-threshold variant), and
**(4) Inner RANSAC** — a fresh sampling restricted to the `I_k` consistent points.
"Fixing the LO-RANSAC" (Lebeda et al. 2012) further capped the inner-sample size and
iteration count. LO typically improves inlier count by 5–15% and tightens RMS.

lumos's `local_optimization` (`ransac/mod.rs:167-240`) implements method **(2)
Simple**, *iterated*: it re-estimates from all current inliers, rescores with the same
(non-narrowing) MAGSAC++ loss, swaps in the result only if it improves, and repeats up
to `lo_iterations` times until the inlier count and score stop improving
(`mod.rs:196-237`) — and crucially **only refines new-best hypotheses**
(`mod.rs:360-361`: `local_optimization && score > best_score`), the standard
cost-saving form. It does *not* do the method-(3) threshold narrowing — a noted gap
(§9.6). A final least-squares refit on all inliers happens after the loop
(`mod.rs:353-388`). This non-minimal final refit is essential: the reported transform
must never be the raw minimal-sample model.

### 3.5 MAGSAC++ : marginalizing the noise scale

MAGSAC (Barath, Noskova & Matas, CVPR 2019;
[CVF PDF](https://openaccess.thecvf.com/content_CVPR_2019/papers/Barath_MAGSAC_Marginalizing_Sample_Consensus_CVPR_2019_paper.pdf))
and MAGSAC++ (CVPR 2020; journal version *Marginalizing Sample Consensus*, TPAMI
2021, [PubMed 34375281](https://pubmed.ncbi.nlm.nih.gov/34375281/)) remove the hard
threshold entirely. The idea — "σ-consensus" — is to treat the noise scale σ as a
nuisance parameter and **marginalize over a range `σ ∈ [0, σ_max]`** rather than
committing to one threshold. Each point gets a *continuous weight* equal to its
marginal likelihood of being an inlier (integrated over σ), and the model quality is
a continuous loss with no inlier set required. The optimized model is then a
**weighted** least-squares fit using those marginal weights, iterated (IRLS).

**The exact MAGSAC++ ρ-function (pass 2, parsed from the primary PDF).** The
MAGSAC++ paper (arXiv 1912.05909, §2.1–2.2) gives the closed-form loss. The noise
σ is marginalized as `σ ∼ U(0, σ_max)`; inlier residuals follow a trimmed
χ-distribution with `n` DOF (`g(r|σ) = 2C(n)σ^{−n}exp(−r²/2σ²)r^{n−1}` for
`r < τ(σ)`, where `C(n)=(2^{n/2}Γ(n/2))^{−1}` and `τ(σ)=kσ`). The IRLS weight is the
σ-marginalized density `w(r) = ∫ g(r|σ)f(σ)dσ`, and the loss is its potential
`ρ(r) = ∫₀ʳ x·w(x)dx`. For `0 ≤ r ≤ kσ_max` the paper's closed form is

```
ρ(r) = [C(n)·2^{(n+1)/2}/σ_max]·[ (σ²_max/2)·γ((n+1)/2, r²/2σ²_max)
                                 + (r²/4)·(Γ((n−1)/2, r²/2σ²_max) − Γ((n−1)/2, k²/2)) ]
```

with `ρ(r)=ρ(kσ_max)` constant beyond, where `γ` is lower- and `Γ` upper-incomplete
gamma. The quality is `Q = 1/Σρ`. The paper sets `k = τ(σ)/σ = 3.64` (the 0.99
χ-quantile) and **`n = 4` for point correspondences** ("For problems using point
correspondences, `n = 4`"), with `σ_max` "set to a fairly high value, e.g. 10 pixels."
The reference C++ matches this exactly: `getModelQualityPlusPlus`
(`.tmp/refs/magsac/src/pymagsac/include/magsac.h:1028-1031`) computes
`loss = [σ²_max/2·γ_lower(x) + r²/4·(Γ_complete(x) − Γ_k)]·2^{(dof+1)/2}/σ_max`,
table-lookup `x = r²/(2σ²_max)`, constant `outlier_loss` above `maximum_threshold`,
score `1/total_loss`, early-exit once `total_loss > previous_best_loss`
(`magsac.h:1039`). Estimator constants: `getDegreesOfFreedom()=4`,
`getSigmaQuantile()=3.64`, `getC()=0.25`
(`.tmp/refs/magsac/src/pymagsac/include/estimators.h:36-49`) — and `C=0.25 = C(n=4) =
(2²·Γ(2))^{−1}` confirms the `n=4` choice.

**Correction (pass 2): lumos does NOT implement the paper's ρ for `n=2` (or any
standard `n`).** Pass 1 claimed lumos "specializes to 2-D point residuals (`k=2`
DOF)" via the exact closed form, asserting the lower-incomplete gamma "collapses to
`γ(1,x)=1−e^{−x}`." That is wrong: the paper's `n=2` form needs `γ(3/2, x)` (lower)
and `Γ(1/2, x)` (upper), **not** `γ(1, x)`. What lumos actually computes
(`magsac.rs:63-75`) is the bespoke loss

```
loss(r²) = (σ²ₘₐₓ/2)·(1 − e^{−x}) + (r²/4)·e^{−x},   x = r²/(2σ²ₘₐₓ)
```

i.e. it keeps the reference's structural skeleton `σ²_max/2·γ_lower + r²/4·(…)` but
substitutes the `a=1` incomplete gamma `1−e^{−x}` for the lower term and `e^{−x}` for
the upper `(Γ_complete − Γ_k)` factor, **and drops** the global `C(n)·2^{(n+1)/2}/σ_max`
prefactor. I verified numerically (recomputed pass 3 from the closed form via
half-integer incomplete gammas) that this diverges from the paper's `ρ` at every `r>0`
(σ_max=1: paper-n2 ρ(1.0)=0.323, paper-n4 ρ(1.0)=0.285, lumos loss(1.0)=0.348; at r=2
paper-n2=0.576, paper-n4=0.746, lumos=0.568; at r=3 paper-n2=0.622, paper-n4=0.908,
lumos=0.519). The divergence is *structural*, not a constant scale factor: lumos sits
**above** both paper curves at small `r`, **near** the n=2 curve at `r≈2`, then **below**
both by `r=3` — because, crucially, **lumos's loss is not monotone.** Written compactly,
`loss = (σ²_max/2)·[1 − e^{−x}(1−x)]` with `x = r²/2σ²_max`, so the derivative
`d(loss)/dx = (σ²_max/2)·e^{−x}(2−x)` is positive only for `x<2`: the loss **rises to a
peak at `r = 2σ_max`** (value `(σ²_max/2)(1+e^{−2}) ≈ 0.568·σ²_max`) and then **declines**
toward the clamp (`≈0.518·σ²_max` at `r=3.03σ_max`) before dropping to
`outlier_loss = 0.5·σ²_max`. Because the peak (`0.568·σ²_max`) *exceeds* the outlier loss
(`0.5·σ²_max`), a borderline point at `r≈2σ_max` is penalized **more** than a clear
outlier beyond `3.03σ_max` — the opposite of the paper's `ρ`, which is monotone
non-decreasing (it is the potential `∫₀ʳ x·w(x)dx` of a non-negative weight `w`). The
in-tree boundary test already notes the tail of this effect ("the MAGSAC formula can
overshoot slightly near the boundary", `magsac.rs:193-194`), but the overshoot is not
confined to the boundary — it peaks well *inside* the band, at `2σ_max`. So lumos's
scorer is **MAGSAC++-*inspired*, not the MAGSAC++ loss**: a smooth, bounded,
threshold-free kernel that keeps the *operational* traits (σ-derived scale, `1/loss`
quality, budget-based early exit, a soft 0→peak ramp for tight inliers) but is neither
the marginalized χ-density nor even a monotone robust kernel. Two practical consequences:
(a) dropping the constant prefactor is harmless for **model selection within one
`register()` call** (σ_max is fixed, so a uniform rescale cannot change the argmax), but
the scores are not comparable across different σ_max; (b) the non-monotonicity means
lumos faintly *prefers* a model that drives borderline points either tight (`r<2σ`) or
clearly out (`r>3σ`) over one that leaves them near `2σ`. The effect is small and bounded
(the in-band excess over the outlier loss is ≤`0.07·σ²_max`, all in the 2–3σ fringe), and
the triangle vote + LO-RANSAC + match-recovery carry the load, so accuracy is unaffected
in practice — but the loss should not be described as the paper's ρ or as monotone.

The per-point loss is clamped to `outlier_loss = σ²ₘₐₓ/2` beyond
`threshold² = χ²₀.₉₉(2)·σ²ₘₐₓ = 9.21·σ²ₘₐₓ` (`magsac.rs:43-74`; note `9.21 = χ²₀.₉₉(2)`,
distinct from the reference's `k=3.64` *χ*-quantile — lumos uses a `χ²`-quantile on
`r²` directly). The hand-computed unit tests verify the lumos formula exactly — e.g.
`loss(r²=1)` with σ=1 equals `0.5·(1−e^{−0.5}) + 0.25·e^{−0.5} = 0.34837`
(`magsac.rs:138-181`). The effective hard fallback threshold is `√9.21·σ ≈ 3.03σ`
(`magsac.rs:248`), which is why lumos derives `max_sigma` from seeing and treats
`~3·max_sigma` as the match-recovery threshold (§4).

**σ-consensus vs σ-consensus++ (pass 2).** The original MAGSAC (arXiv 1803.07469)
computed the weights by *partitioning* `[σ₁, σ_max]` into `d` discrete bins, running a
least-squares fit per bin to get `θ_σ`, and accumulating each point's implied inlier
probability across bins — accurate but slow (several LS fits per model). MAGSAC++
replaces this with the IRLS reformulation above: the weight is the analytic
σ-marginal `w(r)` (one closed-form evaluation per point, via a gamma LUT), and
`θ_{i+1} = argmin_θ Σ w(D(θ_i, p))·D²(θ, p)` is iterated to a local minimum — this is
**σ-consensus++**, used both for the non-minimal fit and as a post-process polisher for
*any* robust estimator's output (`magsacpp_barath2019.txt:166-232`). lumos's
`local_optimization` is the structural analogue (re-fit on the consensus set, iterate)
but uses an *unweighted* least-squares estimator on the binary inlier set rather than a
true `w(r)`-weighted IRLS — so it is closer to plain LO-RANSAC than to σ-consensus++.

Intuition for *why marginalization beats a hard threshold*: a point's contribution
fades **smoothly** from "definitely inlier" (r≈0, loss→0) to "definitely outlier"
(r>3σ, loss→constant) with no cliff, so a genuine inlier nudged just past a single
threshold by seeing noise is not discarded — it is down-weighted continuously. This
is what makes MAGSAC++ "more geometrically accurate and fail fewer times" than
threshold RANSAC (verified: TPAMI 2021 abstract; MAGSAC++ paper reports the new
quality function plus σ-consensus++ as the source of both the accuracy gain and the
speedup over MAGSAC).

lumos's `score_hypothesis` (`ransac/mod.rs:613-640`) sums this loss over all
correspondences, returns `−total_loss` (so higher = better), and **preemptively
exits** once the partial loss exceeds the current best's budget — the same
early-termination trick as the C++ reference.

### 3.6 Normalization (DLT + Hartley) — non-negotiable for homography/affine

The Direct Linear Transform solves the homogeneous system `A h = 0` for the 9
homography entries, where each correspondence contributes two rows. Solving this
*on raw pixel coordinates* (values in the thousands) is numerically catastrophic:
the design-matrix columns span many orders of magnitude, the condition number
explodes, and the smallest singular vector is corrupted. **Hartley normalization**
(Hartley, "In defence of the 8-point algorithm", 1997;
[ANU PDF](https://users.cecs.anu.edu.au/~hartley/Papers/fundamental/fundamental.pdf))
fixes this: translate each point set so its centroid is at the origin and scale so
the **average distance from the origin is √2**, solve in normalized space, then
denormalize `H = T_target⁻¹ · H_norm · T_ref`. Verified across multiple sources
(the √2 isotropic scaling and the resulting conditioning improvement).

lumos does this correctly. `normalize_points` (`ransac/transforms.rs:345-387`)
centers and scales to mean distance √2, returning the similarity normalizer `T`.
`estimate_homography` (`transforms.rs:265-342`) builds the full `2n×9` design matrix
*directly* (not `AᵀA`) so the SVD sees condition number κ rather than κ²
(`solve_homogeneous_svd`, line 394, returns the last row of Vᵀ — the right singular
vector of the smallest singular value), then denormalizes and rescales so `h[8]=1`.
`estimate_affine` (`transforms.rs:172-262`) applies the **same** Hartley
normalization before its normal-equations solve — the right call, since affine DLT
suffers the same conditioning problem. OpenCV's `findHomography`/`calib3d`
estimators follow the identical normalize-solve-denormalize pattern; the build of A
and the `AᵀA` vs full-A tradeoff is the textbook DLT.

### 3.7 Procrustes / Umeyama for similarity & rigid

For Translation/Euclidean/Similarity, the closed-form least-squares solution is
**Procrustes analysis** (the Kabsch/Umeyama solution), not DLT. Umeyama 1991
([IEEE TPAMI 13:376](https://ui.adsabs.harvard.edu/abs/1991ITPAM..13..376U/abstract))
gives the *correct* rotation even with corrupted data by enforcing both
orthogonality and `det(R)=+1` via the SVD of the cross-covariance `Σ = UDVᵀ`, setting
`R = U·diag(1,…,1, det(UVᵀ))·Vᵀ` so that a reflection (`det(UVᵀ)=−1`) is flipped back
to a proper rotation — fixing the earlier Arun/Horn methods that could return an
improper reflection when the data is noisy. lumos uses the **analytic 2-D Umeyama
formulas** directly: `estimate_similarity` computes the rotation angle
`θ = atan2(Sxy−Syx, Sxx+Syy)` and scale `s = ((Sxx+Syy)cosθ + (Sxy−Syx)sinθ)/Var_ref`
from the cross-covariance of centered points (`transforms.rs:147-152`);
`estimate_euclidean` (`transforms.rs:76-108`) is the same with `s≡1`.

**Correction/refinement (pass 2): the 2-D `atan2` form is *structurally* a proper
rotation — no det guard is needed.** The general-`d` Umeyama det-correction exists
because SVD-derived `R` can be a reflection; but in 2-D the closed form
`θ = atan2(·, ·)` *parameterizes a rotation by construction* — every `θ` maps to a
rotation matrix with `det = +1`, so `estimate_similarity`/`estimate_euclidean` can
never emit a reflection regardless of the data. The pass-1 §9 worry that "a degenerate
sample could in principle yield a reflection" is therefore **unfounded for these two
estimators**: the only way lumos handles a mirrored frame is by *disabling*
`check_orientation` so the triangle vote proposes mirror correspondences — at which
point the 2-D rotation model simply cannot fit them (it has no reflection DOF), and the
fit fails the RMS gate rather than silently mirroring. To actually register mirrored
frames one needs an affine/homography model (which *can* represent a reflection via a
negative-determinant linear part) or an explicit pre-flip. Only `estimate_affine` /
`estimate_homography` can express reflection, and those are unconstrained DLT fits with
no orthogonality assumption, so no det guard applies there either.

### 3.8 Degeneracy handling

A minimal sample of collinear or coincident points yields a rank-deficient,
useless model. Robust estimators must detect and skip these. lumos's
`is_sample_degenerate` (`ransac/mod.rs:566-601`) rejects samples with any pair
closer than 1 px or all points collinear (cross product below
`COLLINEARITY_THRESHOLD = 1.0`) *before* fitting (`mod.rs:287`). The MAGSAC
reference goes further for fundamental-matrix estimation with **DEGENSAC**
(`isValidModel`/`applyDegensac`, `/.tmp/refs/magsac/.../estimators.h:229-516`),
detecting an H-degenerate sample (5+ points consistent with a single plane) and
re-estimating via plane-and-parallax — relevant if lumos ever adds essential/
fundamental models, but not needed for the 2-D similarity/homography case.

### 3.9 Model upgrade by residual (`Auto`)

The cleanest way to pick the model is data-driven: fit the *simplest* adequate
model and upgrade only if its residual is too large. lumos's `Auto` does this — it
runs RANSAC with **Similarity** first and accepts it if `rms_error ≤ 0.5 px`,
otherwise re-runs with **Homography** (`src/registration/mod.rs:163-185`,
`AUTO_UPGRADE_THRESHOLD=0.5`). This is the right default: a near-rigid pair gets the
well-conditioned 4-DOF fit (fast, robust, good extrapolation), and only genuinely
projective fields pay for the 8-DOF model. PROSAC's "sort by quality, sample best
first" idea is mirrored by lumos's 3-phase progressive sampling, which front-loads
high-vote correspondences (`PHASE_POOL_FRACTIONS=[0.25,0.50,1.0]`, weighted in the
first two phases, `ransac/mod.rs:30-36, 451-477`).

---

## 4. Match recovery / refinement

RANSAC returns a transform fit from a *minimal-then-locally-optimized* inlier set,
which is often a fraction of the truly matchable stars (the triangle vote only
proposes correspondences that happened to share enough triangles). Once a good
transform exists, you can **recover** many more matches by projecting every
reference star through the transform and snatching the nearest target star within a
tight gate — a guided nearest-neighbor re-match — then refitting, iterating to
convergence. This is the cheapest large win in registration accuracy: it both grows
`n` (lowering the final least-squares variance) and tightens the model.

Astroalign does a lightweight version: after RANSAC it dedups multiple assignments
to the same source, keeping the pair with lowest reprojection error
(`astroalign.py:386-403`), and `_ransac` itself does 3 refinement passes
(`astroalign.py:637-642`).

lumos's `recover_matches` (`src/registration/mod.rs:376-458`) is a fuller
implementation:

1. Build a k-d tree over target stars (`mod.rs:384`).
2. Up to `RECOVERY_MAX_ITERATIONS = 5` passes (`mod.rs:374`): for each *unmatched*
   reference star, project it via the current transform and accept the nearest
   target if within `threshold² = (3.03·max_sigma)²` and not already claimed
   (`mod.rs:411-426`). The threshold is the MAGSAC++ effective hard cutoff (§3.5),
   derived from seeing — `effective_threshold = max_sigma * 3.03`
   (`mod.rs:311`).
3. **Re-validate** all current matches against the refreshed transform, dropping
   any that drifted past the gate (`mod.rs:429-432`) — this removes seed outliers,
   verified by the `test_iterative_recovery_removes_outliers` test (`mod.rs:668`).
4. Refit the transform on the grown set (`mod.rs:446`), stop when the count
   stabilizes (`mod.rs:435`).
5. Safety: never return fewer matches than the input (`mod.rs:453`).

This NN-re-match-and-refit loop is the standard "tweak" step (astrometry.net's
`tweak2`/`solver_tweak2`, `/.tmp/refs/astrometry.net/solver/solver.c:126-220`, does
the analogous thing: re-correspond field stars to catalog under the current WCS,
recompute log-odds, re-fit). The **final least-squares fit on the full inlier set**
is what determines the reported RMS and the transform actually used for warping —
the minimal sample is never the final answer.

A subtlety: recovery uses a *fixed* gate (`3.03·max_sigma`), whereas the RANSAC
scoring used the smooth MAGSAC++ loss. Using the gate here is fine because by this
point the transform is already accurate to well under a pixel, so the gate is
generous; but the gate width directly bounds how many matches can be recovered and
should track the seeing (which it does, via `max_sigma`).

---

## 5. Distortion modeling

After the best *linear* model, a wide-field image still shows a smooth, position-
dependent residual — barrel/pincushion from the lens, field curvature, mild
mustache distortion. Two standard models capture it.

### 5.1 SIP (Simple Imaging Polynomial)

SIP (Shupe et al. 2005,
[FITS registry PDF](https://fits.gsfc.nasa.gov/registry/sip/SIP_distortion_v1_0.pdf))
is the FITS-standard distortion convention used by Spitzer, HST, astrometry.net,
Siril, ASTAP. It adds polynomial corrections to pixel coordinates *before* the
linear (CD-matrix / TAN) transform:

```
u' = u + Σ A_pq · uᵖ · vᵍ      (2 ≤ p+q ≤ order)
v' = v + Σ B_pq · uᵖ · vᵍ
```

where `(u,v)` are pixel offsets from the reference pixel CRPIX. Linear terms
(`p+q<2`) are excluded because the CD matrix already represents them. The canonical
form is exactly `sip_calc_distortion` in astrometry.net
(`/.tmp/refs/astrometry.net/util/sip.c:364-396`): `U = u + f(u,v)`, `V = v +
g(u,v)` with `f,g` polynomials in `A[p][q]`, `B[p][q]`, applied relative to CRPIX
(`sip_distortion`, `sip.c:73-81`). lumos's `SipPolynomial`
(`src/registration/distortion/sip/mod.rs`) matches this convention exactly: the doc
comment (`mod.rs:8-26`) gives the same `u' = u + Σ A_pq uᵖvᵍ` form, terms run
`2 ≤ p+q ≤ order` (`term_exponents`, `mod.rs:375-384`), and the correction is
applied before the linear transform in `WarpTransform::apply`
(`transform.rs:346-351`: `transform.apply(sip.correct(p))`).

**Coefficient counts.** Per axis, an order-`n` SIP has `(n+1)(n+2)/2 − 3` terms:
order 2 → 3 terms, order 3 → 7, order 4 → 12, order 5 → 18. lumos tabulates this
(`mod.rs:21-27`, `MAX_TERMS=18`).

**Order selection & overfitting guards.** This is the crux. The verified consensus
(astrometry.net mailing list; STScI DrizzlePac docs): a SIP fit is well-behaved
*inside the convex hull of the matched stars* but "as soon as you go outside the
convex hull, the SIP predictions wobble around, worse and worse as the order
increases." So:

- **Match order to the physics.** Order 2 = barrel/pincushion (most refractor/lens
  fields); order 3 = + mustache; order 4–5 only for HST-class or extreme wide-field.
  Higher is *not* better — it fits noise and explodes on extrapolation.
- **Require many points per term.** lumos enforces `n ≥ 3·terms` before fitting
  (`mod.rs:163-167`), citing astrometry.net practice ("order 4 → 12 terms → ≥36
  points"). This 3× rule is the primary overfit guard.
- **Normalize coordinates.** lumos normalizes `(u,v)` by the average distance to
  the reference point before building the basis (`norm_scale`, `mod.rs:173`,
  `normalize_point` `mod.rs:394`) — the same conditioning fix as Hartley
  normalization, vital because raw `uᵖvᵍ` for `p+q=5` over thousands of pixels
  overflows the dynamic range of the normal equations.
- **Robust fit with sigma-clipping.** lumos fits by normal equations solved with
  Cholesky (falling back to LU when not positive-definite or ill-conditioned,
  `solve_cholesky`/`solve_lu`, `mod.rs:479-598`), wrapped in MAD-based
  sigma-clipping (`clip_sigma=3`, `clip_iterations=3`, `mod.rs:194-264`,
  `MAD_TO_SIGMA=1.4826`). scamp and astrometry.net likewise fit distortion with
  iterative outlier rejection. lumos defaults to **order 3** (`SipConfig::default`,
  `mod.rs:74-82`) — a sane middle ground.
- **Report quality.** `SipFitResult` exposes `rms_residual`, `max_residual`,
  `points_used/rejected`, and `max_correction` (`mod.rs:122-136`) so a caller can
  detect a runaway fit (huge `max_correction` with few `points_used` ⇒ overfit).

### 5.2 PV/TPV (the SCAMP/SExtractor alternative)

SCAMP (Bertin) and the TPV convention represent distortion as polynomials in
*intermediate world coordinates* via `PVi_j` keywords on a `TPV` projection rather
than pixel-space `A/B` (`/.tmp/refs/scamp/src/fitswcs.c:610-905`: SCAMP reads/writes
`PV?_????` and treats `TPV` as a distinct pcode). PV/TPV and SIP are inter-
convertible (Shupe 2012, "SIP to PV") but live on opposite sides of the projection.
For a frame-to-frame *pixel* registration (lumos's case) the SIP pixel-space form is
the natural choice; PV/TPV matters mainly when writing a WCS for downstream
astrometry tools.

### 5.3 TPS (thin-plate spline) — the non-parametric alternative

A thin-plate spline interpolates a smooth deformation through the control points by
minimizing bending energy; it is *non-parametric* (one radial basis term per
control point) so it adapts to arbitrary smooth warps without choosing a polynomial
order, at the cost of being defined by all control points (heavier, and similarly
ill-behaved on extrapolation). lumos has a **complete, tested TPS implementation
that is `#![allow(dead_code)]` and not wired into `register()`**
(`src/registration/distortion/tps/`) — an alternate post-RANSAC distortion model
kept on the shelf. TPS is the better choice when the distortion is not well
described by a low-order polynomial (e.g. a stitched optical path); SIP is better
when you need a standard FITS-writable solution and the field is a smooth lens warp.

---

## 6. Warping / resampling

Once the transform (and optional distortion) is known, the moving image is
resampled onto the reference grid. Two facts dominate the design.

### 6.1 Inverse mapping, always

Resample by **iterating over output pixels and pulling from the input** (inverse
mapping), never by scattering input pixels forward. Forward mapping leaves holes
and overlaps; inverse mapping visits every output pixel exactly once and asks "where
in the source does this come from?" Astroalign uses `skimage.transform.warp` with
`inverse_map=transform.inverse` (`astroalign.py:441-450`). lumos does the same:
`warp()` produces a fresh output and fills every output pixel by inverse-mapping
through `WarpTransform` (`src/registration/mod.rs:243-265`; the transform docstring
explicitly explains the inverse-sampling direction, `transform.rs:225-232`).

### 6.2 Interpolation kernel comparison

For each output pixel, the source location is fractional, so the value is
interpolated from neighbors with a kernel:

- **Nearest neighbor** (`Nearest`): pick the closest pixel. Fast, but introduces
  up to ±0.5 px positional error and aliasing. **Never use on science data** — it
  shifts flux by up to half a pixel and destroys sub-pixel registration.
- **Bilinear** (2×2): linear blend. Cheap, no ringing, but it is a low-pass filter —
  it *blurs*, enlarging stellar FWHM. Astrophotographers note bilinear "removes the
  dark-halo artifacts but the image is not as sharp (larger FWHM)" (verified).
  Good for masks/weights and quick previews.
- **Bicubic** (4×4, Keys cubic convolution with `a=−0.5`): the Keys (1981) family is
  the piecewise cubic `k(x) = (a+2)|x|³−(a+3)|x|²+1` for `|x|≤1` and
  `a|x|³−5a|x|²+8a|x|−4a` for `1<|x|<2`. The free parameter `a` controls the negative
  lobe; **`a=−0.5` is the unique value that makes the interpolant agree with the
  Taylor series to third order** (the "Catmull-Rom"/Keys-optimal choice), so it is the
  most accurate cubic — sharper than bilinear, mild overshoot. lumos's
  `bicubic_kernel` hard-codes `A=−0.5` (`resample/kernel/mod.rs`), the standard
  choice. A reasonable default when ringing must be avoided.
- **Lanczos-a** (windowed sinc, kernel half-width `a`∈{2,3,4}; 2a×2a footprint): the
  ideal reconstruction filter is `sinc(x)=sin(πx)/(πx)`, but the sinc has infinite
  support and slow `1/x` decay; the Lanczos kernel multiplies it by a *sinc window*
  `sinc(x/a)`, giving `L(x)=sinc(x)·sinc(x/a)` for `|x|<a` and 0 outside. This is the
  best practical frequency response — it preserves the most detail / smallest FWHM
  growth — and is the de-facto astrophotography default. Larger `a` → closer to the
  ideal sinc (sharper) but larger negative lobes (more ringing). lumos defaults to
  **Lanczos3** (`InterpolationMethod::default()`, `config.rs`),
  with a 4096-sample LUT per kernel (`resample/kernel/mod.rs`, ~0.00024
  precision). The kernel is exactly `(sin(πx)/πx)·(sin(πx/a)/(πx/a))` for `|x|<a`
  (`lanczos_kernel_compute`, `mod.rs:45-55`). swarp implements the identical
  Lanczos2/3/4 windowed-sinc kernels
  (`.tmp/refs/swarp/src/interpolate.c:337-428`, computed via `sincos`).
- **Gaussian**: not a sharp interpolator but produces clean, ring-free PSFs;
  preferred when photometric PSF fidelity beats sharpness (verified — "smooth,
  centrally peaked PSFs … better behaved for photometry").

### 6.3 Ringing and signed data

The Lanczos kernel has negative lobes, so near a sharp bright edge (a saturated
star, a hot column) it overshoots into negative values — the **Gibbs/ringing**
phenomenon, seen as **dark halos around bright stars** (verified across sources;
explicitly noted for Lanczos-3 in astrophotography). For `a=2` the ringing is
< 1%, growing with `a`. Mitigations:

- **Preserve linearity on calibrated data.** lumos uses normalized linear Lanczos
  only where the complete kernel is available. Partial kernels use edge-extended
  bilinear interpolation rather than division by a potentially near-zero truncated
  signed weight sum, but only while the source coordinate remains inside the closed
  source pixel footprint `[-0.5, width−0.5] × [-0.5, height−0.5]`. Every method
  returns the configured fill and zero coverage/confidence outside that footprint.
  Bilinear and bicubic likewise normalize partial support using only real source
  pixels. The former absolute-intensity soft clamp was removed because
  calibrated samples can be negative; classifying taps by `sample × weight` was not
  translation invariant and could amplify ordinary negative values catastrophically.
- **Lower `a`** (Lanczos2 instead of 3/4) or fall back to bicubic/bilinear in
  high-contrast regions.

### 6.4 Flux conservation vs interpolation, and when to drizzle

Plain interpolation (bilinear/bicubic/Lanczos) is **not flux-conserving** — it
estimates the value at a point, not the integral over a pixel. For most stacking
this is acceptable because the per-frame error averages out and relative photometry
survives. But two regimes demand a flux-conserving resampler:

- **Sub-sampled / undersampled data you want to up-sample.** **Drizzle** (Fruchter
  & Hook 2002) is a *flux-conserving resampler*: it maps a shrunken "drop" of each
  input pixel onto a finer output grid, distributing flux by geometric overlap area.
  It recovers resolution lost to undersampling and produces a coverage/weight map.
  lumos implements this as a separate stage (`src/stacking/drizzle/`), and each
  `DrizzleFrame` bundles a source with its registration `Transform` and weights. Use drizzle (not
  warp) when frames are dithered and undersampled and you want to up-sample; use
  plain warp when the data is already well-sampled and you just need alignment.
- **Surface-brightness / mosaic photometry across resampled tiles.** The `reproject`
  package distinguishes interpolation, an **adaptive anti-aliasing** resampler
  (DeForest's algorithm, `/.tmp/refs/reproject/reproject/adaptive/`), and exact
  flux-conserving **spherical-polygon** reprojection — the last conserves surface
  brightness exactly and is the right tool for mosaics, at much higher cost.

The general principle: **interpolate when you need a sharp value at a point;
flux-conserve (drizzle / exact reprojection) when you need the integral over a
pixel preserved.**

### 6.5 Sub-pixel accuracy limits resolution

The final stacked resolution is bounded by the **registration RMS added in
quadrature with the resampling blur**. If centroids are good to ~0.1 px and the
transform RMS is ~0.2 px, alignment costs ~0.2 px of effective PSF broadening —
negligible against a 2 px FWHM. But if RMS climbs toward 0.5–1 px (poor matches,
wrong model, distortion uncorrected at field edge), stacking *blurs* rather than
sharpens. This is why every earlier stage matters: tight centroids (Stage 3),
the right transform model (§2), robust fitting (§3), match recovery (§4), and
distortion correction at the field edge (§5) all exist to keep that RMS small.
Choosing nearest-neighbor or an unnecessarily blurry kernel then squanders the
budget at the last step.

---

## 7. Recommended best-practice implementation

A defensible end-to-end registration stage:

1. **Detect & sort stars by flux** (Stage 3). Feed only the **brightest 50–200**
   into registration; faint sources add noise and spurious matches.
2. **Asterism match** with kNN triangles (k≈5–10), reject elongated (`R>10`) /
   flat / collinear triangles, range-search invariants with ~1% ratio tolerance,
   and **vote** for correspondences requiring ≥3 confirming triangles. Enforce
   orientation unless mirrored frames are expected.
3. **Robust fit** with MAGSAC++-scored RANSAC + LO-RANSAC:
   - Derive `σ_max` from the median stellar FWHM (`σ_max ≈ FWHM/2`), so the noise
     model tracks seeing instead of a magic constant.
   - Start with the **simplest adequate model** (Similarity), use **Hartley
     normalization** for any DLT (affine/homography), recompute the **adaptive
     iteration count** `N=log(1−p)/log(1−wˢ)`, skip degenerate samples, and run
     **local optimization on new-best hypotheses** only.
   - **Upgrade to Homography only if the Similarity residual exceeds ~0.5 px.**
4. **Recover matches**: project all reference stars, NN-re-match within ~3σ, drop
   drifted matches, refit; iterate to convergence (≤5 passes).
5. **Final least-squares** on the full inlier set — never report the minimal model.
6. **Distortion** (wide field only): fit SIP order 2–3, require ≥3× points per
   term, normalize coordinates, sigma-clip, and inspect `max_correction`/RMS for
   overfit. Apply SIP *before* the linear transform.
7. **Resample** by inverse mapping with **normalized linear Lanczos-3** for science
   stacks; **drizzle** instead when dithered & undersampled; bilinear only for
   masks/previews. **Propagate separate support and confidence maps** so the stacker
   can exclude extrapolated pixels and account for interpolation noise independently.
8. **Gate on RMS** — reject the registration if the final RMS exceeds a fraction of
   the FWHM (lumos: `max_rms_error`, `mod.rs:199`).

lumos already implements 1–6 and 8 closely to this; see §9 for the gaps.

---

## 8. Pitfalls & anti-patterns

- **Matching on too many faint / spurious stars.** Faint detections have noisy
  centroids and include hot pixels and cosmic rays; they flood the triangle vote
  with garbage. *Fix:* brightest-N only.
- **No coordinate normalization in DLT.** Solving homography/affine on raw pixel
  coordinates makes the normal equations / design matrix wildly ill-conditioned and
  the SVD null-vector meaningless. *Fix:* Hartley √2 normalization (lumos does this
  for both homography and affine).
- **SVD of `AᵀA` instead of `A`.** Squaring the matrix squares the condition
  number. *Fix:* SVD the full `2n×9` design matrix (lumos `solve_homogeneous_svd`).
- **Hard RANSAC threshold.** A single inlier cutoff is either too tight (rejects
  the true model under seeing noise) or too loose (admits a wrong model). *Fix:*
  MSAC soft loss at minimum, MAGSAC++ marginalization ideally.
- **Reporting the minimal-sample model.** The 2–4 point fit is noisy. *Fix:*
  always do a final non-minimal least-squares refit on all inliers.
- **Over-high SIP/polynomial order.** High order fits noise and **explodes outside
  the star convex hull** (the field corners — exactly where you most need it).
  *Fix:* lowest order that flattens residuals; ≥3× points per term; inspect
  extrapolation.
- **Homography on a near-rigid pair.** The two perspective DOF are weakly
  constrained in a narrow field and absorb noise, *worsening* extrapolation versus
  Similarity. *Fix:* upgrade-by-residual (lumos `Auto`).
- **Nearest-neighbor warp on science data.** Up to ±0.5 px flux displacement,
  aliasing, destroyed sub-pixel registration. *Fix:* Lanczos/bicubic; NN only for
  integer-labeled masks.
- **Ignoring ringing.** Lanczos dark halos around bright stars corrupt photometry
  and look ugly when stacked. *Fix:* use a lower `a`, bicubic, or bilinear where
  ringing is more damaging than the corresponding loss of sharpness.
- **Double interpolation.** Warping an already-warped/resampled image compounds
  blur. *Fix:* compose all transforms (linear ∘ distortion ∘ …) and resample the
  *original* pixels **once** (lumos's `WarpTransform` bundles SIP + linear so a
  single inverse-map pass applies both, `transform.rs:346`).
- **Not propagating which pixels were extrapolated.** Output pixels mapped from
  outside the input have no real data (border fill); silently stacking them biases
  the result. *Fix:* emit a footprint/coverage mask (astroalign returns `footprint`,
  `astroalign.py:452-458`; drizzle emits a coverage map). lumos's `warp()` returns
  magnitude-based geometric support plus independent coefficient-energy confidence,
  both exactly zero outside the closed source pixel footprint.
- **Mismatched transform direction.** Confusing `apply` vs `apply_inverse` flips the
  alignment. *Fix:* document the convention rigorously (lumos: `T.apply(ref)→target`,
  warp samples target at reference-mapped positions, `transform.rs:220-248`).
- **Fixed noise threshold independent of seeing.** A 2-px tolerance is wrong for
  both 1-px and 5-px FWHM data. *Fix:* derive σ from the measured median FWHM
  (lumos: `max_sigma = max(FWHM/2, 0.5)`, `mod.rs:128-129`).

---

## 9. How lumos currently does it — and gaps/opportunities

**Pipeline** (`register()`, `src/registration/mod.rs:105-207`): validate ≥`min_stars`
→ derive `max_sigma = max(median_FWHM·0.5, 0.5)` → take brightest `max_stars` →
kNN-triangle match with ratio-space voting → MAGSAC++-RANSAC (`Auto`:
Similarity, upgrade to Homography if RMS>0.5) → iterative match recovery → optional
SIP → RMS gate. This closely follows the best-practice flow of §7 and is
well-grounded against astroalign, MAGSAC, astrometry.net, and SCAMP.

**Strong points.**
- Correct **Groth R=10** elongated-triangle rejection + Heron flatness guard
  (`geometry.rs:71-83`).
- **Voting with ≥3 confirming triangles** and greedy one-to-one resolution
  (`voting.rs`).
- **MAGSAC++-*inspired* threshold-free loss**, hand-verified by unit tests
  (`ransac/magsac.rs`) — a smooth, bounded (but **non-monotone**, §3.5) robust kernel,
  *not* the paper's exact σ-marginalized ρ (see §3.5 Correction); preemptive scoring;
  **adaptive iterations**;
  **LO-RANSAC on new-best only**; **degeneracy rejection**; **final non-minimal refit**
  (`ransac/mod.rs`).
- **Hartley √2 normalization** for *both* homography and affine, SVD of the full
  design matrix (`ransac/transforms.rs`).
- **Seeing-derived** noise scale and recovery gate (`mod.rs:128, 311`).
- **SIP** matching the FITS convention, normalized coordinates, 3×-points-per-term
  guard, MAD sigma-clipping, Cholesky→LU fallback (`distortion/sip/mod.rs`).
- **Normalized linear Lanczos-3** default with a signed-safe partial-kernel fallback,
  plus a single-pass `WarpTransform` that applies SIP+linear together (no double
  interpolation) (`resample/`, `transform.rs`).

**Gaps / opportunities.**
1. **Resolved: warp quality is explicit.** `warp()` returns an in-bounds kernel-
   magnitude coverage map and a separate coefficient-energy confidence map. The
   stacker uses coverage only for inclusion and confidence only for inverse-variance
   weighting (`registration/resample/`, `combine/cache/mod.rs`).
2. **TPS is implemented, tested, but unwired** (`distortion/tps/`,
   `#![allow(dead_code)]`). For optical paths poorly modeled by low-order SIP it is
   the better distortion model; wiring it as an alternative post-RANSAC option (with
   a selection heuristic) is a clear opportunity. Confirmed unwired in the crate map.
3. **SIP order is fixed by config, not auto-selected.** Best practice (astrometry.net
   `tweak`) tries increasing orders and stops when residuals stop improving (an
   AIC/BIC-style or RMS-plateau criterion). lumos always uses `SipConfig::order`; an
   automatic order sweep guarded by the 3×-points rule would prevent both under- and
   over-fitting without user tuning.
4. **`Auto` only considers Similarity↔Homography**, skipping Euclidean (the most
   common tracked-mount case, 3 DOF, fastest) and Affine. A finer ladder (Euclidean
   → Similarity → Affine → Homography, upgrading by residual) would fit the simplest
   adequate model more often.
5. **Mirrored-frame support (revised pass 2).** `estimate_similarity`/`euclidean` use
   the 2-D `atan2` closed form, which *structurally* yields a proper rotation
   (`det=+1`) — no reflection can leak out, so no det guard is needed (§3.7
   Correction). The real gap is the *opposite*: lumos has **no path to register a
   mirrored frame** (meridian flip, diagonal optical mirror). Disabling
   `check_orientation` lets the triangle vote propose mirror correspondences, but the
   similarity/euclidean model can't fit them (no reflection DOF), so the fit fails the
   RMS gate. Supporting mirrors cleanly would mean detecting the orientation sign from
   the vote and either pre-flipping the image or fitting an affine/homography (whose
   linear part can carry the `det<0`).
6. **Match recovery uses a fixed `3.03·σ` gate**, not the smooth MAGSAC++ loss. This
   is fine post-convergence but a narrowing-threshold recovery (à la "Fixing
   LO-RANSAC") could squeeze out a few more correct matches at the faint end.
7. **Lanczos warp SIMD is bilinear/Lanczos3-only** (per crate map: AVX2/SSE4.1
   bilinear and a const-generic Lanczos path; bicubic/Lanczos2/4 are scalar). Not a
   correctness gap, but Lanczos2/4 and bicubic could be vectorized if they become
   hot.
8. **MAGSAC++ loss is a heuristic, not the paper's ρ** (§3.5 Correction). lumos's
   `MagsacScorer::loss` uses `(σ²/2)(1−e^{−x}) + (r²/4)e^{−x}`, which is bounded but
   **non-monotone** — it peaks at `r=2σ_max` (≈`0.568·σ²_max`, *above* the `0.5·σ²_max`
   outlier loss) then declines — and does not equal the σ-marginalized χ-density loss
   for `n=2` or `n=4`. If
   exact MAGSAC++ behavior is wanted, implement the real ρ via a small incomplete-gamma
   LUT (`n=2`: `γ(3/2,·)`, `Γ(1/2,·)`; `n=4`: `γ(5/2,·)`, `Γ(3/2,·)`). In practice the
   current loss works because the triangle vote + LO-RANSAC + match-recovery do most of
   the heavy lifting; the divergence mainly affects borderline down-weighting at the
   `~2–3σ` fringe, not gross inlier/outlier separation.

---

## Primary sources parsed (pass 2)

PDFs fetched and `pdftotext`-extracted to `.tmp/papers/` (relative to the lumos crate
root). Each line: source → load-bearing takeaway → local `.txt`.

- **Beroiz, Cabral & Sanchez 2020, *Astroalign*** (arXiv 1909.02946) → confirmed the
  invariant tuple `M({Lᵢ}) = (L₂/L₁, L₁/L₀)` with `L₂≥L₁≥L₀` (their Eq. 3–4), the
  brightest-50 cap, the `C(5,3)=10`-triangles-per-star kNN construction, and the
  invariant-space geometry (equilateral→(1,1), collinear curve `y=(x−1)⁻¹`). →
  `.tmp/papers/astroalign_beroiz2020.txt`.
- **Lang et al. 2010, astrometry.net** (arXiv 0910.2233) → quad code construction
  (A,B widest pair → origin/(1,1); code `(xC,yC,xD,yD)`), symmetry-breaking `xC≤xD ∧
  xC+xD≤1`, C/D-in-circle constraint, "quad ≈ two triangles sharing an edge" rationale,
  and the **quantified Bayesian verifier** (Bayes factor `K=p(D|F)/p(D|B)`, prior
  `10⁻⁶`, utility `u(FP)=−1999`). → `.tmp/papers/astrometry_lang2010.txt`.
- **Barath et al. 2018, MAGSAC** (arXiv 1803.07469) → the original partition-based
  σ-consensus weighting (`d` bins, LS fit per bin) that MAGSAC++ supersedes. →
  `.tmp/papers/magsac_barath2018.txt`.
- **Barath et al. 2019/20, MAGSAC++** (arXiv 1912.05909) → the **exact closed-form ρ**
  (quoted in §3.5), `n=4` for point correspondences, `k=3.64` (0.99 χ-quantile),
  IRLS/σ-consensus++ reformulation, gamma-LUT. **This is what exposed the lumos
  loss-formula correction.** → `.tmp/papers/magsacpp_barath2019.txt`.
- **Groth 1986, AJ 91 1244** → the *actual* invariants `(R=longest/shortest, C=cosine
  at vertex 1)` with propagated tolerances, the `R≤10` cut ("triangle count drops a few
  %, search time halves"), the false-match count `≈48 n_t² ε²` ⇒ `ε ∝ n^{−3/2}`, the
  log-perimeter pruning, and the vote-drops-by-2 termination. →
  `.tmp/papers/groth1986.txt`.
- **Shupe et al. 2005, SIP convention** (Spitzer ADASS PDF) → confirmed `[x;y] =
  CD·[u+f(u,v); v+g(u,v)]`, `f=ΣA_pq uᵖvᵍ`, `g=ΣB_pq uᵖvᵍ` with `p+q≤ORDER`, and the
  inverse AP/BP polynomials (their Eqs. 1–6). → `.tmp/papers/sip_shupe2005.txt`.
- **Hartley 1997, "In defence of the 8-point algorithm"** → confirmed the isotropic
  normalization (centroid→origin, average distance→√2) and the condition-number
  argument (`κ` of `A` vs `κ²` when forming `AᵀA`). → `.tmp/papers/hartley1997.txt`.
- **Chum, Matas & Kittler 2003, LO-RANSAC** → confirmed "LO only on new maxima" and the
  four inner-method taxonomy (Standard/Simple/Iterative/Inner-RANSAC). →
  `.tmp/papers/loransac_chum2003.txt`.
- **Calabretta & Greisen 2002, WCS Paper II** (arXiv astro-ph/0207413) → TAN/gnomonic
  projection + CD-matrix → intermediate-world-coordinate chain (context for why a wide
  field needs projective/distortion terms). → `.tmp/papers/wcs2_greisen2002.txt`.
- **Fischler & Bolles 1981, RANSAC** → original consensus-set formulation (background
  for §3.1). → `.tmp/papers/fischler_bolles1981.txt`.
- **Failed to parse:** Umeyama 1991 (TPAMI 13:376) — every mirror found is a *scanned
  image* PDF with no text layer (`pdftotext` yields 0 bytes); the 2-D analytic formulas
  and the general `det`-correction are cross-checked against the lumos code and the
  standard statement instead. The IRSA SIP mirror and one Spitzer copy were the only
  working SIP PDFs. (Re-checked pass 3: `umeyama1991.txt` is still empty — 5 bytes.)

## Pass 3 — independent re-verification log

Every lumos `file:line` citation in this document was re-read against the current source
and found accurate (modulo ±1-line drift where a citation points at a `#[derive(…)]`
attribute rather than the `enum`/`struct` line it decorates — e.g. `TransformType` at
`transform.rs:14` vs 15, `Orientation` at `geometry.rs:11` vs 12; both resolve to the
right item). The load-bearing reference-clone citations were re-confirmed verbatim:
`magsac.h` termination (939-940), `getModelQualityPlusPlus` loss `maximum_sigma_2_per_2 *
γ_lower + r²/4·(Γ_complete − γ_k)` × `two_ad_dof_plus_one_per_maximum_sigma` (1028-1031)
and early-exit (1039); `estimators.h` `getSigmaQuantile()=3.64`, `getDegreesOfFreedom()=4`,
`getC()=0.25` (36-49); `astroalign.py` (`r=0.1` empirical comment 349-356, 3 refinement
passes 637-642, `_invariantfeatures` 106-115, footprint 452-474); `codefile.c`
scale/costheta/sintheta + C-rotation (49-65); `sip.c` `sip_calc_distortion` `*U=u+fuv`,
`*V=v+guv` (364-396) and `sip_distortion` (73-78); `swarp/interpolate.c` LANCZOS2/3/4 via
`sincos` (337-428); `scamp/fitswcs.c` `PV?_????` + `TPV` pcode (610-905); `solver.c`
`solver_tweak2` (126). Paper claims re-confirmed in `.tmp/papers/`: Groth `R=r3/r2`,
`C`=cosine at vertex 1, `≈48 n_t² ε²` false-match count, log-magnification pruning;
astrometry.net prior `p(F)/p(B)=10⁻⁶`, `u(FP)=−1999`, "quad ≈ two triangles sharing an
edge"; MAGSAC++ "`n = 4` for point correspondences" and the `0 ≤ r ≤ kσ_max` closed form.
The §3.5 MAGSAC++ ρ values were recomputed independently (half-integer incomplete gammas,
no scipy): n2 ρ(1)=0.3232, n4 ρ(1)=0.2849, n4 ρ(2)=0.7461, n4 ρ(3)=0.9084, lumos
loss(1/2/3)=0.3484/0.5677/0.5194 — all matching the quoted figures. The Keys `a=−0.5`
"agrees with the Taylor series to third order" claim and the SIP order/overfitting
consensus were re-checked against fresh web sources.

**One substantive correction (this pass):** §3.5, §9 and §9.8 had described lumos's loss
as "monotone(-increasing)". It is **not** — written `loss = (σ²_max/2)[1 − e^{−x}(1−x)]`,
its derivative `(σ²_max/2)e^{−x}(2−x)` changes sign at `x=2`, so the loss peaks at
`r=2σ_max` (`≈0.568·σ²_max`, which *exceeds* the `0.5·σ²_max` outlier loss) and then
declines to the clamp. The three passages were rewritten to state the loss is *bounded but
non-monotone*, with the analytic peak and its practical (small, fringe-only) consequences.

## 10. References

### Source paths (cloned upstream, read for this document)

> Note: clone paths are under the lumos crate's `.tmp/refs/` (i.e.
> `lumos/.tmp/refs/...`), not a filesystem-root `/.tmp/`. Line numbers below were
> re-verified in pass 2 where cited.

- **astroalign** — invariant-triangle asterism matching, `_invariantfeatures`,
  `_generate_invariants`, `_ransac`, `apply_transform`:
  `/.tmp/refs/astroalign/astroalign.py`.
- **MAGSAC / MAGSAC++** — `run`, `sigmaConsensus`, `sigmaConsensusPlusPlus`,
  `getModelQualityPlusPlus` (the marginalized loss), termination criterion:
  `/.tmp/refs/magsac/src/pymagsac/include/magsac.h`; estimator constants
  (DoF=4, σ-quantile 3.64, C=0.25, gamma values), DEGENSAC:
  `/.tmp/refs/magsac/src/pymagsac/include/estimators.h`.
- **astrometry.net** — SIP polynomial form `sip_calc_distortion` / `sip_distortion`:
  `/.tmp/refs/astrometry.net/util/sip.c`; 4-star quad code geometry
  `codefile_compute_field_code`: `/.tmp/refs/astrometry.net/solver/codefile.c`;
  hypothesis verification / tweak (`solver_tweak2`, logodds, distractor ratio,
  verify_pix, codetol): `/.tmp/refs/astrometry.net/solver/solver.c`,
  `solver/onefield.c`.
- **SCAMP** — PV/TPV distortion keyword handling and `TPV` projection:
  `/.tmp/refs/scamp/src/fitswcs.c`.
- **swarp** — Lanczos2/3/4 windowed-sinc resampling kernels:
  `/.tmp/refs/swarp/src/interpolate.c`, `resample.c`.
- **reproject** — adaptive (DeForest) and flux-conserving reprojection:
  `/.tmp/refs/reproject/reproject/adaptive/`.
- **OpenCV** — DLT/normalization and USAC estimator framework reference:
  `/.tmp/refs/opencv/modules/calib3d/src/`.

### lumos source (this crate)

`src/registration/` — `mod.rs` (register, recover_matches, Auto upgrade, warp),
`triangle/{geometry,matching,voting}.rs`, `ransac/{mod,magsac,transforms}.rs`,
`transform.rs`, `distortion/sip/mod.rs`, `distortion/tps/`, `resample/`, `config.rs`.

### Online sources (verified ≥2 where load-bearing)

- Groth, E.J. 1986, *A pattern-matching algorithm for two-dimensional coordinate
  lists*, AJ 91, 1244 — [ADS](https://ui.adsabs.harvard.edu/abs/1986AJ.....91.1244G/abstract).
- Beroiz, Cabral & Sanchez 2020, *Astroalign* (Astronomy & Computing) — software at
  the cloned repo; method per `astroalign.py`.
- Barath, Noskova & Matas 2019, *MAGSAC: Marginalizing Sample Consensus*, CVPR —
  [CVF PDF](https://openaccess.thecvf.com/content_CVPR_2019/papers/Barath_MAGSAC_Marginalizing_Sample_Consensus_CVPR_2019_paper.pdf),
  [arXiv 1803.07469](https://arxiv.org/abs/1803.07469); MAGSAC++ / *Marginalizing
  Sample Consensus*, TPAMI 2021 — [PubMed 34375281](https://pubmed.ncbi.nlm.nih.gov/34375281/).
- Chum, Matas & Kittler 2003, *Locally Optimized RANSAC* —
  [CMP PDF](https://cmp.felk.cvut.cz/~matas/papers/chum-dagm03.pdf); Lebeda, Matas
  & Chum 2012, *Fixing the LO-RANSAC* —
  [BMVC PDF](https://cmp.felk.cvut.cz/software/LO-RANSAC/Lebeda-2012-Fixing_LORANSAC-BMVC.pdf).
- USAC / PROSAC / GC-RANSAC framework —
  [OpenCV USAC tutorial](https://docs.opencv.org/4.x/de/d3e/tutorial_usac.html);
  Barath et al., *Graph-Cut RANSAC* —
  [SZTAKI PDF](https://eprints.sztaki.hu/10367/1/Barath_4961_31970983_ny.pdf).
- Hartley 1997, *In defence of the 8-point algorithm* —
  [ANU PDF](https://users.cecs.anu.edu.au/~hartley/Papers/fundamental/fundamental.pdf)
  (√2 isotropic normalization, conditioning).
- Umeyama 1991, *Least-squares estimation of transformation parameters between two
  point patterns*, IEEE TPAMI 13:376 —
  [ADS](https://ui.adsabs.harvard.edu/abs/1991ITPAM..13..376U/abstract).
- Shupe et al. 2005, *The SIP Convention for Representing Distortion in FITS Image
  Headers*, ADASS — [FITS registry PDF](https://fits.gsfc.nasa.gov/registry/sip/SIP_distortion_v1_0.pdf);
  SIP order/overfitting — [astrometry mailing list](https://groups.google.com/g/astrometry/c/3y2HoRTNXN8),
  [STScI DrizzlePac](https://www.stsci.edu/itt/review/DrizzlePac/HTML/ch33.html).
- Lanczos resampling & ringing —
  [Wikipedia](https://en.wikipedia.org/wiki/Lanczos_resampling),
  [AstroPixelProcessor interpolation artifacts](https://www.astropixelprocessor.com/community/tutorials-workflows/interpolation-artifacts/),
  [BigGo: image-interpolation math / ringing](https://biggo.com/news/202510120739_Image-Interpolation-Math-Ringing-Artifacts).
- Drizzle (flux conservation) — Fruchter & Hook 2002; Siril drizzle docs —
  [Siril](https://siril.readthedocs.io/en/latest/preprocessing/drizzle.html).
