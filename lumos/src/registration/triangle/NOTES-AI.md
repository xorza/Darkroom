# Triangle Matching Module - Research Notes

## Overview

This module implements the Groth (1986) / Valdes (1995) triangle voting algorithm for
star pattern matching. Triangles are formed from star positions using k-nearest neighbors,
characterized by scale-invariant side ratios, indexed in a k-d tree, and matched via a
vote accumulation scheme.

**Files:**
- `mod.rs` - `TriangleParams` (ratio_tolerance=0.01, min_votes=3, check_orientation=true)
- `geometry.rs` - `Triangle` struct: side ratio invariants, orientation, degeneracy filters
- `matching.rs` - Triangle formation via KNN k-d tree, `match_triangles()` entry point
- `voting.rs` - Vote matrix (dense/sparse), invariant k-d tree, correspondence voting
- `tests.rs` - 65+ tests covering transforms, noise, edge cases, stress tests

## Algorithm Flow

1. Build spatial k-d tree on star positions
2. For each star, find k nearest neighbors, form C(k,2) triangles from pairs of neighbors
3. Compute invariant ratios (shortest/longest, middle/longest) per triangle
4. Build 2D k-d tree on reference triangle ratios
5. For each target triangle, radius search in invariant tree to find similar reference triangles
6. Matching triangles cast 3 votes each for vertex pair correspondences
7. Greedy conflict resolution: sort by votes descending, assign one-to-one

## Comparison with Industry Tools

| Aspect | This Crate | Astroalign | Astrometry.net | PixInsight |
|--------|-----------|------------|----------------|------------|
| **Descriptor type** | Triangle (3-star) | Triangle (3-star) | Quad (4-star) | Polygon (4-8 star, default pentagon) |
| **Invariant** | (s0/s2, s1/s2) | (s2/s1, s1/s0) | (xC, yC, xD, yD) in local frame | N-2 star coords in local frame |
| **Invariant dimensions** | 2 | 2 | 4 | 2*(N-2) |
| **Neighbors per star** | adaptive k = clamp(n/3, 5, 20) | 4 (fixed, 5 incl. self) | N/A (pre-indexed catalog) | N/A |
| **Triangles per star** | C(k,2) = 10..190 | C(5,3) = 10 | N/A | N/A |
| **Invariant lookup** | k-d tree radius search | k-d tree ball query (r=0.1) | Geometric hash table | Hash table |
| **Matching tolerance** | 0.01 (L-inf) | 0.1 (L2) | Bayesian odds | Hash bin tolerance |
| **Vertex correspondence** | Geometric-role order | Geometric-role order | Built into hash | Built into hash |
| **Orientation check** | Optional (default on) | Not explicit | N/A (4D hash) | Cannot handle mirrors |
| **Outlier rejection** | Vote threshold + greedy | RANSAC on transform | Bayesian verification | RANSAC |
| **Degenerate filter** | R>10 rejection + area | None documented | Quad geometry constraints | Polygon constraints |
| **Mirror handling** | Orientation toggle | Implicit | Implicit (hash) | Falls back to triangles |

## Issues Found

### Issue 0: Orientation Test Comments Incorrectly State Rotation Changes Orientation

**Severity:** Medium (misleading documentation)

**Location:** `tests.rs:365` and `tests.rs:1050`

```rust
check_orientation: false, // Rotation changes orientation   // <-- WRONG
check_orientation: false, // Must disable for 180 degree rotation  // <-- WRONG
```

Pure rotations (including 90 and 180 degrees) **preserve** orientation (determinant = +1).
Only reflections change chirality. The tests pass with `check_orientation: false` because
the symmetric test pattern (a square + center point) creates ambiguous triangle
correspondences -- many isosceles right triangles from the square have identical shapes,
so the orientation check rejects valid matches that map to different vertex permutations
of the symmetric pattern.

**Fix:** Correct the comments to explain symmetric pattern ambiguity. Add a test with an
asymmetric star pattern that verifies rotation works with `check_orientation: true`.

### Issue 1: Invariant Ratio Convention Differs from Literature

**Severity:** Low (functionally equivalent but confusing)

**Location:** `geometry.rs:70`

This crate computes `(sides[0]/sides[2], sides[1]/sides[2])` = (shortest/longest,
middle/longest), both in [0, 1].

Astroalign computes `(sides[2]/sides[1], sides[1]/sides[0])` = (longest/middle,
middle/shortest), both >= 1.

Valdes (1995) FOCAS uses `(b/a, c/a)` where a >= b >= c, i.e., (middle/longest,
shortest/longest), both in [0, 1].

All three encode the same geometric information but occupy different regions of 2D
space. This crate's convention is valid; the only concern is that the ratio tolerance
of 0.01 should be calibrated to the specific convention being used. With ratios in
[0,1], 0.01 is quite tight (1% of full range). Astroalign uses 0.1 on ratios >= 1,
which is approximately 5% relative tolerance.

### Issue 2: k-d Tree Radius Uses L2, Similarity Check Uses L-infinity

**Severity:** Low (conservative, no false negatives)

**Location:** `voting.rs:131-134`

The k-d tree `radius_indices_into()` uses L2 (Euclidean) distance, but `is_similar()`
uses L-infinity (per-axis max). The L-inf square is inscribed in the L2 circle, so
L-inf is stricter. However, points at L-inf boundary corners (e.g., `(t, t)` from
query, L2 distance `t*sqrt(2)`) are NOT in the L2 ball of radius `t`. This means
valid L-inf matches at the corners are missed. Effective tolerance in worst case is
`ratio_tolerance / sqrt(2)`.

**Fix:** Either multiply the k-d tree radius by `sqrt(2)`:
```rust
invariant_tree.radius_indices_into(query, params.ratio_tolerance * SQRT_2, &mut candidates);
```
Or implement L-infinity radius search in the k-d tree (see spatial/NOTES-AI.md).

### Issue 3: Adaptive k_neighbors May Be Too Aggressive

**Severity:** Low

**Location:** `matching.rs:60`

```rust
let k_neighbors = (n_ref.min(n_target) / 3).clamp(5, 20);
```

For 150 stars (typical pipeline limit), k = clamp(50, 5, 20) = 20, producing
C(20, 2) = 190 triangles per star. For 500 stars, still k=20 so 190 triangles/star.
For 15 stars, k = clamp(5, 5, 20) = 5, producing C(5, 2) = 10 triangles/star.

Astroalign always uses 4 neighbors (5 including self), producing C(5,3) = 10
triangles per star regardless of star count. The C(k,2) formula here counts
pairs of neighbors combined with the central star to form a triangle -- so for
k=20 neighbors, each star forms up to C(20, 2) = 190 triangles. This is 19x
more triangles per star than Astroalign's fixed 10.

More triangles means better vote statistics but O(n*k^2) complexity. At k=20 and
n=150, that is 150 * 190 = 28,500 triangles (before deduplication via HashSet).
This is manageable but much larger than Astroalign's ~1500 triangles for the same
star count.

### Issue 4: HashSet Deduplication is Wasteful for Large k

**Severity:** Low (performance, not correctness)

**Location:** `matching.rs:108,127`

Triangle indices are deduplicated via `HashSet<[usize; 3]>`. Each star forms
triangles from its k neighbors, and neighboring stars share many neighbors,
producing many duplicate triangles. For k=20, the HashSet can see ~28,500 inserts
but keep only ~5,000-10,000 unique triangles. The hashing overhead is non-trivial.

Alternative: Sort all triangle triples at the end and dedup, or use a more
targeted triangle generation strategy that avoids duplicates by construction.

### Issue 4: Default ratio_tolerance May Be Too Tight

**Severity:** Low

**Location:** `mod.rs:36` (0.01)

With sub-pixel centroid noise, star position errors propagate to side-length errors,
then to ratio errors. For a triangle with sides ~50px and centroid error ~0.3px,
ratio error is approximately `0.3/50 = 0.006` -- already 60% of the 0.01 tolerance.
Combined with the L2/L-inf mismatch (Issue 2), effective tolerance drops to ~0.007.
The `precise_wide_field` config already relaxes to 0.02.

**Recommendation:** Consider increasing default to 0.02, or document that 0.01 is
calibrated for well-separated stars with good centroid accuracy.

## Missing Features

### 1. Weighted Voting by Triangle Rarity

**Priority:** Medium

**Location:** `voting.rs:143-149`

All triangle votes have equal weight. Near-equilateral triangles are extremely
common in uniform star distributions and dominate the vote matrix with false
correspondences. Tabur (2007) explicitly addresses this by processing rare triangles
first. The GMTV algorithm (2022) weights by selectivity score.

**Recommendation:** Weight each vote by inverse density in invariant space.
Pre-compute weight per reference triangle (count neighbors within tolerance, invert).
Particularly helps with clustered/gridded star fields.

### 2. Quad/Polygon Descriptors

Triangle descriptors have only 2 degrees of freedom (2D invariant space), making
them prone to false matches in dense star fields. Industry has moved to:

- **Quads (Astrometry.net):** 4D hash code from 4 stars. Two most distant stars
  define a local (0,0)-(1,1) coordinate frame; other two stars' positions in that
  frame form the hash. 4D = 2x the discriminating power of triangles. False positive
  rate drops dramatically.

- **Polygons (PixInsight):** N-star descriptors with 2*(N-2) dimensional hash codes.
  Default pentagon has 6D = 3x triangle discriminating power. Uncertainty of a
  quad is half that of a triangle; pentagon is one-third. Supports quadrilaterals
  through octagons.

Triangle matching works for small-to-medium fields (<200 stars) but struggles with
dense star fields (many similar triangle shapes), wide-field distortion (triangle
shapes change across field), and high contamination rates (>50% spurious detections).

### 2. Proper Bayesian Verification

Astrometry.net uses a Bayesian decision process to verify matches: given a quad
match, predict where other stars should appear, and compute the odds ratio. This
provides rigorous false-positive control (default threshold: 10^9 odds). The current
implementation relies only on vote counts and a minimum vote threshold, which is
less principled.

### 4. Global Triangle Selection Strategy

The current implementation uses only local KNN triangles. Astrometry.net pre-selects
specific star configurations that maximize discriminating power across the entire
field. Adding a few globally-formed triangles (e.g., from the 4-5 brightest stars)
could improve matching robustness for small star counts.

## Potential Improvements

### Priority 1: Fix L2/L-inf Tolerance Mismatch
Multiply k-d tree radius by `sqrt(2)` or implement L-inf search. One-line fix.

### Priority 2: Fix Orientation Test Comments
Correct misleading comments about rotation changing orientation.

### Priority 3: Upgrade to Quad Descriptors
Replace 2D triangle invariants with 4D quad hash codes. This would:
- Halve false match rate compared to triangles
- Enable matching in denser star fields
- Eliminate need for orientation check (quads encode chirality implicitly)
- Match PixInsight/Astrometry.net capability level

Implementation sketch:
1. For each star, take k nearest neighbors
2. For each pair of neighbors (A, B) with A-B being the longest edge, compute local
   frame where A=(0,0), B=(1,1)
3. For each additional pair (C, D) from remaining neighbors, compute (xC, yC, xD, yD)
4. Sort C, D coordinates lexicographically (canonicalize)
5. Index quads in 4D k-d tree
6. Match with radius search as now

### Priority 4: Consider Fixed k Like Astroalign
The adaptive k formula produces 19x more triangles than Astroalign for 150 stars.
Consider benchmarking with k=5 (matching Astroalign) to see if quality holds with
dramatically fewer triangles and faster runtime.

### Priority 5: Pre-filter by Star Brightness
Both Astrometry.net and PixInsight weight their star selection by brightness.
Forming triangles only from the brightest N stars (already done upstream in the
pipeline per NOTES-AI.md) is correct, but the triangle formation could additionally
prioritize triangles that include bright stars.

## Implementation Quality Assessment

**Strengths:**
- K-d tree for spatial queries is efficient and standard (O(n*k^2) vs O(n^3))
- K-d tree for invariant lookup is a good choice (O(log n) per query)
- Dense/sparse vote matrix auto-switching at 250K entries is practical engineering
- Elongation filter (R>10) matches Groth 1986 recommendation
- Area-based degeneracy check (Heron's formula) is more robust than cross-product alone
- Orientation check is optional (correctly rejects mirror-image matches when enabled)
- Vertex ordering by geometric role (opposite shortest/middle/longest side)
- Comprehensive test suite (65+ tests, including noise, outliers, stress tests)
- Greedy conflict resolution is standard and adequate

**Weaknesses:**
- Only 2D invariant space (triangles); industry uses 4-8D (quads/polygons)
- No verification beyond vote counting (no Bayesian or geometric consistency check)
- Adaptive k produces excessive triangles for moderate star counts
- No weighted voting (all triangle votes equal regardless of shape rarity)

## References

- Groth, E. J. (1986). "A pattern-matching algorithm for two-dimensional coordinate
  lists." *Astronomical Journal*, 91, 1244-1248.
  [ADS](https://ui.adsabs.harvard.edu/abs/1986AJ.....91.1244G/abstract)

- Valdes, F. G., Campusano, L. E., Velasquez, J. D., & Stetson, P. B. (1995).
  "FOCAS Automatic Catalog Matching Algorithms." *PASP*, 107, 1119.
  [ADS](https://ui.adsabs.harvard.edu/abs/1995PASP..107.1119V/abstract)

- Lang, D., Hogg, D. W., Mierle, K., Blanton, M., & Roweis, S. (2010).
  "Astrometry.net: Blind astrometric calibration of arbitrary astronomical images."
  *Astronomical Journal*, 139, 1782-1800.
  [Astrometry.net](https://astrometry.net/summary.html)

- Beroiz, M., Cabral, J. B., & Sanchez, B. (2020). "Astroalign: A Python module
  for astronomical image registration." *Astronomy and Computing*, 32, 100384.
  [arXiv](https://arxiv.org/abs/1909.02946) |
  [GitHub](https://github.com/quatrope/astroalign)

- Tabur, V. (2007). "Fast Algorithms for Matching CCD Images to a Stellar Catalogue."
  *PASA*, 24, 189-198. [arXiv](https://arxiv.org/abs/0710.3618)

- PixInsight StarAlignment documentation. "Arbitrary Distortion Correction with
  StarAlignment."
  [Tutorial](https://www.pixinsight.com/tutorials/sa-distortion/index.html)

- GMTV (2022). "Global Multi-Triangle Voting" star identification.
  [MDPI](https://www.mdpi.com/2076-3417/12/19/9993)
