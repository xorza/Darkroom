use super::*;

/// A vote matrix counts each pair's votes at the pair it was given, at any star count — corners
/// of a 600×600 field and of a 3×4 one alike — and visits the pairs in `(reference, target)`
/// order, whatever order the votes came in. 70 000 votes on one pair count exactly.
#[test]
fn vote_matrices_count_each_pair_in_order() {
    for (n_ref, n_target) in [(10, 10), (3, 4), (600, 600)] {
        let mut matrix = VoteMatrix::default();
        assert!(matrix.nonzero_entries().is_empty());
        let entries = [
            (n_ref - 1, n_target - 1, 4),
            (0, n_target - 1, 2),
            (n_ref / 2, n_target / 3, 5),
            (n_ref - 1, 0, 3),
            (0, 0, 1),
        ];
        for &(r, t, votes) in &entries {
            for _ in 0..votes {
                matrix.increment(r, t);
            }
        }
        let mut expected = entries.to_vec();
        expected.sort_unstable();
        assert_eq!(matrix.nonzero_entries(), expected, "{n_ref}×{n_target}");
    }
    let mut matrix = VoteMatrix::default();
    for _ in 0..70_000 {
        matrix.increment(599, 3);
    }
    assert_eq!(matrix.nonzero_entries(), [(599, 3, 70_000)]);
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
        let matches = resolve_matches(&mut vote_matrix_from_entries(entries), 5, 5, min_votes);
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
        let mut votes = vote_for_correspondences(
            &triangles_of(&target, 4),
            &reference,
            &invariant_tree,
            &config,
        );
        assert_eq!(
            votes.nonzero_entries(),
            expected,
            "orientation check {check_orientation}"
        );
    }
}

/// A near-isosceles triangle whose two shorter sides trade places under noise still votes for its
/// true vertices. Over (0, 0)–(10, 0), the apex 0.01 px left of the axis, (4.99, 3), makes the
/// side from the origin the shortest, √33.9001 against √34.1001; 0.01 px right, the other side is.
/// The roles of the base vertices swap, and with them the orientation each triangle reads, so the
/// order alone pairs each base vertex with the other, and the orientation check refuses even that.
/// The sides differ by 0.0017 of the longest, inside the 0.01 tolerance: the swapped order votes,
/// once for each true pair.
#[test]
fn a_near_isosceles_triangle_votes_in_both_orders() {
    let base = [DVec2::new(0.0, 0.0), DVec2::new(10.0, 0.0)];
    let with_apex = |x: f64| [base[0], base[1], DVec2::new(x, 3.0)];
    let reference = triangles_of(&with_apex(4.99), 3);
    let target = triangles_of(&with_apex(5.01), 3);
    let invariant_tree = build_invariant_tree(&reference).unwrap();
    let mut votes = vote_for_correspondences(
        &target,
        &reference,
        &invariant_tree,
        &TriangleConfig::default(),
    );
    assert_eq!(
        votes.nonzero_entries(),
        vec![(0, 0, 1), (1, 1, 1), (2, 2, 1)]
    );
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
    let mut votes = vote_for_correspondences(
        &triangles_of(&thin, 3),
        &reference,
        &build_invariant_tree(&reference).unwrap(),
        &TriangleConfig::default(),
    );
    assert!(votes.nonzero_entries().is_empty());
}
