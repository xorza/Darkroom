use super::*;
use std::collections::HashMap;

#[test]
fn vote_matrix_dense_mode() {
    // 10*10 = 100 < 250,000 → dense
    let mut vm = VoteMatrix::new(10, 10);
    assert!(matches!(vm, VoteMatrix::Dense { .. }));

    vm.increment(0, 0);
    vm.increment(0, 0);
    vm.increment(5, 7);

    let entries: Vec<_> = vm.nonzero_entries();
    let get = |r, t| entries.iter().find(|e| e.0 == r && e.1 == t).map(|e| e.2);
    assert_eq!(get(0, 0), Some(2));
    assert_eq!(get(5, 7), Some(1));
    assert_eq!(entries.len(), 2);
}

#[test]
fn vote_matrix_sparse_mode() {
    // 600*600 = 360,000 >= 250,000 → sparse
    let mut vm = VoteMatrix::new(600, 600);
    assert!(matches!(vm, VoteMatrix::Sparse(_)));

    vm.increment(0, 0);
    vm.increment(0, 0);
    vm.increment(100, 200);

    let entries: Vec<_> = vm.nonzero_entries();
    let get = |r, t| entries.iter().find(|e| e.0 == r && e.1 == t).map(|e| e.2);
    assert_eq!(get(0, 0), Some(2));
    assert_eq!(get(100, 200), Some(1));
    assert_eq!(entries.len(), 2);
}

#[test]
fn vote_matrix_empty() {
    let vm_dense = VoteMatrix::new(5, 5);
    assert_eq!(vm_dense.nonzero_entries().len(), 0);

    let vm_sparse = VoteMatrix::new(600, 600);
    assert_eq!(vm_sparse.nonzero_entries().len(), 0);
}

#[test]
fn vote_matrix_threshold_boundary() {
    // size < 250,000 → dense, size >= 250,000 → sparse

    // 499*500 = 249,500 < 250,000 → dense
    let vm_below = VoteMatrix::new(499, 500);
    assert!(matches!(vm_below, VoteMatrix::Dense { .. }));

    // 500*500 = 250,000, not < 250,000 → sparse
    let vm_at = VoteMatrix::new(500, 500);
    assert!(matches!(vm_at, VoteMatrix::Sparse(_)));
}

#[test]
fn vote_matrix_dense_index_mapping() {
    // Verify that dense mode correctly maps (ref_idx, target_idx) → flat index
    // Formula: flat_idx = ref_idx * n_target + target_idx
    let n_ref = 3;
    let n_target = 4;
    let mut vm = VoteMatrix::new(n_ref, n_target);

    // Set specific cells with different vote counts to verify index mapping
    // (0,0) → idx 0, (0,3) → idx 3, (1,2) → idx 6, (2,0) → idx 8, (2,3) → idx 11
    vm.increment(0, 0); // 1 vote at (0,0)
    vm.increment(0, 3);
    vm.increment(0, 3); // 2 votes at (0,3)
    vm.increment(1, 2);
    vm.increment(1, 2);
    vm.increment(1, 2); // 3 votes at (1,2)
    vm.increment(2, 0); // 1 vote at (2,0)
    vm.increment(2, 3);
    vm.increment(2, 3);
    vm.increment(2, 3);
    vm.increment(2, 3); // 4 votes at (2,3)

    let entries: Vec<_> = vm.nonzero_entries();
    let get = |r, t| entries.iter().find(|e| e.0 == r && e.1 == t).map(|e| e.2);

    assert_eq!(get(0, 0), Some(1));
    assert_eq!(get(0, 3), Some(2));
    assert_eq!(get(1, 2), Some(3));
    assert_eq!(get(2, 0), Some(1));
    assert_eq!(get(2, 3), Some(4));
    assert_eq!(entries.len(), 5);
}

#[test]
fn vote_matrix_dense_boundary_indices() {
    // Test accessing corners: (0,0), (0,n-1), (n-1,0), (n-1,n-1)
    let n = 10;
    let mut vm = VoteMatrix::new(n, n);
    vm.increment(0, 0);
    vm.increment(0, n - 1);
    vm.increment(n - 1, 0);
    vm.increment(n - 1, n - 1);

    let entries: Vec<_> = vm.nonzero_entries();
    let get = |r, t| entries.iter().find(|e| e.0 == r && e.1 == t).map(|e| e.2);
    assert_eq!(get(0, 0), Some(1));
    assert_eq!(get(0, n - 1), Some(1));
    assert_eq!(get(n - 1, 0), Some(1));
    assert_eq!(get(n - 1, n - 1), Some(1));
    assert_eq!(entries.len(), 4);
}

#[test]
fn vote_matrix_dense_saturating_add() {
    // Dense mode uses u16. Verify exact count for reasonable values.
    let mut vm = VoteMatrix::new(2, 2);
    for _ in 0..1000 {
        vm.increment(0, 0);
    }
    let entries: Vec<_> = vm.nonzero_entries();
    let votes = entries.iter().find(|e| e.0 == 0 && e.1 == 0).unwrap().2;
    assert_eq!(votes, 1000);
}

/// Greedy resolution, case by case: the pairs that survive `min_votes` are taken in descending
/// vote order, each star at most once, and each kept pair's confidence is its votes over the most
/// any kept pair drew. Every confidence below is a correctly rounded quotient, so equality is exact.
/// Vote entries, `min_votes`, and the resolved `(reference, target, confidence)` in output order.
type ResolveCase = (
    &'static [(usize, usize, usize)],
    usize,
    &'static [(usize, usize, f64)],
);

#[test]
fn resolve_matches_claims_each_star_once_by_votes() {
    let cases: [ResolveCase; 7] = [
        // No conflicts: all three, 10/10, 8/10, 6/10.
        (
            &[(0, 0, 10), (1, 1, 8), (2, 2, 6)],
            1,
            &[(0, 0, 1.0), (1, 1, 0.8), (2, 2, 0.6)],
        ),
        // Ref 0 takes target 0 first, so ref 1 falls back to target 1 with 3 votes.
        (
            &[(0, 0, 10), (1, 0, 5), (1, 1, 3)],
            1,
            &[(0, 0, 1.0), (1, 1, 0.3)],
        ),
        // Ref 0 is taken by its 10-vote pair, so its 5-vote pair is dropped.
        (
            &[(0, 0, 10), (0, 1, 5), (1, 1, 3)],
            1,
            &[(0, 0, 1.0), (1, 1, 0.3)],
        ),
        // Only the 10-vote pair clears `min_votes = 3`.
        (&[(0, 0, 10), (1, 1, 2), (2, 2, 1)], 3, &[(0, 0, 1.0)]),
        (
            &[(0, 0, 20), (1, 1, 10), (2, 2, 5)],
            1,
            &[(0, 0, 1.0), (1, 1, 0.5), (2, 2, 0.25)],
        ),
        (&[(0, 0, 10)], 1, &[(0, 0, 1.0)]),
        (&[], 1, &[]),
    ];
    for (entries, min_votes, expected) in cases {
        let matches = resolve_matches(vote_matrix_from_entries(5, 5, entries), 5, 5, min_votes);
        let resolved: Vec<(usize, usize, f64)> = matches
            .iter()
            .map(|m| (m.indices.reference, m.indices.target, m.confidence))
            .collect();
        assert_eq!(resolved, expected, "{entries:?} with min_votes {min_votes}");
    }
}

#[test]
fn vote_for_correspondences_identical_triangles() {
    // Identical point sets → every triangle matches itself → diagonal dominates
    let positions = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(0.0, 10.0),
        DVec2::new(10.0, 10.0),
        DVec2::new(5.0, 5.0),
    ];

    let triangles = triangles_of(&positions, 4);
    assert!(!triangles.is_empty());
    let invariant_tree = build_invariant_tree(&triangles).unwrap();

    let config = TriangleConfig::default();
    let vm = vote_for_correspondences(
        &triangles,
        &triangles,
        &invariant_tree,
        &config,
        positions.len(),
        positions.len(),
    );

    let votes: HashMap<(usize, usize), usize> = vm
        .nonzero_entries()
        .into_iter()
        .map(|(r, t, v)| ((r, t), v))
        .collect();

    // Diagonal should dominate: self-votes >= any cross-vote for each point
    for i in 0..positions.len() {
        let self_votes = votes.get(&(i, i)).copied().unwrap_or(0);
        assert!(self_votes > 0, "Point {i} should have self-votes");
        for j in 0..positions.len() {
            if i != j {
                let cross_votes = votes.get(&(i, j)).copied().unwrap_or(0);
                assert!(
                    self_votes >= cross_votes,
                    "Point {i}: self-votes ({self_votes}) < cross-votes to {j} ({cross_votes})"
                );
            }
        }
    }
}

#[test]
fn vote_for_correspondences_no_matching_triangles() {
    // Equilateral-ish triangle vs very thin triangle → no matches at tight tolerance
    let positions_a = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(5.0, 8.66), // equilateral, ratios ≈ (1.0, 1.0)
    ];

    let positions_b = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(100.0, 0.0),
        DVec2::new(50.0, 1.0), // very thin, ratios ≈ (0.5, 0.5)
    ];

    let tri_a = triangles_of(&positions_a, 3);
    let tri_b = triangles_of(&positions_b, 3);
    assert!(!tri_a.is_empty());
    assert!(!tri_b.is_empty());

    let invariant_tree = build_invariant_tree(&tri_a).unwrap();

    let config = TriangleConfig {
        ratio_tolerance: 0.01,
        ..Default::default()
    };

    let vm = vote_for_correspondences(
        &tri_b,
        &tri_a,
        &invariant_tree,
        &config,
        positions_a.len(),
        positions_b.len(),
    );

    assert_eq!(vm.nonzero_entries().len(), 0);
}

#[test]
fn vote_for_correspondences_orientation_filtering() {
    let positions = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(0.0, 10.0),
        DVec2::new(10.0, 10.0),
        DVec2::new(5.0, 5.0),
    ];

    // Mirror x to flip all triangle orientations
    let mirrored: Vec<DVec2> = positions.iter().map(|p| DVec2::new(-p.x, p.y)).collect();

    let ref_triangles = triangles_of(&positions, 4);
    let target_triangles = triangles_of(&mirrored, 4);
    let invariant_tree = build_invariant_tree(&ref_triangles).unwrap();

    // With orientation check: mirrored triangles rejected → fewer/no votes
    let config_with = TriangleConfig {
        check_orientation: true,
        ..Default::default()
    };
    let vm_with = vote_for_correspondences(
        &target_triangles,
        &ref_triangles,
        &invariant_tree,
        &config_with,
        positions.len(),
        mirrored.len(),
    );

    // Without orientation check: all matching triangles accepted → more votes
    let config_without = TriangleConfig {
        check_orientation: false,
        ..Default::default()
    };
    let vm_without = vote_for_correspondences(
        &target_triangles,
        &ref_triangles,
        &invariant_tree,
        &config_without,
        positions.len(),
        mirrored.len(),
    );

    let total_with: usize = vm_with
        .nonzero_entries()
        .into_iter()
        .map(|(_, _, v)| v)
        .sum();
    let total_without: usize = vm_without
        .nonzero_entries()
        .into_iter()
        .map(|(_, _, v)| v)
        .sum();

    // With mirroring, orientation check should block matches
    assert!(
        total_without > total_with,
        "Orientation filtering should reduce votes: with={total_with}, without={total_without}"
    );
}
