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
- `tests.rs` -- 40+ tests: transforms, noise, outliers, edge cases, parameter sensitivity

## Algorithm Description

### Step 1: Triangle Formation (`matching.rs:102-138`)
Build spatial k-d tree on star positions. For each star i, find k nearest neighbors.
For each pair of neighbors (j, k), form triangle (i, j, k). Normalize to sorted indices
[min, mid, max] and deduplicate via sort+dedup. Adaptive k = clamp(min(n_ref, n_target)/3, 5, 10).

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
longest and shortest sides. Filters: R < 10 (some implementations use R < 8), C < 0.99.
Voting: each matched triangle pair casts 3 votes, one per vertex correspondence. Vertex
labeling reduces false matches by factor of 6: vertices A-B define shortest side, B-C
longest, A-C intermediate.

**vs ours:** We use Valdes (s0/s2, s1/s2) convention instead of (R, C) -- equivalent 2 DOF.
KNN O(n*k^2) formation vs O(n^3). We implement R=10 filter but NOT C<0.99. Our Heron's
area check covers most cases but has a small gap (see Issue 5). Vertex labeling differs
(opposite-side vs adjacent-side) but functionally equivalent for voting.

### Valdes et al. (1995) -- FOCAS

Ratios (b/a, c/a) in [0,1] with geometric hash bins. Our (s0/s2, s1/s2) is the same
convention. K-d tree is better than hash bins (no boundary artifacts). Valdes took a
subsample of brightest objects for initial matching, then matched all objects using the
derived transformation -- same as our pipeline (triangle match -> RANSAC -> recovery).

### Tabur (2007) -- Optimistic Pattern Matching

OPM-A uses a cosine metric that reduces density in invariant space, and rarity-ordered
search: rare triangles processed first (fewer candidates = fast rejection). Optimistic
early termination when good match found. 100% success on 10,063 images, mean 6ms. Lists
of 10-200 points. **vs ours:** We process all triangles in arbitrary order, no early
termination. Full voting is more robust but slower. Adequate for our 50-200 star use case.

### Astroalign (Beroiz et al. 2020)

Python. Fixed k=5 (NUM_NEAREST_NEIGHBORS=5 including self), C(5,3)=10 triangles/star.
Invariant: (L2/L1, L1/L0) = (longest/middle, middle/shortest) -- ratios >= 1. K-d tree
ball query r=0.1 (~5% relative tolerance). No explicit degeneracy filters (duplicates
removed). RANSAC on triangle matches directly: each iteration picks one triangle match,
tests remaining against transformation. Acceptance: 80% of matches or 10 (whichever lower).
MAX_CONTROL_POINTS=50.
**vs ours:** Our k is adaptive (5-10), tolerance tighter (0.01 vs ~5%), we have degeneracy
filters. We use voting+greedy; they use RANSAC on triangle matches directly.

### Astrometry.net (Lang et al. 2010)

4-star quads: two most distant stars (A, B) define a local coordinate system where (A,B)
maps to origin and (1,1). Positions of stars C, D in this frame give 4D hash code
(xC, yC, xD, yD). Hash code is invariant under translation, rotation, and scale.
Dramatically higher discriminating power than triangles. Bayesian verification (log-odds
test). 99.9% success rate for blind all-sky solving with no false positives.
Not directly comparable to our image-to-image use case.

### PixInsight StarAlignment

Polygon descriptors (default pentagon = 6D, 3x discriminating power of triangles). A
polygon with N stars associates N-2 triangles, so quad = 2 triangles = half the
uncertainty. RANSAC. Cannot handle mirrors (falls back to triangles for mirrored images).
v1.9.0 added TPS distortion correction.
**vs ours:** Higher discriminating power but we handle mirrors via orientation toggle.

### Siril (v1.4-1.5)

Brute-force O(n^3) on brightest 20 stars. Triangle similarity + RANSAC. Based on Michael
Richmond's match program. v1.4 added 5th-order SIP distortion, Drizzle, Gaia DR3.
**vs ours:** Same pipeline, our KNN scales to 200+ stars while Siril is limited to ~20.

### ASTAP (Han Kleijn)

Tetrahedron (4-star) hash: takes 500 brightest stars, creates ~300 tetrahedron figures.
6 distances between 4 stars, normalized by largest, yield 5 ratios (5D). ~2.5x
discriminating power vs triangles. Hash-based indexing. SIP distortion.

### starmatch (PyPI)

Supports triangle and quad modes. HEALPix hierarchical blind matching with pre-divided
celestial sphere. Multi-pass tolerances. GPR distortion calibration.
**vs ours:** Single fixed tolerance; multi-pass could improve robustness.

### GMTV (2022) -- Global Multi-Triangle Voting

PCA-based feature extraction from triangle units (more noise-robust than raw ratios).
2D lookup table + fuzzy match strategy. PCA invariants are rotation-invariant and robust
to noise. K-vector accelerated search. **Weighted voting** by selectivity: rare triangles
get higher weight, common near-equilateral triangles get lower weight.
**vs ours:** We use uniform weights; selectivity weighting would reduce false matches.

## Issues Found

### Issue 1: Default ratio_tolerance (0.01) May Be Too Tight
**Severity:** Medium | **Location:** `mod.rs:36`

Sub-pixel centroid noise (~0.3px) on 50px sides gives ratio errors of ~0.006 = 60% of
tolerance. Astroalign uses ~5% relative tolerance (r=0.1 in their unit space). Our
`precise_wide_field` preset relaxes to 0.02. The tight default works for clean data with
well-separated stars but may miss valid matches in noisy or crowded fields.
**Note:** The current value is conservative by design -- downstream RANSAC handles the
case where some true matches are missed. A tighter tolerance means fewer false positives
in the vote matrix, so this is a precision vs recall tradeoff.
**Recommendation:** Consider increasing default to 0.015 or 0.02.

### Issue 2: Missing Groth C<0.99 Cosine Filter
**Severity:** Low | **Location:** `geometry.rs:68-83`

Gap: a triangle with sides (5, 4, 1) has R=5 < 10, area^2=3.9 >> 1e-6, but
cos(angle_between_longest_and_shortest)=1.0 >= 0.99. Passes our filters, rejected by
Groth. Rare in KNN sets because KNN produces compact local triangles rather than
long-thin ones. Astroalign uses no filters at all and works fine.
**If desired:** `let cos_v1 = (s0*s0 + s2*s2 - s1*s1) / (2.0*s0*s2); if cos_v1 > 0.99 { return None; }`

### Issue 3: No Configurable k_neighbors
**Severity:** Low | **Location:** `matching.rs:63`

The k value is hardcoded as `clamp(min(n_ref, n_target)/3, 5, 10)`. This is not exposed
in `TriangleParams`. For dense fields a higher k might help; for sparse fields lower k
saves computation. The current adaptive formula is reasonable for the 50-200 star range.
Not a bug, but inflexible.

### Issue 4: VoteMatrix Dense Overflow Check Is debug_assert Only
**Severity:** Low | **Location:** `voting.rs:61-67`

The dense vote matrix uses u16 with saturating_add, and the overflow check is only a
debug_assert. In release mode, votes silently saturate at 65535. For typical use (200
stars, k=10), max votes per pair ~= 45 triangles/star = well under u16::MAX. The
saturating_add is intentional (safe behavior), and the debug_assert catches logic errors
during development. No actual bug, but worth noting.

## What We Do Correctly

1. **KNN triangle formation** -- O(n*k^2) vs Groth's O(n^3). Standard modern approach
   (same as Astroalign). Scales to 200+ stars.
2. **Valdes invariant convention** -- (s0/s2, s1/s2) in [0,1]. Canonical formulation,
   same as Siril and standard references.
3. **R=10 elongation filter** -- Matches Groth's original paper. Rejects triangles where
   small perturbations cause large ratio changes.
4. **Heron's area check** -- Catches near-collinear triangles. Numerically stable for
   rejection. Small gap vs C<0.99 (Issue 2) but rare in KNN-formed triangles.
5. **K-d tree invariant lookup** -- O(log n) radius queries, no bin boundary artifacts.
   Better than Valdes hash bins and Siril brute-force.
6. **L2/L-inf sqrt(2) correction** -- Mathematically correct: L2 ball radius = tolerance
   * sqrt(2) circumscribes the L-inf tolerance square. Ensures no valid candidates missed.
7. **Deterministic vertex ordering** -- Geometric role (opposite shortest/middle/longest
   side) + index tiebreak for equal sides. All 6 input permutations produce identical
   output. Tests verify this explicitly.
8. **Orientation check** -- Optional CW/CCW filter prevents mirror-image matches. Not
   found in Astroalign (they use affine model), not found in Siril. Useful for
   same-camera registration where mirrors are not expected.
9. **Dense/sparse vote matrix** -- Auto-switching at 250K entries. Dense for direct O(1)
   indexing on small sets, sparse HashMap for memory efficiency on large sets. Practical
   optimization not in reference implementations.
10. **Greedy resolution** -- Standard across Groth, Valdes, LSST, Kolomenkin (2008),
    multilayer voting (2021). Hungarian O(n^3) gives negligible improvement when
    downstream RANSAC handles outliers.
11. **Sort+dedup for triangle deduplication** -- More cache-friendly and lower overhead
    than HashSet for the ~50% duplication rate typical of KNN-formed triangles.
12. **No unnecessary complexity** -- Implementation is lean. No redundant two-step
    refinement (removed; RANSAC handles this). No hash table (replaced by k-d tree).

## What We Should Consider Adding

1. **Weighted voting by triangle rarity** (Medium priority) -- GMTV weights by 1/density
   in feature space. Reduces false matches from common near-equilateral triangles. Could
   be computed cheaply: count how many reference triangles fall within each triangle's
   tolerance radius, use inverse as weight.

2. **Quad descriptors** (Low) -- 4 DOF vs 2 DOF. Higher discriminating power. Used by
   Astrometry.net (4D hash), ASTAP (5D hash), starmatch, PixInsight (N-gon). Only needed
   for dense fields or when false positive rate from triangles is too high.

3. **Tabur-style ordered search** (Low) -- Process rare triangles first + early
   termination when sufficient matches found. Tabur achieves 6ms on 200 stars. Our
   full voting is fast enough for current use case but this could help at scale.

4. **Global triangles from brightest stars** (Low) -- Form a few triangles from the 5-10
   brightest stars (large-scale structure) in addition to KNN local triangles. Provides
   coarse anchors for ambiguous local KNN neighborhoods.

## What We Do NOT Need

Nothing identified as unnecessary. The implementation follows industry standards without
excess complexity. The only arguable overhead is forming more triangles than strictly
necessary when k=10 for moderate star counts, but this is a constant factor, not an
algorithmic issue.

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
- ASTAP. [Algorithm](https://www.hnsky.org/astap_astrometric_solving.htm)
- starmatch. [PyPI](https://pypi.org/project/starmatch/)
