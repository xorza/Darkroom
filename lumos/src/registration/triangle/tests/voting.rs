use super::*;

/// A vote matrix counts each pair's votes at the pair it was given, at any star count — corners
/// of a 600×600 field and of a 3×4 one alike — and visits the pairs in `(reference, target)`
/// order, whatever order the votes came in. 70 000 votes on one pair count exactly.
#[test]
fn vote_matrices_count_each_pair_in_order() {
    for (n_ref, n_target) in [(10, 10), (3, 4), (600, 600)] {
        let mut matrix = Votes::default();
        assert!(matrix.nonzero_entries().is_empty());
        let entries = [
            tally(n_ref - 1, n_target - 1, 4),
            tally(0, n_target - 1, 2),
            tally(n_ref / 2, n_target / 3, 5),
            tally(n_ref - 1, 0, 3),
            tally(0, 0, 1),
        ];
        for &PairVotes { pair, votes } in &entries {
            for _ in 0..votes {
                matrix.increment(pair.reference, pair.target);
            }
        }
        let mut expected = entries.to_vec();
        expected.sort_unstable_by_key(|entry| (entry.pair.reference, entry.pair.target));
        assert_eq!(matrix.nonzero_entries(), expected, "{n_ref}×{n_target}");
    }
    let mut matrix = Votes::default();
    for _ in 0..70_000 {
        matrix.increment(599, 3);
    }
    assert_eq!(matrix.nonzero_entries(), [tally(599, 3, 70_000)]);
}

/// One case of [`resolve_claims_each_star_once_by_votes`].
#[derive(Debug)]
struct ResolveCase<'a> {
    entries: &'a [PairVotes],
    min_votes: usize,
    /// The resolved matches, in output order.
    expected: &'a [PointMatch],
}

/// `reference` paired with `target` at `confidence`.
const fn resolved(reference: usize, target: usize, confidence: f64) -> PointMatch {
    PointMatch {
        idxs: MatchIdxs { reference, target },
        confidence,
    }
}

/// Greedy resolution, case by case: the pairs that survive `min_votes` are taken in descending vote
/// order, each star at most once, and each kept pair's confidence is its votes over the most any
/// kept pair drew. Every confidence below is a correctly rounded quotient, so equality is exact.
#[test]
fn resolve_claims_each_star_once_by_votes() {
    let cases: [ResolveCase<'_>; 7] = [
        // No conflicts: all three, 10/10, 8/10, 6/10.
        ResolveCase {
            entries: &[tally(0, 0, 10), tally(1, 1, 8), tally(2, 2, 6)],
            min_votes: 1,
            expected: &[
                resolved(0, 0, 1.0),
                resolved(1, 1, 0.8),
                resolved(2, 2, 0.6),
            ],
        },
        // Ref 0 takes target 0 first, so ref 1 falls back to target 1 with 3 votes.
        ResolveCase {
            entries: &[tally(0, 0, 10), tally(1, 0, 5), tally(1, 1, 3)],
            min_votes: 1,
            expected: &[resolved(0, 0, 1.0), resolved(1, 1, 0.3)],
        },
        // Ref 0 is taken by its 10-vote pair, so its 5-vote pair is dropped.
        ResolveCase {
            entries: &[tally(0, 0, 10), tally(0, 1, 5), tally(1, 1, 3)],
            min_votes: 1,
            expected: &[resolved(0, 0, 1.0), resolved(1, 1, 0.3)],
        },
        // Only the 10-vote pair clears `min_votes = 3`.
        ResolveCase {
            entries: &[tally(0, 0, 10), tally(1, 1, 2), tally(2, 2, 1)],
            min_votes: 3,
            expected: &[resolved(0, 0, 1.0)],
        },
        ResolveCase {
            entries: &[tally(0, 0, 20), tally(1, 1, 10), tally(2, 2, 5)],
            min_votes: 1,
            expected: &[
                resolved(0, 0, 1.0),
                resolved(1, 1, 0.5),
                resolved(2, 2, 0.25),
            ],
        },
        ResolveCase {
            entries: &[tally(0, 0, 10)],
            min_votes: 1,
            expected: &[resolved(0, 0, 1.0)],
        },
        ResolveCase {
            entries: &[],
            min_votes: 1,
            expected: &[],
        },
    ];
    for ResolveCase {
        entries,
        min_votes,
        expected,
    } in cases
    {
        let matches = votes_from_entries(entries).resolve(5, 5, min_votes);
        assert_eq!(matches, expected, "{entries:?} with min_votes {min_votes}");
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
    let diagonal: Vec<PairVotes> = (0..5).map(|i| tally(i, i, 6)).collect();
    for (target, check_orientation, expected) in [
        (IRREGULAR, true, diagonal.clone()),
        (mirrored, false, diagonal),
        (mirrored, true, Vec::new()),
    ] {
        let config = TriangleConfig {
            check_orientation,
            ..Default::default()
        };
        let mut votes = cast_votes(&triangles_of(&target, 4), reference.clone(), &config);
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
    let mut votes = cast_votes(&target, reference, &TriangleConfig::default());
    assert_eq!(
        votes.nonzero_entries(),
        [tally(0, 0, 1), tally(1, 1, 1), tally(2, 2, 1)]
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
    let mut votes = cast_votes(
        &triangles_of(&thin, 3),
        triangles_of(&equilateral, 3),
        &TriangleConfig::default(),
    );
    assert!(votes.nonzero_entries().is_empty());
}
