use super::*;

/// A vote matrix is dense below 250 000 cells and sparse from there, and either way it counts each
/// pair's votes at the pair it was given: `ref·n_target + target` in the dense layout, corners
/// included.
#[test]
fn vote_matrices_count_each_pair() {
    for (n_ref, n_target, dense) in [
        (10, 10, true),
        (3, 4, true),
        (499, 500, true),
        (500, 500, false),
        (600, 600, false),
    ] {
        let mut matrix = VoteMatrix::new(n_ref, n_target);
        assert_eq!(
            matches!(matrix, VoteMatrix::Dense { .. }),
            dense,
            "{n_ref}×{n_target}"
        );
        assert!(matrix.nonzero_entries().is_empty());
        let entries = [
            (0, 0, 1),
            (0, n_target - 1, 2),
            (n_ref - 1, 0, 3),
            (n_ref - 1, n_target - 1, 4),
            (n_ref / 2, n_target / 3, 5),
        ];
        for &(r, t, votes) in &entries {
            for _ in 0..votes {
                matrix.increment(r, t);
            }
        }
        let mut counted = matrix.nonzero_entries();
        counted.sort_unstable();
        let mut expected = entries.to_vec();
        expected.sort_unstable();
        assert_eq!(counted, expected, "{n_ref}×{n_target}");
    }
}

/// A dense cell holds a `u16`: 65 534 votes count exactly, and the next is past what the dense
/// layout promises to hold — a debug build stops on it. The sparse layout counts in `u32`, past
/// the dense limit.
#[test]
fn a_dense_cell_holds_65_534_votes_and_a_sparse_one_more() {
    let mut dense = VoteMatrix::new(2, 2);
    for _ in 0..65_534 {
        dense.increment(1, 0);
    }
    assert_eq!(dense.nonzero_entries(), [(1, 0, 65_534)]);

    let mut sparse = VoteMatrix::new(600, 600);
    for _ in 0..70_000 {
        sparse.increment(599, 3);
    }
    assert_eq!(sparse.nonzero_entries(), [(599, 3, 70_000)]);
}

#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "Vote overflow")]
fn the_65_535th_dense_vote_is_refused_in_debug() {
    let mut dense = VoteMatrix::new(2, 2);
    for _ in 0..65_535 {
        dense.increment(0, 0);
    }
}

/// Greedy resolution, case by case: the pairs that survive `min_votes` are taken in descending vote
/// order, each star at most once, and each kept pair's confidence is its votes over the most any
/// kept pair drew. Every confidence below is a correctly rounded quotient, so equality is exact.
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
        let matches = resolve_matches(&vote_matrix_from_entries(5, 5, entries), 5, 5, min_votes);
        let resolved: Vec<(usize, usize, f64)> = matches
            .iter()
            .map(|m| (m.indices.reference, m.indices.target, m.confidence))
            .collect();
        assert_eq!(resolved, expected, "{entries:?} with min_votes {min_votes}");
    }
}

/// Each pair of similar triangles votes for its three vertex pairs. `IRREGULAR` against itself: its
/// ten triangles are pairwise dissimilar, so each votes only for itself, and point `i` draws one
/// vote per triangle it is a vertex of — six, `C(4, 2)` — at `(i, i)` and nothing anywhere else.
/// Mirrored, every triangle turns the other way: with the orientation check no pair votes at all,
/// and without it the votes are the unmirrored ones.
#[test]
fn similar_triangles_vote_for_their_vertices() {
    let mirrored = IRREGULAR.map(|p| DVec2::new(-p.x, p.y));
    let reference = triangles_of(&IRREGULAR, 4);
    let invariant_tree = build_invariant_tree(&reference).unwrap();
    let diagonal: Vec<(usize, usize, usize)> = (0..5).map(|i| (i, i, 6)).collect();
    for (target, check_orientation, expected) in [
        (IRREGULAR, true, diagonal.clone()),
        (mirrored, false, diagonal),
        (mirrored, true, Vec::new()),
    ] {
        let config = TriangleConfig {
            check_orientation,
            ..Default::default()
        };
        let votes = vote_for_correspondences(
            &triangles_of(&target, 4),
            &reference,
            &invariant_tree,
            &config,
            5,
            5,
        );
        let mut counted = votes.nonzero_entries();
        counted.sort_unstable();
        assert_eq!(counted, expected, "orientation check {check_orientation}");
    }
}

/// A triangle only votes for one within the ratio tolerance: an equilateral one, ratios (1, 1),
/// against a thin one, ratios (0.5001, 0.5001), at 0.01, draws nothing.
#[test]
fn dissimilar_triangles_do_not_vote() {
    let equilateral = [
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(5.0, 75.0f64.sqrt()),
    ];
    let thin = [
        DVec2::new(0.0, 0.0),
        DVec2::new(100.0, 0.0),
        DVec2::new(50.0, 1.0),
    ];
    let reference = triangles_of(&equilateral, 3);
    let votes = vote_for_correspondences(
        &triangles_of(&thin, 3),
        &reference,
        &build_invariant_tree(&reference).unwrap(),
        &TriangleConfig::default(),
        3,
        3,
    );
    assert!(votes.nonzero_entries().is_empty());
}
