# Triangle Matching: Research & Comparison

## Literature Survey

### Foundational Papers

**Groth (1986)** — "A pattern-matching algorithm for two-dimensional coordinate lists"
- Original triangle matching algorithm for astronomical image registration.
- Forms all O(n^3) triangles from n points, characterizes by side ratios.
- Voting scheme: each matched triangle pair casts 3 votes (one per vertex pair).
- Vertex labeling: shortest side between A-B, intermediate B-C, longest C-A. Reduces false matches by factor of 6.
- Filters: rejects triangles with length ratio R > 10 and cos(vertex1) > 0.99.
- Complexity: O(n^4.5) matching phase.
- Tolerates 25% non-overlapping points between the two lists.

**Valdes et al. (1995)** — "FOCAS automatic catalog matching algorithms"
- Compressed all triangle ratios into (0, 1] range using (s0/s2, s1/s2).
- Used geometric hashing with 2D bins on the ratio space.
- Basis for most modern hash-table-based triangle matchers.

**Tabur (2007)** — "Fast Algorithms for Matching CCD Images to a Stellar Catalogue"
- Two algorithms: Optimistic Pattern Matcher A and B (OPM-A, OPM-B).
- Key innovations:
  - **Cosine metric**: describes triangles by (longest/shortest ratio, dot-product cosine of acute angle), reducing density in invariant space.
  - **Ordered search**: processes rarely-occurring triangles first (acute-angle, outlying in invariant space). Few candidates means fast rejection.
  - **Optimistic early termination**: tests each hypothesized transform immediately. If it maps most stars correctly, accept and return. Avoids processing the full candidate set.
- Performance: 100% success on 10,063 images, mean 6ms (2.4 GHz, 2007 hardware). Search time varies < 1ms between 10 and 200 stars.
- Used by LSST pipeline (MatchOptimisticBTask).

**Lang et al. (2010)** — "Astrometry.net: Blind astrometric calibration"
- Uses **quadrilateral** asterisms instead of triangles for blind plate solving.
- 4D hash code from 4-star quads: higher discriminating power, fewer false positives.
- Bayesian verification for hypothesis acceptance.
- Designed for full-sky blind solving where triangle false-positive rate is too high.
- Not directly comparable to image-to-image registration (different problem domain).

**Beroiz et al. (2020)** — "Astroalign: A Python Module for Astronomical Image Registration"
- Modern Python implementation, closest comparable to our module.
- Detailed implementation analysis below.

### Implementation Comparison

| Feature | Current (Scenarium) | Astroalign | Siril | Tabur OPM |
|---|---|---|---|---|
| Max points | 200 | 50 | 20 | ~50 |
| Triangle formation | k-NN (k=5..20, adaptive) | k-NN (k=4, fixed) | All O(n^3) | Brightest NSET |
| Triangle invariant | (s0/s2, s1/s2) | (s2/s1, s1/s0) | (s0/s2, s1/s2) | (ratio, cosine) |
| Invariant lookup | k-d tree on invariant space | k-d tree on invariant space | Brute-force | Sorted search |
| Matching strategy | Voting + greedy resolution | RANSAC on triangle matches | Triangle + RANSAC | Optimistic early-accept |
| Refinement | Two-step transform-guided | RANSAC re-fit with all inliers | RANSAC outlier rejection | Immediate verification |
| Orientation check | Yes (configurable) | No (uses affine) | No | No |
| Tolerance | 1% ratio | ~5% (r=0.1 in unit space) | Not documented | Adaptive |
| Transform model | Similarity (in refinement) | Affine similarity | Similarity/Affine/Homography | Similarity |

## Analysis of Current Implementation

### What Works Well

1. **K-d tree triangle formation** — O(n*k^2) instead of Groth's O(n^3). Good choice for handling 200 stars.

2. **Adaptive k-neighbors** — `k = clamp(min(n_ref, n_target) / 3, 5, 20)` adapts to point density. Astroalign uses fixed k=4 which limits triangle diversity for larger fields.

3. **Dense/sparse vote matrix** — Automatic switch at 250K entries balances memory vs performance. Practical optimization not found in reference implementations.

4. **K-d tree on invariant space** — Same approach as astroalign, with exact radius queries and no bin boundary artifacts. Uses existing KdTree infrastructure with buffer-based `radius_indices_into` for zero-allocation candidate lookup in the voting loop.

5. **Two-step refinement** — Transform-guided re-voting with position proximity boost. More sophisticated than single-pass voting.

6. **Orientation check** — Rejects mirror-image matches. Useful for typical same-camera registration. Astroalign handles this implicitly via affine model.

### Suggested Improvements

#### 1. ~~Replace hash table with k-d tree on invariant space~~ (DONE)

Replaced the 2D hash table with a k-d tree on invariant space. Removed `hash_table.rs` entirely, removed `hash_bins` config parameter. Uses existing `KdTree` infrastructure with a new `radius_indices_into` method for buffer-based candidate lookup. Eliminates bin boundary artifacts and simplifies the codebase.

#### 2. Use astroalign's invariant formulation: (L2/L1, L1/L0) instead of (L0/L2, L1/L2)

**Current**: `(shortest/longest, middle/longest)` — both ratios in (0, 1].

**Astroalign**: `(longest/middle, middle/shortest)` — both ratios in [1, inf).

The astroalign formulation spreads out the invariant space more uniformly. With (s0/s2, s1/s2), both values are squeezed into (0,1], and many different triangles cluster near (1,1) (near-equilateral). The astroalign formulation spreads these out since L2/L1 and L1/L0 can range from 1 to large values.

**However**: The current formulation is the same as Valdes (1995), the canonical reference. Both formulations cluster near-equilateral triangles at (1,1), so the claimed spread improvement is minimal. More importantly, a fixed tolerance in (0,1] space gives uniform relative precision, while in [1,inf) space a fixed tolerance is relatively tighter near 1.0 and looser for elongated triangles — arguably worse.

**Verdict**: Rejected. Current Valdes formulation works well with fixed-tolerance radius queries. No practical benefit from switching.

#### 3. ~~Triangle deduplication~~ (ALREADY DONE)

`form_triangles_from_neighbors` in `spatial/mod.rs` already sorts index triples and deduplicates via `HashSet` before returning. No duplicate triangles reach the matching stage.

#### 4. RANSAC integration into the triangle module (or clarify pipeline boundary)

**Current**: Triangle module does voting + greedy resolution + optional two-step refinement. Then the pipeline feeds results to a separate RANSAC module.

**Astroalign**: RANSAC is integral to the matching — each triangle match proposes a transform, immediately verified against other matches.

**Tabur OPM**: Similar — each candidate is immediately tested, accepted if it maps most stars correctly.

The current two-step approach (voting then RANSAC) is actually a reasonable design for a pipeline architecture where triangle matching and RANSAC are separate concerns. The two-step refinement in the triangle module partially fills the role that RANSAC plays in astroalign.

**Verdict**: Current architecture is fine. The two-step refinement + downstream RANSAC gives two layers of outlier rejection which is robust. No change needed.

#### 5. Remove two-step refinement (simplification)

The two-step refinement in matching.rs is complex (~80 lines) and overlaps with the downstream RANSAC stage:
- Phase 1: voting with normal tolerance
- Phase 2: estimate transform from initial matches, re-vote with 0.5x tolerance and position-proximity bonus
- Fallback: if refined count < initial count, keep initial

Since the downstream RANSAC already handles outlier rejection and transform estimation, the two-step refinement is doing redundant work. The benefit is marginal: it may produce slightly better matches before RANSAC, but RANSAC is specifically designed to handle noisy initial matches.

**Verdict**: Consider removing. Simplifies the triangle module significantly. Would need benchmarking on real data to confirm no regression. The two-step code can always be re-added if needed. At minimum, make it default-off if keeping it.

#### 6. Tabur-style ordered search (optimization)

**Current**: Processes all target triangles in arbitrary order.

**Tabur**: Processes triangles ordered by decreasing rarity in invariant space. Rarely-occurring triangles (acute, extreme ratios) have fewer candidates, so they're cheaper to process and more discriminating.

**Implementation**: After forming target triangles, sort by distance from center of invariant space (0.5, 0.5 for our formulation or the dense region). Process outliers first. Add early termination when enough high-confidence matches are found.

**Verdict**: Nice optimization for large point sets, but adds complexity. Current approach is fast enough for 200 stars. Low priority unless profiling shows voting is a bottleneck.

#### 7. ~~Groth's side-ratio filter~~ (DONE)

Added `if longest / sides[0] > 10.0 { return None; }` in `Triangle::from_positions`. Rejects very elongated triangles where small perturbations in the shortest side cause large ratio changes (Groth 1986, R=10 threshold). Three tests cover rejection, acceptance, and boundary behavior.

#### 8. ~~Confidence formula improvement~~ (DONE)

Changed from theoretical maximum formula to `votes / max_votes_in_set`. The top match now gets confidence 1.0, and others are proportional. More meaningful for downstream consumers.

#### 9. ~~Sparse VoteMatrix: use u32 instead of usize for values~~ (DONE)

Changed sparse `HashMap` value type from `usize` (8 bytes) to `u32` (4 bytes). Converted to `usize` in `iter_nonzero()`. Also eliminated the `into_hashmap()` conversion entirely — `resolve_matches` now takes `VoteMatrix` directly and iterates via `iter_nonzero()`.

## Summary of Recommendations

### Done

- **K-d tree for invariant lookup** — Replaced hash table with k-d tree on invariant space. Eliminated `hash_table.rs` and `hash_bins` parameter.
- **Triangle deduplication** — Already implemented in `form_triangles_from_neighbors` via sorted index triples and `HashSet`.
- **Groth's side-ratio filter** — Reject triangles with longest/shortest > 10 in `Triangle::from_positions`.
- **Confidence formula fix** — `votes / max_votes_in_set` instead of theoretical maximum.
- **Sparse VoteMatrix u32 values** — `u32` instead of `usize` for sparse HashMap values.
- **Eliminated `into_hashmap()` conversion** — `resolve_matches` takes `VoteMatrix` directly via `iter_nonzero()`.
- **Added `#[derive(Debug)]` to VoteMatrix** — Per project coding rules.
- **Fixed doc comments** — `two_step_matching` config comment now matches actual behavior.

### Rejected

- **Astroalign invariant formulation** — No practical benefit over Valdes (1995). Both cluster at (1,1); fixed tolerance works better in bounded (0,1] space.

### Remaining suggestions

1. **Consider removing two-step refinement** — Overlaps with downstream RANSAC. Benchmark first.
2. **Tabur-style ordered search** — Process rare triangles first. Only matters for large point sets.

## References

- [Groth 1986 — "A pattern-matching algorithm for two-dimensional coordinate lists"](https://ui.adsabs.harvard.edu/abs/1986AJ.....91.1244G/abstract)
- [Valdes et al. 1995 — "FOCAS automatic catalog matching algorithms"](https://ui.adsabs.harvard.edu/abs/1995PASP..107.1119V/abstract)
- [Tabur 2007 — "Fast Algorithms for Matching CCD Images to a Stellar Catalogue"](https://arxiv.org/abs/0710.3618)
- [Lang et al. 2010 — "Astrometry.net: Blind astrometric calibration"](https://arxiv.org/abs/0910.2233)
- [Beroiz et al. 2020 — "Astroalign: A Python Module for Astronomical Image Registration"](https://arxiv.org/abs/1909.02946)
- [Astroalign source code](https://github.com/quatrope/astroalign)
- [Siril registration documentation](https://siril.readthedocs.io/en/latest/preprocessing/registration.html)
- [LSST MatchOptimisticBTask (Tabur OPM-B implementation)](https://pipelines.lsst.io/modules/lsst.meas.astrom/index.html)
- [GMTV — Global Multi-Triangle Voting for star identification](https://www.mdpi.com/2076-3417/12/19/9993)
