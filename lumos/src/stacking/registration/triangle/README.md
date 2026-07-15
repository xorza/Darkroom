# Triangle Matching

Triangle matching converts two star-position catalogs into candidate one-to-one point
correspondences for RANSAC. All implementation types are internal; `TriangleConfig` is the public
stage configuration embedded in `RegistrationMatchingConfig`.

## Flow

1. Build a `spatial::KdTree` over each catalog.
2. For every point, form triangles from pairs among its nearest neighbors. The neighbor count is
   `min(reference_len, target_len) / 3`, clamped to 5–10.
3. Sort and deduplicate vertex-index triples.
4. Reject degenerate, nearly flat, and highly elongated triangles.
5. Describe each triangle by `(shortest / longest, middle / longest)` plus orientation.
6. Build a second 2D k-d tree over the reference invariants.
7. Radius-query candidates for every target invariant, apply the exact per-axis tolerance and
   optional orientation gate, then cast votes for corresponding vertices.
8. Filter by `min_votes`, sort deterministically, and greedily enforce one reference and one target
   occurrence per match.

Dense `Vec<u16>` vote storage is used below 250,000 reference×target entries. Larger matrices use a
sparse `HashMap<(usize, usize), u32>`.

## Configuration

| Field | Default | Meaning |
|-------|---------|---------|
| `ratio_tolerance` | `0.01` | Maximum difference on each invariant axis |
| `min_votes` | `3` | Required supporting triangle votes |
| `check_orientation` | `true` | Reject mirrored triangle pairs |

The registration-level matching config separately owns `max_stars`, `min_stars`, and
`min_matches`.

## Layout

- `geometry.rs`: triangle construction, vertex roles, invariants, and degeneracy gates.
- `matching.rs`: k-neighbor formation and stage orchestration.
- `voting.rs`: invariant queries, dense/sparse votes, and deterministic conflict resolution.
- `tests.rs`: geometry, storage boundary, matching, and deterministic resolution tests.

RANSAC remains a separate stage. Triangle votes supply correspondence confidence; robust transform
estimation and unmatched-star recovery are owned by `ransac/` and `registration/mod.rs`.
