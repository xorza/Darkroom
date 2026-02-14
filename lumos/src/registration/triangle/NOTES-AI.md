# Triangle Matching Module

## Architecture

Implements the Groth (1986) / Valdes (1995) triangle voting algorithm for star pattern
matching. Triangles formed from star positions using k-nearest neighbors, characterized
by scale-invariant side ratios, indexed in a k-d tree, matched via vote accumulation.

**Files:**
- `mod.rs` -- `TriangleParams` (ratio_tolerance=0.01, min_votes=3, check_orientation=true)
- `geometry.rs` -- `Triangle` struct: side ratio invariants, orientation, degeneracy filters
- `matching.rs` -- Triangle formation via KNN k-d tree, `match_triangles()` entry point
- `voting.rs` -- Vote matrix (dense/sparse), invariant k-d tree, correspondence voting
- `tests.rs` -- 40+ tests: transforms, noise, outliers, edge cases, stress tests

## Algorithm Description

### Step 1: Triangle Formation (`matching.rs:99-133`)
Build spatial k-d tree on star positions. For each star i, find k nearest neighbors.
For each pair of neighbors (j, k), form triangle (i, j, k). Normalize to sorted indices
[min, mid, max] and deduplicate via `HashSet`. Adaptive k = clamp(min(n_ref, n_target)/3, 5, 20).

### Step 2: Invariant Computation (`geometry.rs:32-117`)
For each triangle:
1. Compute three side lengths: d01, d12, d20
2. Sort sides ascending: sides[0] <= sides[1] <= sides[2]
3. Reject if longest/shortest > 10 (Groth R=10 filter)
4. Reject if area^2 < 1e-6 (Heron's formula, flat triangle filter)
5. Compute invariant ratios: (sides[0]/sides[2], sides[1]/sides[2]) -- both in (0, 1]
6. Reorder vertex indices by geometric role: [opp_shortest, opp_middle, opp_longest]
7. Compute orientation (CW/CCW) from cross product of reordered vertices

### Step 3: Invariant Indexing (`voting.rs:96-102`)
Build 2D k-d tree on reference triangle ratios for O(log n) radius queries.

### Step 4: Voting (`voting.rs:111-157`)
For each target triangle, radius search in invariant tree (L2 radius = tolerance * sqrt(2)
to circumscribe L-inf square). For each candidate, L-inf filter + optional orientation
check. Matching triangles cast 3 votes: vertex[i] in ref maps to vertex[i] in target
(same geometric role = same sorted position).

### Step 5: Resolution (`voting.rs:163-210`)
Filter by min_votes. Sort by vote count descending. Greedy one-to-one assignment.
Confidence = votes / max_votes_in_set.

## Comparison with Industry Standards

| Tool/Paper | Descriptor | DOF | Key Difference |
|---|---|---|---|
| Groth (1986) | Triangle (R, C) | 2 | O(n^3) formation, C<0.99 filter we lack |
| Valdes (1995) | Triangle (b/a, c/a) | 2 | Same convention, hash bins vs our k-d tree |
| Tabur (2007) | Triangle (R, C) | 2 | Rarity-ordered search + early termination |
| Astroalign | Triangle ratios | 2 | Fixed k=5, RANSAC not voting, no degeneracy filters |
| Astrometry.net | 4-star quad | 4 | 2x discriminating power, blind solving |
| PixInsight | N-gon (default 5) | 6 | 3x discriminating power, TPS distortion |
| ASTAP | 4-star tetrahedron | 5 | 5D hash, flip-invariant |
| starmatch | Triangle + quad | 2/4 | Multi-pass tolerances, HEALPix, GPR distortion |
| GMTV (2022) | Triangle PCA | 2 | Weighted voting by selectivity, k-vector search |
| Siril | Triangle | 2 | Brute-force O(n^3) on brightest 20, same pipeline |
| LSST | Pairs (OPM-B) | - | Not triangle-based; pair matching with consistency |

### Groth (1986) -- Original Triangle Algorithm

Forms ALL O(n^3) triangles. Features: R = longest/shortest, C = cosine of angle between
longest and shortest sides. Filters: R < 10, C < 0.99. Voting: matched pair casts 3 votes.

**vs ours:** We use Valdes (s0/s2, s1/s2) convention instead of (R, C) -- equivalent 2 DOF.
KNN O(n*k^2) formation vs O(n^3). We implement R=10 filter but NOT C<0.99. Our Heron's
area check covers most cases but has a small gap (see Issue 5). Vertex labeling differs
(opposite-side vs adjacent-side) but functionally equivalent for voting.

### Valdes et al. (1995) -- FOCAS

Ratios (b/a, c/a) in [0,1] with geometric hash bins. Our (s0/s2, s1/s2) is the same
convention with swapped axes. K-d tree is better than hash bins (no boundary artifacts).

### Tabur (2007) -- Optimistic Pattern Matching

OPM-A uses rarity-ordered search: rare triangles processed first (fewer candidates = fast
rejection). Optimistic early termination when good match found. 100% success on 10,063
images, mean 6ms. **vs ours:** We process all triangles in arbitrary order, no early
termination. Full voting is more robust but slower. Adequate for our 50-200 star use case.

### Astroalign (Beroiz et al. 2020)

Python. Fixed k=5, C(5,3)=10 triangles/star. Invariant: (L2/L1, L1/L0) >= 1. K-d tree
ball query r=0.1 (~5% relative tolerance). No degeneracy filters. RANSAC verification.
**vs ours:** Our k is adaptive (5-20), tolerance tighter (0.01 vs ~5%), we have degeneracy
filters. We use voting+greedy; they use RANSAC on triangle matches directly.

### Astrometry.net (Lang et al. 2010)

4-star quads: two most distant stars define frame, two inner stars give 4D hash code.
Dramatically higher discriminating power. Bayesian verification (odds > 10^9). Designed
for blind all-sky solving. Not directly comparable to our image-to-image use case.

### PixInsight StarAlignment (v1.9.0, Dec 2024)

Polygon descriptors (default pentagon = 6D, 3x discriminating power of triangles). RANSAC.
v1.9.0 added TPS distortion correction. Cannot handle mirrors (falls back to triangles).
**vs ours:** Higher discriminating power but we handle mirrors via orientation toggle.

### Siril (v1.4, 2024-2025)

Brute-force O(n^3) on brightest 20 stars. Triangle similarity + RANSAC. v1.4 added
5th-order SIP distortion, Drizzle, Gaia DR3. **vs ours:** Same pipeline, our KNN scales
to 200+ stars while Siril is limited to ~20.

### ASTAP (Han Kleijn)

Tetrahedron (4-star) hash: 6 distances normalized by largest yield 5 ratios (5D). ~2.5x
discriminating power. Hash-based indexing. SIP distortion. 2024: 20% speed improvement.

### starmatch (PyPI 2024-2025)

Supports triangle and quad modes. HEALPix hierarchical blind matching. Multi-pass
tolerances (60px/20px/3px). GPR distortion calibration. **vs ours:** Single fixed
tolerance; multi-pass could improve robustness.

### GMTV (2022) -- Global Multi-Triangle Voting

PCA-based feature extraction (more noise-robust than raw ratios). K-vector accelerated
search. **Weighted voting** by selectivity (1/density): rare triangles get higher weight.
Largest cluster verification. **vs ours:** We use uniform weights; selectivity weighting
would reduce false matches from common near-equilateral triangles.

## Issues Found

### Issue 1: Misleading Test Comments About Rotation and Orientation
**Severity:** Medium | **Location:** `tests.rs:364` and `tests.rs:1050`

```rust
check_orientation: false, // Rotation changes orientation   // WRONG
check_orientation: false, // Must disable for 180 degree rotation  // WRONG
```

Pure rotations preserve orientation (det = +1). The tests use a symmetric pattern (square
+ center) creating ambiguous correspondences among identical isosceles right triangles.
The orientation check rejects valid matches to different vertex permutations of the
symmetric pattern -- it's a symmetry issue, not a rotation issue.
**Fix:** Correct comments. Add asymmetric-pattern rotation test with `check_orientation: true`.

### Issue 2: Default ratio_tolerance (0.01) Is Too Tight
**Severity:** Medium | **Location:** `mod.rs:36`

Sub-pixel centroid noise (~0.3px) on 50px sides gives ratio errors of ~0.006 = 60% of
tolerance. Astroalign uses ~5% relative tolerance. Our `precise_wide_field` relaxes to 0.02.
**Recommendation:** Increase default to 0.02.

### Issue 3: Adaptive k Produces Excessive Triangles
**Severity:** Low | **Location:** `matching.rs:60`

For 150 stars: k=20, C(20,2)=190 triangles/star. After dedup: 5K-10K unique. This is
10-19x more than Astroalign's C(5,3)=10. Diminishing returns past k~8-10.

### Issue 4: HashSet Deduplication Overhead
**Severity:** Low | **Location:** `matching.rs:108,127`

>50% duplicate inserts for large k. Alternative: Vec sort+dedup, or generate only when
central star has smallest index (avoids duplicates by construction).

### Issue 5: Missing Groth C<0.99 Cosine Filter
**Severity:** Low | **Location:** `geometry.rs:68-83`

Gap: triangle (5, 4, 1) has R=5 < 10, area^2=3.9 >> 1e-6, but cos(angle)=1.0 >= 0.99.
Passes our filters, rejected by Groth. Rare in KNN sets; Astroalign uses no filters at all.
**If desired:** `let cos_v1 = (s0*s0 + s2*s2 - s1*s1) / (2.0*s0*s2); if cos_v1 > 0.99 { return None; }`

## What We Do Correctly

1. **KNN triangle formation** -- O(n*k^2) vs Groth's O(n^3). Standard modern approach.
2. **Valdes invariant convention** -- (s0/s2, s1/s2) in [0,1]. Canonical formulation.
3. **R=10 elongation filter** -- Matches Groth's original paper.
4. **Heron's area check** -- Catches near-collinear triangles. Numerically adequate for
   rejection (cancellation inflates area = safe direction). Small gap vs C<0.99 (Issue 5).
5. **K-d tree invariant lookup** -- O(log n), no bin boundary artifacts. Better than hash bins.
6. **L2/L-inf sqrt(2) correction** -- Mathematically correct circumscription.
7. **Deterministic vertex ordering** -- Geometric role + index tiebreak. All 6 permutations
   produce identical output.
8. **Orientation check** -- Optional CW/CCW filter. Not found in Astroalign.
9. **Dense/sparse vote matrix** -- Auto-switching at 250K. Not in reference implementations.
10. **Greedy resolution** -- Standard across Groth, Valdes, LSST, Kolomenkin (2008),
    multilayer voting (2021). Hungarian O(n^3) gives negligible improvement with RANSAC.

## What We Should Consider Adding

1. **Weighted voting by triangle rarity** (Medium priority) -- GMTV weights by 1/density
   in feature space. Reduces false matches from common near-equilateral triangles.
2. **Quad descriptors** (Low) -- 4 DOF vs 2 DOF. Only needed for dense fields or blind solving.
3. **Tabur-style ordered search** (Low) -- Rare triangles first + early termination.
4. **Global triangles from brightest stars** (Low) -- Coarse anchors for ambiguous KNN fields.

## What We Do Unnecessarily

Nothing identified. Implementation is lean. Only arguable excess is high k for moderate
star counts (Issue 3) -- constant-factor overhead, not algorithmic complexity.

## Prioritized Improvements

1. **Fix test comments** (Issue 1) -- Trivial, prevents confusion.
2. **Increase default tolerance to 0.02** (Issue 2) -- One-line change, improves robustness.
3. **Reduce k or benchmark k=8** (Issue 3) -- May cut triangle count 4x.
4. **Weighted voting** -- Medium effort, meaningful quality improvement.
5. **Replace HashSet dedup** (Issue 4) -- Minor perf, optional.
6. **Add C<0.99 filter** (Issue 5) -- One-line, marginal improvement.
7. **Quad descriptors** -- Large effort, only if dense fields are a problem.

## References

- Groth (1986). "A pattern-matching algorithm for two-dimensional coordinate lists."
  *AJ*, 91, 1244. [ADS](https://ui.adsabs.harvard.edu/abs/1986AJ.....91.1244G/abstract)
- Valdes et al. (1995). "FOCAS Automatic Catalog Matching." *PASP*, 107, 1119.
  [ADS](https://ui.adsabs.harvard.edu/abs/1995PASP..107.1119V/abstract)
- Tabur (2007). "Fast Algorithms for Matching CCD Images." *PASA*, 24, 189.
  [arXiv](https://arxiv.org/abs/0710.3618)
- Lang et al. (2010). "Astrometry.net: Blind astrometric calibration." *AJ*, 139, 1782.
  [arXiv](https://arxiv.org/abs/0910.2233)
- Beroiz et al. (2020). "Astroalign." *A&C*, 32, 100384.
  [GitHub](https://github.com/quatrope/astroalign)
- Kolomenkin & Pollak (2008). "Geometric voting algorithm for star trackers."
  *IEEE TAES*, 44(2), 441-456.
- GMTV (2022). "Global Multi-Triangle Voting." *Applied Sciences*, 12(19), 9993.
  [MDPI](https://www.mdpi.com/2076-3417/12/19/9993)
- Multilayer Voting (2021). *Sensors*, 21(9), 3084.
  [PMC](https://pmc.ncbi.nlm.nih.gov/articles/PMC8124596/)
- Rijlaarsdam et al. (2020). "Survey of Lost-in-Space Star ID." *Sensors*, 20(9), 2579.
  [PMC](https://pmc.ncbi.nlm.nih.gov/articles/PMC7248786/)
- Dense Star Scene (2024). *Remote Sensing*, 16(11), 2035.
  [MDPI](https://www.mdpi.com/2072-4292/16/11/2035)
- PixInsight StarAlignment. [Tutorial](https://www.pixinsight.com/tutorials/sa-distortion/)
- Siril. [Docs](https://siril.readthedocs.io/en/latest/preprocessing/registration.html)
- LSST MatchOptimisticBTask (pairs, not triangles).
  [Source](https://github.com/lsst/meas_astrom/blob/main/src/matchOptimisticB.cc)
