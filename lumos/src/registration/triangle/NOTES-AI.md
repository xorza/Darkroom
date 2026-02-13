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

### Groth (1986) -- Original Triangle Algorithm

**Paper:** Forms ALL O(n^3) triangles. Characterizes each by two features:
- R = longest/shortest side ratio
- C = cosine of angle between longest and shortest sides
Filters: R < 10, C < 0.99. Vertex labeling: shortest side between vertices 1-2,
intermediate between 2-3, longest between 3-1. Voting: matched triangle pair casts
3 votes (one per vertex pair). Complexity: O(n^4.5).

**Our implementation vs Groth:**
- We use (s0/s2, s1/s2) instead of (R, C). Our invariant is the Valdes (1995) convention,
  not Groth's. Both encode equivalent information (2 DOF per triangle).
- We use KNN-based O(n*k^2) triangle formation instead of O(n^3). Correct optimization.
- We implement Groth's R=10 elongation filter (`geometry.rs:71`). Correct.
- We do NOT implement Groth's C<0.99 cosine filter. Our Heron's formula area check
  (`geometry.rs:81`) serves a similar but not identical purpose -- it rejects flat
  triangles, while C<0.99 rejects triangles where the angle between the two longest
  sides is very small. The area check is actually more robust because it catches all
  degenerate configurations, not just one specific angle.
- Our vertex labeling differs from Groth: we order by "opposite side length" (opp shortest,
  opp middle, opp longest), while Groth orders by "adjacent side" (shortest between 1-2,
  etc.). Both establish deterministic vertex correspondence from sorted side lengths.
  Functionally equivalent for voting purposes.

### Valdes et al. (1995) -- FOCAS

**Paper:** Compressed triangle ratios to (b/a, c/a) where a >= b >= c (i.e., middle/longest,
shortest/longest), both in [0, 1]. Used geometric hashing with 2D bins.

**Our implementation vs Valdes:**
- Our (s0/s2, s1/s2) = (shortest/longest, middle/longest) is the same convention with
  swapped axes. Functionally identical.
- We use k-d tree instead of hash bins. Better: no bin boundary artifacts, exact radius
  queries, O(log n) per query.

### Tabur (2007) -- Optimistic Pattern Matching

**Paper:** Two algorithms (OPM-A, OPM-B).
- OPM-A: Uses (R = longest/shortest, C = cosine of angle between them) -- a cosine metric
  that spreads invariant space more uniformly than pure ratios.
- Key innovation: **ordered search by rarity**. Rarely-occurring triangles (extreme ratios,
  acute angles) are processed first. Few candidates per rare triangle means fast rejection.
- **Optimistic early termination**: each match hypothesis is immediately tested by checking
  if it maps most stars correctly. If yes, accept and return without processing remaining
  candidates.
- Performance: 100% success on 10,063 images, mean 6ms, search time varies <1ms between
  10 and 200 stars.

**Our implementation vs Tabur:**
- We process all triangles in arbitrary order. No rarity-based prioritization.
- We have no early termination -- all triangles vote, then resolve. This is the standard
  Groth/Valdes approach, not Tabur's optimistic approach.
- Tabur's approach is inherently faster for large catalogs because it skips most work
  once a good match is found. Our approach is more robust (full voting) but slower.
- For our use case (50-200 stars, image-to-image), full voting is adequate. Tabur's
  optimization matters for catalog matching (thousands of stars).

### Astroalign (Beroiz et al. 2020)

**Implementation:** Python, scipy, scikit-image.
- KNN with k=5 (4 neighbors + self), C(5,3)=10 triangles per star. Fixed k.
- Invariant: (L2/L1, L1/L0) where L2 >= L1 >= L0. Ratios >= 1 (inverse of our convention).
- K-d tree ball query with r=0.1 (~5% tolerance on ratios >= 1).
- Vertex ordering: `_arrangetriplet` orders by which vertex is at intersection of
  consecutive sorted sides. Functionally equivalent to our geometric-role ordering.
- **No degenerate triangle filtering.** No R>10 filter, no area check.
- RANSAC verification: proposes affine transform from matched triangles, tests reprojection.

**Our implementation vs Astroalign:**
- Our KNN k is adaptive (5-20) vs Astroalign's fixed k=4. We generate 1x-19x more
  triangles per star. More redundancy but higher compute cost.
- Our tolerance (0.01 in [0,1] space) is much tighter than Astroalign's (0.1 in [1,inf)
  space, which is approximately 5% relative tolerance). We may reject valid matches that
  Astroalign accepts.
- We have degenerate triangle filters (R>10, area); Astroalign does not. Advantage: ours.
- We use voting + greedy; Astroalign uses RANSAC on triangle matches. Both work; our
  downstream RANSAC achieves the same effect.

### Astrometry.net (Lang et al. 2010)

**Implementation:** Uses 4-star quads, not triangles.
- Two most distant stars (A, B) define local coordinate frame where A=(0,0), B=(1,1).
- Two inner stars (C, D) have positions (xC, yC, xD, yD) in that frame = 4D hash code.
- 4D invariant space has dramatically higher discriminating power than 2D triangles.
- Bayesian verification: computes odds ratio for match acceptance (threshold: 10^9).
- Designed for blind all-sky solving with pre-indexed catalog.

**Our implementation vs Astrometry.net:**
- Not directly comparable. Astrometry.net solves a harder problem (blind plate solving
  against full-sky catalog). We solve image-to-image registration.
- Their 4D quads would be overkill for our 50-200 star problem but would improve matching
  in dense fields.
- Their Bayesian verification is more principled than our vote counting + RANSAC.

### PixInsight StarAlignment

**Implementation:** Polygon descriptors (quad through octagon, default pentagon).
- Two most distant stars define local coordinate frame.
- Remaining N-2 stars provide 2*(N-2) dimensional hash code.
- Pentagon: 6D (3x discriminating power of triangles). Uncertainty = 1/(N-2) of triangle.
- Cannot handle specular (mirror) transformations -- falls back to triangles.
- RANSAC for outlier rejection.

**Our implementation vs PixInsight:**
- Our 2D triangles have 3x higher uncertainty than PixInsight's default pentagon.
- PixInsight handles local distortion and projective transforms better via polygon
  robustness. Our triangles are only invariant to similarity transforms.
- PixInsight cannot handle mirrors; we can (orientation toggle).

### Siril

**Implementation:** Based on Michael Richmond's `match` program.
- Takes brightest 20 stars, forms ALL N*(N-1)*(N-2)/6 triangles (brute force O(n^3)).
- Uses triangle similarity for matching, then RANSAC.
- Simple but limited to ~20 stars due to cubic complexity.

**Our implementation vs Siril:**
- Our KNN approach scales to 200+ stars; Siril's brute-force is limited to ~20.
- Both use triangle similarity + RANSAC pipeline. Same conceptual architecture.

### GMTV (2022) -- Global Multi-Triangle Voting

- Uses PCA-reduced triangle features for efficient search.
- Weights votes by triangle selectivity (inverse density in feature space).
- Designed for star sensor "lost in space" problem (different domain, but voting
  methodology is relevant).

## Issues Found

### Issue 1: Misleading Test Comments About Rotation and Orientation

**Severity:** Medium (incorrect documentation, tests still pass)
**Location:** `tests.rs:364` and `tests.rs:1050`

```rust
check_orientation: false, // Rotation changes orientation   // WRONG
check_orientation: false, // Must disable for 180 degree rotation  // WRONG
```

Pure rotations (including 90 and 180 degrees) **preserve** orientation (det = +1). Only
reflections flip chirality. The tests use a symmetric pattern (square + center point) that
creates ambiguous triangle correspondences among identical isosceles right triangles, so
the orientation check rejects valid matches to different vertex permutations of the
symmetric pattern. The comments should explain this symmetry issue, not claim rotation
changes orientation.

**Fix:** Correct comments. Add an asymmetric-pattern rotation test that passes with
`check_orientation: true`.

### Issue 2: Default ratio_tolerance (0.01) Is Too Tight for Noisy Data

**Severity:** Medium (may cause missed matches in real data)
**Location:** `mod.rs:36`

With sub-pixel centroid noise (~0.3px), side-length errors for a triangle with sides ~50px
produce ratio errors of ~0.3/50 = 0.006, which is already 60% of the 0.01 tolerance. For
comparison:
- Astroalign uses 0.1 in its (>=1) space, roughly 5% relative tolerance
- For our (0,1] space, an equivalent tolerance would be ~0.02-0.05

The `precise_wide_field` config relaxes to 0.02. The default 0.01 works for well-separated
stars with good centroid accuracy but may drop matches in crowded fields or poor seeing.

**Recommendation:** Increase default to 0.02. Document that 0.01 is for high-precision use.

### Issue 3: Adaptive k Produces Excessive Triangles

**Severity:** Low (performance, not correctness)
**Location:** `matching.rs:60`

```rust
let k_neighbors = (n_ref.min(n_target) / 3).clamp(5, 20);
```

For 150 stars: k=20, producing C(20,2)=190 triangles per star. After deduplication via
HashSet (~28,500 inserts, ~5,000-10,000 unique), this is 10-19x more than Astroalign's
fixed C(5,3)=10. The extra triangles improve vote statistics but with diminishing returns.

**Recommendation:** Benchmark with k=8 or k=10. Astroalign achieves good results with k=5.

### Issue 4: HashSet Deduplication Overhead

**Severity:** Low (performance)
**Location:** `matching.rs:108,127`

For k=20, neighboring stars share many neighbors, so >50% of triangle insertions are
duplicates. HashSet hashing overhead is non-trivial. Alternative: collect all sorted
triples into a Vec, sort, dedup. Or generate triangles only when the central star has the
smallest index among the three -- avoids duplicates by construction.

### Issue 5: Duplicate Issue Number in Previous NOTES-AI.md

**Severity:** Cosmetic. Previous version had two "Issue 4" entries. Fixed in this version.

## What We Do Correctly

1. **KNN triangle formation** (`matching.rs:99-133`): O(n*k^2) instead of Groth's O(n^3).
   Standard optimization used by Astroalign and modern implementations.

2. **Valdes (1995) invariant convention** (`geometry.rs:62-75`): (s0/s2, s1/s2) in [0,1]
   is the canonical reference formulation. Equivalent to Groth's (R, C) with different axes.

3. **Groth R=10 elongation filter** (`geometry.rs:71`): Rejects very elongated triangles
   where small perturbations cause large ratio changes. Matches the original paper.

4. **Heron's formula degeneracy check** (`geometry.rs:78-83`): More robust than Groth's
   C<0.99 cosine filter. Catches all near-degenerate triangles, not just one angle.

5. **K-d tree invariant lookup** (`voting.rs:96-102`): O(log n) per query, no bin boundary
   artifacts. Better than Valdes' hash bins. Same approach as Astroalign.

6. **L2/L-inf radius correction** (`voting.rs:127`): Search radius multiplied by sqrt(2) so
   L2 ball circumscribes L-inf square. Prevents missed matches at corners.

7. **Deterministic vertex ordering** (`geometry.rs:89-93`): Reorder by geometric role
   (opposite shortest/middle/longest side) with tiebreak on original index. All 6 input
   permutations produce identical output (verified by test).

8. **Orientation check** (`voting.rs:142`): Optional CW/CCW filter correctly rejects
   mirror-image matches when enabled. Not found in Astroalign.

9. **Dense/sparse vote matrix** (`voting.rs:22-89`): Auto-switching at 250K entries.
   Dense u16 for small counts (fast indexing), sparse HashMap<(usize,usize), u32> for large.
   Practical engineering not found in reference implementations.

10. **Greedy conflict resolution** (`voting.rs:163-210`): Sort by votes, one-to-one
    assignment. Standard approach matching Groth/Valdes.

## What We Do Not Do But Should Consider

### 1. Weighted Voting by Triangle Rarity
**Priority:** Medium
**Location:** `voting.rs:148-153`

All votes have equal weight. Near-equilateral triangles are extremely common in uniform
distributions and pollute the vote matrix with false correspondences. Tabur (2007) addresses
this by processing rare triangles first. GMTV (2022) weights by selectivity.

**Implementation:** For each reference triangle, count how many other reference triangles
fall within the tolerance radius (density). Weight = 1/density. Store weight in Triangle
struct. Multiply vote increment by weight (requires changing u16/u32 to f32 in VoteMatrix).

### 2. Quad Descriptors (Architecture-Level)
**Priority:** Low (only needed for dense fields or blind solving)

Triangle descriptors have 2 DOF. Quads (Astrometry.net) have 4 DOF -- 2x discriminating
power. Pentagons (PixInsight) have 6 DOF -- 3x. False positive rate drops exponentially
with dimensionality.

For our use case (50-200 stars, image-to-image, downstream RANSAC), triangles are adequate.
Quads would help if:
- Dense Milky Way fields (>500 stars, many similar triangles)
- Very noisy centroids (need higher-DOF descriptors to disambiguate)
- Blind solving (need to match against large catalogs)

### 3. Tabur-Style Ordered Search
**Priority:** Low (optimization, not correctness)

Process triangles sorted by distance from the dense center of invariant space. Rare
triangles first = fewer candidates = faster rejection. Add early termination when enough
high-confidence matches are found. Only matters for large star counts or real-time use.

### 4. Global Triangle Selection
**Priority:** Low

Add a few triangles from the 4-5 brightest stars regardless of spatial proximity. These
"global" triangles span the full field and provide coarse registration anchors. Helps when
KNN-local triangles are ambiguous (clustered or gridded star fields).

## What We Do Unnecessarily

Nothing identified. The implementation is lean. The only arguable excess is the high k_neighbors
for moderate star counts (Issue 3), which generates more triangles than necessary but does
not add algorithmic complexity -- just constant-factor overhead.

## Prioritized Improvements

1. **Fix test comments** (Issue 1) -- Trivial, prevents confusion.
2. **Increase default tolerance to 0.02** (Issue 2) -- One-line change, improves robustness.
3. **Reduce k or benchmark k=8** (Issue 3) -- Profile-guided, may cut triangle count 4x.
4. **Weighted voting** (Missing Feature 1) -- Medium effort, meaningful quality improvement
   for clustered/gridded fields.
5. **Replace HashSet dedup with sort+dedup** (Issue 4) -- Minor perf, optional.
6. **Quad descriptors** (Missing Feature 2) -- Large effort, only if dense fields are a problem.

## References

- Groth, E. J. (1986). "A pattern-matching algorithm for two-dimensional coordinate lists."
  *Astronomical Journal*, 91, 1244-1248.
  [ADS](https://ui.adsabs.harvard.edu/abs/1986AJ.....91.1244G/abstract)

- Valdes, F. G., Campusano, L. E., Velasquez, J. D., & Stetson, P. B. (1995).
  "FOCAS Automatic Catalog Matching Algorithms." *PASP*, 107, 1119.
  [ADS](https://ui.adsabs.harvard.edu/abs/1995PASP..107.1119V/abstract)

- Tabur, V. (2007). "Fast Algorithms for Matching CCD Images to a Stellar Catalogue."
  *PASA*, 24, 189-198.
  [arXiv](https://arxiv.org/abs/0710.3618) |
  [Cambridge](https://www.cambridge.org/core/journals/publications-of-the-astronomical-society-of-australia/article/fast-algorithms-for-matching-ccd-images-to-a-stellar-catalogue/DF8B193A1321CA334C6070F8DBBCB6EA)

- Lang, D., Hogg, D. W., Mierle, K., Blanton, M., & Roweis, S. (2010).
  "Astrometry.net: Blind astrometric calibration of arbitrary astronomical images."
  *Astronomical Journal*, 139, 1782-1800.
  [arXiv](https://arxiv.org/abs/0910.2233)

- Beroiz, M., Cabral, J. B., & Sanchez, B. (2020). "Astroalign: A Python module
  for astronomical image registration." *Astronomy and Computing*, 32, 100384.
  [arXiv](https://arxiv.org/abs/1909.02946) |
  [GitHub](https://github.com/quatrope/astroalign)

- PixInsight StarAlignment. "Arbitrary Distortion Correction with StarAlignment."
  [Tutorial](https://www.pixinsight.com/tutorials/sa-distortion/index.html)

- GMTV (2022). "A Star-Identification Algorithm Based on Global Multi-Triangle Voting."
  *Applied Sciences*, 12(19), 9993.
  [MDPI](https://www.mdpi.com/2076-3417/12/19/9993)

- Siril registration documentation.
  [Docs](https://siril.readthedocs.io/en/latest/preprocessing/registration.html)

- LSST MatchOptimisticBTask (Tabur OPM-B implementation).
  [LSST](https://pipelines.lsst.io/modules/lsst.meas.astrom/index.html) |
  [Python OPM-B](https://github.com/morriscb/PythonOptimisticPatternMatcherB)
