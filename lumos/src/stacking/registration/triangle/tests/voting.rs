use crate::stacking::registration::triangle::tests::*;

#[test]
fn test_vote_matrix_dense_mode() {
    // 10*10 = 100 < 250,000 → dense
    let mut vm = VoteMatrix::new(10, 10);
    assert!(matches!(vm, VoteMatrix::Dense { .. }));

    vm.increment(0, 0);
    vm.increment(0, 0);
    vm.increment(5, 7);

    let entries: Vec<_> = vm.iter_nonzero().collect();
    let get = |r, t| entries.iter().find(|e| e.0 == r && e.1 == t).map(|e| e.2);
    assert_eq!(get(0, 0), Some(2));
    assert_eq!(get(5, 7), Some(1));
    assert_eq!(entries.len(), 2);
}

#[test]
fn test_vote_matrix_sparse_mode() {
    // 600*600 = 360,000 >= 250,000 → sparse
    let mut vm = VoteMatrix::new(600, 600);
    assert!(matches!(vm, VoteMatrix::Sparse(_)));

    vm.increment(0, 0);
    vm.increment(0, 0);
    vm.increment(100, 200);

    let entries: Vec<_> = vm.iter_nonzero().collect();
    let get = |r, t| entries.iter().find(|e| e.0 == r && e.1 == t).map(|e| e.2);
    assert_eq!(get(0, 0), Some(2));
    assert_eq!(get(100, 200), Some(1));
    assert_eq!(entries.len(), 2);
}

#[test]
fn test_vote_matrix_empty() {
    let vm_dense = VoteMatrix::new(5, 5);
    assert_eq!(vm_dense.iter_nonzero().count(), 0);

    let vm_sparse = VoteMatrix::new(600, 600);
    assert_eq!(vm_sparse.iter_nonzero().count(), 0);
}

#[test]
fn test_vote_matrix_threshold_boundary() {
    // size < 250,000 → dense, size >= 250,000 → sparse

    // 499*500 = 249,500 < 250,000 → dense
    let vm_below = VoteMatrix::new(499, 500);
    assert!(matches!(vm_below, VoteMatrix::Dense { .. }));

    // 500*500 = 250,000, not < 250,000 → sparse
    let vm_at = VoteMatrix::new(500, 500);
    assert!(matches!(vm_at, VoteMatrix::Sparse(_)));
}

#[test]
fn test_vote_matrix_dense_index_mapping() {
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

    let entries: Vec<_> = vm.iter_nonzero().collect();
    let get = |r, t| entries.iter().find(|e| e.0 == r && e.1 == t).map(|e| e.2);

    assert_eq!(get(0, 0), Some(1));
    assert_eq!(get(0, 3), Some(2));
    assert_eq!(get(1, 2), Some(3));
    assert_eq!(get(2, 0), Some(1));
    assert_eq!(get(2, 3), Some(4));
    assert_eq!(entries.len(), 5);
}

#[test]
fn test_vote_matrix_dense_boundary_indices() {
    // Test accessing corners: (0,0), (0,n-1), (n-1,0), (n-1,n-1)
    let n = 10;
    let mut vm = VoteMatrix::new(n, n);
    vm.increment(0, 0);
    vm.increment(0, n - 1);
    vm.increment(n - 1, 0);
    vm.increment(n - 1, n - 1);

    let entries: Vec<_> = vm.iter_nonzero().collect();
    let get = |r, t| entries.iter().find(|e| e.0 == r && e.1 == t).map(|e| e.2);
    assert_eq!(get(0, 0), Some(1));
    assert_eq!(get(0, n - 1), Some(1));
    assert_eq!(get(n - 1, 0), Some(1));
    assert_eq!(get(n - 1, n - 1), Some(1));
    assert_eq!(entries.len(), 4);
}

#[test]
fn test_vote_matrix_dense_saturating_add() {
    // Dense mode uses u16. Verify exact count for reasonable values.
    let mut vm = VoteMatrix::new(2, 2);
    for _ in 0..1000 {
        vm.increment(0, 0);
    }
    let entries: Vec<_> = vm.iter_nonzero().collect();
    let votes = entries.iter().find(|e| e.0 == 0 && e.1 == 0).unwrap().2;
    assert_eq!(votes, 1000);
}

#[test]
fn test_resolve_matches_one_to_one() {
    // 3 non-conflicting matches sorted by descending votes
    let vm = vote_matrix_from_entries(3, 3, &[(0, 0, 10), (1, 1, 8), (2, 2, 6)]);

    let matches = resolve_matches(vm, 3, 3, 1);
    assert_eq!(matches.len(), 3);

    // Sorted by votes descending
    assert_eq!(matches[0].ref_idx, 0);
    assert_eq!(matches[0].target_idx, 0);
    assert_eq!(matches[0].votes, 10);

    assert_eq!(matches[1].ref_idx, 1);
    assert_eq!(matches[1].target_idx, 1);
    assert_eq!(matches[1].votes, 8);

    assert_eq!(matches[2].ref_idx, 2);
    assert_eq!(matches[2].target_idx, 2);
    assert_eq!(matches[2].votes, 6);
}

#[test]
fn test_resolve_matches_target_conflict() {
    // Two ref points compete for the same target:
    // ref 0 → target 0 (10 votes), ref 1 → target 0 (5 votes), ref 1 → target 1 (3 votes)
    // Greedy: ref 0 wins target 0, ref 1 falls back to target 1
    let vm = vote_matrix_from_entries(3, 3, &[(0, 0, 10), (1, 0, 5), (1, 1, 3)]);

    let matches = resolve_matches(vm, 3, 3, 1);
    assert_eq!(matches.len(), 2);

    let m0 = matches.iter().find(|m| m.ref_idx == 0).unwrap();
    assert_eq!(m0.target_idx, 0);
    assert_eq!(m0.votes, 10);

    let m1 = matches.iter().find(|m| m.ref_idx == 1).unwrap();
    assert_eq!(m1.target_idx, 1);
    assert_eq!(m1.votes, 3);
}

#[test]
fn test_resolve_matches_ref_conflict() {
    // Two target points compete for the same ref:
    // ref 0 → target 0 (10 votes), ref 0 → target 1 (5 votes), ref 1 → target 1 (3 votes)
    // Greedy: ref 0 gets target 0 (highest), ref 0 → target 1 blocked (ref 0 used), ref 1 gets target 1
    let vm = vote_matrix_from_entries(3, 3, &[(0, 0, 10), (0, 1, 5), (1, 1, 3)]);

    let matches = resolve_matches(vm, 3, 3, 1);
    assert_eq!(matches.len(), 2);

    let m0 = matches.iter().find(|m| m.ref_idx == 0).unwrap();
    assert_eq!(m0.target_idx, 0);
    assert_eq!(m0.votes, 10);

    let m1 = matches.iter().find(|m| m.ref_idx == 1).unwrap();
    assert_eq!(m1.target_idx, 1);
    assert_eq!(m1.votes, 3);
}

#[test]
fn test_resolve_matches_min_votes_filter() {
    // Only ref 0 → target 0 (10 votes) survives min_votes = 3
    let vm = vote_matrix_from_entries(3, 3, &[(0, 0, 10), (1, 1, 2), (2, 2, 1)]);

    let matches = resolve_matches(vm, 3, 3, 3);
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].ref_idx, 0);
    assert_eq!(matches[0].target_idx, 0);
    assert_eq!(matches[0].votes, 10);
}

#[test]
fn test_resolve_matches_empty() {
    let vm = VoteMatrix::new(5, 5);
    let matches = resolve_matches(vm, 5, 5, 1);
    assert!(matches.is_empty());
}

#[test]
fn test_resolve_matches_confidence_relative() {
    // Confidence = votes / max_votes in resolved set
    // Three matches: 20, 10, 5 votes → confidence = 1.0, 0.5, 0.25
    let vm = vote_matrix_from_entries(5, 5, &[(0, 0, 20), (1, 1, 10), (2, 2, 5)]);

    let matches = resolve_matches(vm, 5, 5, 1);
    assert_eq!(matches.len(), 3);

    // matches[0]: 20 votes → 20/20 = 1.0
    assert_eq!(matches[0].votes, 20);
    assert!((matches[0].confidence - 1.0).abs() < 1e-10);

    // matches[1]: 10 votes → 10/20 = 0.5
    assert_eq!(matches[1].votes, 10);
    assert!((matches[1].confidence - 0.5).abs() < 1e-10);

    // matches[2]: 5 votes → 5/20 = 0.25
    assert_eq!(matches[2].votes, 5);
    assert!((matches[2].confidence - 0.25).abs() < 1e-10);
}

#[test]
fn test_resolve_matches_single_entry_confidence_is_1() {
    // Single match: confidence = votes/max_votes = 10/10 = 1.0
    let vm = vote_matrix_from_entries(5, 5, &[(0, 0, 10)]);

    let matches = resolve_matches(vm, 5, 5, 1);
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].votes, 10);
    assert!((matches[0].confidence - 1.0).abs() < 1e-10);
}

#[test]
fn test_vote_for_correspondences_identical_triangles() {
    // Identical point sets → every triangle matches itself → diagonal dominates
    let positions = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(0.0, 10.0),
        DVec2::new(10.0, 10.0),
        DVec2::new(5.0, 5.0),
    ];

    let triangles = form_triangles_kdtree(&positions, 4);
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

    let votes: std::collections::HashMap<(usize, usize), usize> =
        vm.iter_nonzero().map(|(r, t, v)| ((r, t), v)).collect();

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
fn test_vote_for_correspondences_no_matching_triangles() {
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

    let tri_a = form_triangles_kdtree(&positions_a, 3);
    let tri_b = form_triangles_kdtree(&positions_b, 3);
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

    assert_eq!(vm.iter_nonzero().count(), 0);
}

#[test]
fn test_vote_for_correspondences_orientation_filtering() {
    let positions = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(0.0, 10.0),
        DVec2::new(10.0, 10.0),
        DVec2::new(5.0, 5.0),
    ];

    // Mirror x to flip all triangle orientations
    let mirrored: Vec<DVec2> = positions.iter().map(|p| DVec2::new(-p.x, p.y)).collect();

    let ref_triangles = form_triangles_kdtree(&positions, 4);
    let target_triangles = form_triangles_kdtree(&mirrored, 4);
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

    let total_with: usize = vm_with.iter_nonzero().map(|(_, _, v)| v).sum();
    let total_without: usize = vm_without.iter_nonzero().map(|(_, _, v)| v).sum();

    // With mirroring, orientation check should block matches
    assert!(
        total_without > total_with,
        "Orientation filtering should reduce votes: with={total_with}, without={total_without}"
    );
}
