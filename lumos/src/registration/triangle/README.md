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
| Invariant lookup | Hash table (100x100 bins) | k-d tree on invariant space | Brute-force | Sorted search |
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

4. **Hash table approach** — O(1) lookup per triangle vs astroalign's k-d tree O(log n). For the typical workload (200 stars, ~2000 triangles), hash table is faster.

5. **Two-step refinement** — Transform-guided re-voting with position proximity boost. More sophisticated than single-pass voting.

6. **Orientation check** — Rejects mirror-image matches. Useful for typical same-camera registration. Astroalign handles this implicitly via affine model.

### Suggested Improvements

#### 1. Replace hash table with k-d tree on invariant space (simplification)

**Current**: 2D hash table with `bins x bins` grid. Requires tolerance-based neighbor search scanning adjacent bins.

**Alternative**: k-d tree on 2D invariant space (like astroalign).

**Pros**:
- Eliminates bin boundary artifacts (hash table can miss matches at bin edges despite tolerance search).
- Cleaner code: removes hash_table.rs entirely, uses existing KdTree infrastructure.
- Natural nearest-neighbor queries with exact distance thresholds.
- No `hash_bins` parameter to tune.

**Cons**:
- O(log n) per lookup vs O(1) for hash table.
- For 2000 triangles, difference is negligible (~11 comparisons vs 1 + bin scan).

**Verdict**: Consider. Simplifies code and removes a parameter. Performance difference is negligible for typical workloads. The hash table's tolerance-based bin scanning already approximates k-d tree behavior anyway.

#### 2. Use astroalign's invariant formulation: (L2/L1, L1/L0) instead of (L0/L2, L1/L2)

**Current**: `(shortest/longest, middle/longest)` — both ratios in (0, 1].

**Astroalign**: `(longest/middle, middle/shortest)` — both ratios in [1, inf).

The astroalign formulation spreads out the invariant space more uniformly. With (s0/s2, s1/s2), both values are squeezed into (0,1], and many different triangles cluster near (1,1) (near-equilateral). The astroalign formulation spreads these out since L2/L1 and L1/L0 can range from 1 to large values.

**However**: The current formulation is the same as Valdes (1995), the canonical reference. Astroalign's formulation makes sense for k-d tree matching (unbounded range works fine) but is awkward for hash table indexing (unbounded range needs clamping or log-scaling). If staying with hash tables, current formulation is better. If switching to k-d tree, consider astroalign's.

**Verdict**: Keep current if using hash table. Reconsider if switching to k-d tree.

#### 3. Triangle deduplication

**Current**: No deduplication. The same triangle can appear multiple times if formed from different starting vertices during k-NN enumeration.

**Astroalign**: Explicitly removes duplicate invariant tuples after triangle formation.

Duplicate triangles inflate vote counts uniformly across all vertex pairs of that triangle, so they don't create false positives. However, they waste hash lookups and voting computation.

**Implementation**: Sort triangle index triples (i,j,k) and deduplicate with a HashSet before computing invariants.

**Verdict**: Worth doing. Reduces triangle count (and thus matching work) by estimated 2-3x based on k-NN overlap patterns. Simple to implement.

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

#### 7. Groth's side-ratio filter

**Current**: No filter on triangle shape. All non-degenerate triangles are kept.

**Groth (1986)**: Rejects triangles with longest/shortest > 10 (very elongated). These produce imprecise matches because small perturbations in the shortest side cause large ratio changes.

**Siril**: Similar filtering of elongated triangles.

**Current code**: Already rejects via `MIN_TRIANGLE_AREA_SQ` (Heron's formula), which catches some elongated triangles. But a direct ratio filter (e.g., s2/s0 > 10) would be more targeted.

**Implementation**: Add `if sides[2] / sides[0] > 10.0 { return None; }` in `Triangle::from_positions`.

**Verdict**: Worth adding. Simple, improves match quality by excluding unstable triangles. Groth's R=10 threshold is well-established.

#### 8. Confidence formula improvement

**Current**: `confidence = votes / ((min(n_ref, n_target) - 2) * (min(n_ref, n_target) - 1) / 2)`. This is the theoretical maximum votes if a point appeared in every possible triangle pair.

This formula doesn't account for the k-NN triangle formation limiting triangle count. The actual max votes per point is much lower than the formula suggests, so all confidences are very small.

**Better formula**: Compute confidence relative to the maximum vote count in the current match set: `confidence = votes / max_votes_in_set`. This gives a relative ranking that's more meaningful.

**Verdict**: Worth fixing. The current formula produces misleadingly small confidence values. A simple `votes / max_votes` would be more useful to downstream consumers.

#### 9. Sparse VoteMatrix: use u32 instead of usize for values

The sparse HashMap uses `usize` (8 bytes on 64-bit) for vote counts that never exceed ~1000. Using `u32` would halve the per-entry memory overhead in the HashMap.

**Verdict**: Minor optimization. Low priority.

## Summary of Recommendations

### High priority (clear benefit, low risk)

1. **Triangle deduplication** — HashSet on sorted index triples. Reduces matching work 2-3x.
2. **Groth's side-ratio filter** — Reject triangles with s2/s0 > 10. Simple, proven.
3. **Confidence formula fix** — Use `votes / max_votes_in_set` instead of theoretical maximum.

### Medium priority (meaningful simplification, needs benchmarking)

4. **Consider removing two-step refinement** — Overlaps with downstream RANSAC. Benchmark first.
5. **Consider k-d tree for invariant lookup** — Eliminates hash_table.rs and hash_bins parameter. Simpler, no bin boundary artifacts.

### Low priority (nice-to-have)

6. **Tabur-style ordered search** — Process rare triangles first. Only matters for large point sets.
7. **Astroalign invariant formulation** — Only relevant if switching to k-d tree lookup.
8. **Sparse VoteMatrix u32 values** — Minor memory savings.

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
