//! Tests for the spatial module (k-d tree).

use crate::stacking::registration::spatial::*;
use crate::testing::test_rng::TestRng;

/// A tree holds every point at its original index whatever order it sorts them into, and has
/// nothing to hold for an empty set.
#[test]
fn build_stores_every_point_by_its_index() {
    assert!(KdTree::build(Vec::new()).is_none());
    let points = [
        DVec2::new(3.0, 1.0),
        DVec2::new(1.0, 3.0),
        DVec2::new(2.0, 2.0),
        DVec2::new(4.0, 0.0),
        DVec2::new(-1.0, 42.0),
    ];
    let tree = KdTree::build(points.to_vec()).unwrap();
    assert_eq!(tree.len(), 5);
    assert_eq!(tree.points(), &points);
    for (i, &p) in points.iter().enumerate() {
        assert_eq!(tree.get_point(i), p);
    }
}

/// One rank, or a run of ranks the tree may fill in any order.
#[derive(Debug)]
struct Group {
    dist_sq: f64,
    /// Consecutive ranks sitting at this distance.
    ranks: usize,
    /// The indices those ranks draw from, each used at most once. One index for a distinct
    /// distance; several when the fixture ties, because a k-d tree promises an ordering by
    /// distance and nothing about points that share one. More entries than `ranks` where the
    /// fixture has more tied points than the query asked for.
    allowed: Vec<usize>,
}

#[derive(Debug)]
struct KNearestCase {
    name: &'static str,
    points: Vec<DVec2>,
    query: DVec2,
    k: usize,
    expected: Vec<Group>,
}

/// `k_nearest` over every layout that mattered, as one table.
///
/// Each row pins the whole result — every rank's index and squared distance, in order — where
/// several of the thirteen tests this replaces spot-checked a few ranks and left the rest
/// unasserted. `clustered_points` in particular checked ranks 0, 1 and 4 of its second query.
///
/// Distances are squared and hand-computed in each row's comment. They are compared to a relative
/// 1e-9, which is far above the few ulps these arithmetic sums carry and far below the gap any
/// real error would open — a wrong neighbour shows up in the index, not the distance.
#[test]
fn k_nearest_over_every_layout() {
    /// Ranks at distinct distances, in order.
    fn ranked(pairs: &[(usize, f64)]) -> Vec<Group> {
        pairs
            .iter()
            .map(|&(index, dist_sq)| Group {
                dist_sq,
                ranks: 1,
                allowed: vec![index],
            })
            .collect()
    }
    fn points(pairs: &[(f64, f64)]) -> Vec<DVec2> {
        pairs.iter().map(|&(x, y)| DVec2::new(x, y)).collect()
    }
    /// `count` points along the x-axis at integer coordinates.
    fn on_x_axis(count: usize) -> Vec<DVec2> {
        (0..count).map(|i| DVec2::new(i as f64, 0.0)).collect()
    }
    /// Two tight diagonal clusters of five, the second offset by `separation`.
    fn two_clusters(separation: f64) -> Vec<DVec2> {
        (0..10)
            .map(|i| {
                let base = if i < 5 { 0.0 } else { separation };
                let step = f64::from(i % 5) * 0.1;
                DVec2::new(base + step, base + step)
            })
            .collect()
    }

    let cases = vec![
        // On the x-axis at 0, 3, 7, 8, 15; query (6,0) → 36, 9, 1, 4, 81.
        KNearestCase {
            name: "distinct distances on a line",
            points: points(&[(0.0, 0.0), (3.0, 0.0), (7.0, 0.0), (8.0, 0.0), (15.0, 0.0)]),
            query: DVec2::new(6.0, 0.0),
            k: 3,
            expected: ranked(&[(2, 1.0), (3, 4.0), (1, 9.0)]),
        },
        // Query (2,2) → idx0 4+4=8, idx1 1+4=5, idx2 1+1=2, idx3 16+36=52.
        KNearestCase {
            name: "two dimensions",
            points: points(&[(0.0, 0.0), (3.0, 4.0), (1.0, 1.0), (6.0, 8.0)]),
            query: DVec2::new(2.0, 2.0),
            k: 2,
            expected: ranked(&[(2, 2.0), (1, 5.0)]),
        },
        // The query sits exactly on a point, which must come back at distance zero.
        KNearestCase {
            name: "query lands on a point",
            points: points(&[(0.0, 0.0), (10.0, 10.0), (5.0, 5.0)]),
            query: DVec2::new(5.0, 5.0),
            k: 1,
            expected: ranked(&[(2, 0.0)]),
        },
        KNearestCase {
            name: "results come back sorted",
            points: on_x_axis(4)
                .into_iter()
                .chain([DVec2::new(10.0, 0.0)])
                .collect(),
            query: DVec2::new(0.0, 0.0),
            k: 3,
            expected: ranked(&[(0, 0.0), (1, 1.0), (2, 4.0)]),
        },
        // k above the point count returns everything, not an error or a padded list.
        KNearestCase {
            name: "k exceeds the point count",
            points: points(&[(0.0, 0.0), (1.0, 1.0)]),
            query: DVec2::new(0.0, 0.0),
            k: 10,
            expected: ranked(&[(0, 0.0), (1, 2.0)]),
        },
        KNearestCase {
            name: "k is zero",
            points: points(&[(0.0, 0.0), (1.0, 1.0)]),
            query: DVec2::new(0.0, 0.0),
            k: 0,
            expected: Vec::new(),
        },
        // Query (-7,-7) → idx0 9+9=18, idx1 4+4=8, idx2 49+49=98.
        KNearestCase {
            name: "negative coordinates",
            points: points(&[
                (-10.0, -10.0),
                (-5.0, -5.0),
                (0.0, 0.0),
                (5.0, 5.0),
                (10.0, 10.0),
            ]),
            query: DVec2::new(-7.0, -7.0),
            k: 2,
            expected: ranked(&[(1, 8.0), (0, 18.0)]),
        },
        // Far query on the unit square: idx3 999²+999² = 1_996_002, then idx1 and idx2 tie at
        // 999²+1000² = 1_998_001.
        KNearestCase {
            name: "query far outside the points",
            points: points(&[(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)]),
            query: DVec2::new(1000.0, 1000.0),
            k: 2,
            expected: vec![
                Group {
                    dist_sq: 1_996_002.0,
                    ranks: 1,
                    allowed: vec![3],
                },
                Group {
                    dist_sq: 1_998_001.0,
                    ranks: 1,
                    allowed: vec![1, 2],
                },
            ],
        },
        // Three coincident points: all three ranks are at distance zero and must be distinct.
        KNearestCase {
            name: "coincident points",
            points: points(&[(5.0, 5.0), (5.0, 5.0), (5.0, 5.0), (10.0, 10.0)]),
            query: DVec2::new(5.0, 5.0),
            k: 3,
            expected: vec![Group {
                dist_sq: 0.0,
                ranks: 3,
                allowed: vec![0, 1, 2],
            }],
        },
        KNearestCase {
            name: "every point identical",
            points: vec![DVec2::new(7.0, 7.0); 5],
            query: DVec2::new(7.0, 7.0),
            k: 5,
            expected: vec![Group {
                dist_sq: 0.0,
                ranks: 5,
                allowed: vec![0, 1, 2, 3, 4],
            }],
        },
        // Collinear on y=x, query on the middle point: idx1 and idx3 both sit at 1+1=2.
        KNearestCase {
            name: "collinear with a symmetric tie",
            points: points(&[(0.0, 0.0), (1.0, 1.0), (2.0, 2.0), (3.0, 3.0), (4.0, 4.0)]),
            query: DVec2::new(2.0, 2.0),
            k: 3,
            expected: vec![
                Group {
                    dist_sq: 0.0,
                    ranks: 1,
                    allowed: vec![2],
                },
                Group {
                    dist_sq: 2.0,
                    ranks: 2,
                    allowed: vec![1, 3],
                },
            ],
        },
        // Two clusters 100 apart. Querying either one must return that cluster entire and never
        // reach across: steps of 0.1 on both axes give 0, 0.02, 0.08, 0.18, 0.32.
        KNearestCase {
            name: "clustered, query near cluster one",
            points: two_clusters(100.0),
            query: DVec2::new(0.0, 0.0),
            k: 5,
            expected: ranked(&[(0, 0.0), (1, 0.02), (2, 0.08), (3, 0.18), (4, 0.32)]),
        },
        KNearestCase {
            name: "clustered, query near cluster two",
            points: two_clusters(100.0),
            query: DVec2::new(100.0, 100.0),
            k: 5,
            expected: ranked(&[(5, 0.0), (6, 0.02), (7, 0.08), (8, 0.18), (9, 0.32)]),
        },
        // Past `SMALL_HEAP_CAPACITY` the search swaps to the large heap; the i-th nearest on the
        // x-axis is idx i at i².
        KNearestCase {
            name: "k past the small-heap capacity",
            points: on_x_axis(50),
            query: DVec2::new(0.0, 0.0),
            k: SMALL_HEAP_CAPACITY + 5,
            expected: ranked(
                &(0..SMALL_HEAP_CAPACITY + 5)
                    .map(|rank| (rank, (rank * rank) as f64))
                    .collect::<Vec<_>>(),
            ),
        },
    ];

    for case in cases {
        let tree = KdTree::build(case.points.clone()).expect("every fixture has points");
        let neighbours = tree.k_nearest(case.query, case.k);
        let name = case.name;

        let total: usize = case.expected.iter().map(|group| group.ranks).sum();
        assert_eq!(neighbours.len(), total, "{name}: neighbour count");

        let mut rank = 0;
        for group in &case.expected {
            let run = &neighbours[rank..rank + group.ranks];
            for neighbour in run {
                let tolerance = 1e-9 * group.dist_sq.abs().max(1.0);
                assert!(
                    (neighbour.dist_sq - group.dist_sq).abs() <= tolerance,
                    "{name}: rank {rank} distance {} should be {}",
                    neighbour.dist_sq,
                    group.dist_sq
                );
                assert!(
                    group.allowed.contains(&neighbour.index),
                    "{name}: rank {rank} index {} not among {:?}",
                    neighbour.index,
                    group.allowed
                );
            }
            let mut used: Vec<usize> = run.iter().map(|neighbour| neighbour.index).collect();
            used.sort_unstable();
            used.dedup();
            assert_eq!(
                used.len(),
                group.ranks,
                "{name}: tied ranks at {} must be distinct points",
                group.dist_sq
            );
            rank += group.ranks;
        }
    }
}

/// The nearest point and its squared distance, hand-computed: an exact hit, a lone point at
/// `3² + 7²` = 58, and a tie at 1.25 that either point may win.
#[test]
fn nearest_one_hand_cases() {
    let p = DVec2::new;
    for (points, query, indices, dist_sq) in [
        (
            &[p(0.0, 0.0), p(10.0, 10.0), p(5.0, 5.0)][..],
            p(5.0, 5.0),
            &[2][..],
            0.0,
        ),
        (&[p(3.0, 7.0)][..], p(0.0, 0.0), &[0][..], 58.0),
        (
            &[p(3.0, 4.0), p(5.0, 5.0)][..],
            p(4.0, 4.5),
            &[0, 1][..],
            1.25,
        ),
    ] {
        let nearest = KdTree::build(points.to_vec())
            .unwrap()
            .nearest_one(query)
            .unwrap();
        assert!(
            indices.contains(&nearest.index),
            "{query:?}: {}",
            nearest.index
        );
        assert_eq!(nearest.dist_sq, dist_sq, "{query:?}");
    }
}

/// A radius search keeps every point with `dist² ≤ r²` — the boundary included — and only those,
/// into a buffer it clears first. Points (0, 0), (1, 0), (2, 0), (0, 1), (5, 0), (−3, −4) and
/// (10, 10), every distance a hand sum of squares.
#[test]
fn radius_search_hand_cases() {
    let points = [
        DVec2::new(0.0, 0.0),
        DVec2::new(1.0, 0.0),
        DVec2::new(2.0, 0.0),
        DVec2::new(0.0, 1.0),
        DVec2::new(5.0, 0.0),
        DVec2::new(-3.0, -4.0),
        DVec2::new(10.0, 10.0),
    ];
    let tree = KdTree::build(points.to_vec()).unwrap();
    let mut found = vec![999, 888];
    for (query, radius, expected, why) in [
        (DVec2::ZERO, 0.0, &[0][..], "radius 0: the exact hit only"),
        (
            DVec2::ZERO,
            1.0,
            &[0, 1, 3][..],
            "dist² 1 on the boundary is kept",
        ),
        (DVec2::ZERO, 1.5, &[0, 1, 3][..], "dist² 4 is past 2.25"),
        (
            DVec2::ZERO,
            3.0,
            &[0, 1, 2, 3][..],
            "dist² 4 inside 9, 25 past it",
        ),
        (
            DVec2::ZERO,
            5.0,
            &[0, 1, 2, 3, 4, 5][..],
            "dist² 25 on the boundary, twice",
        ),
        (DVec2::new(-2.0, -3.0), 2.0, &[5][..], "(−3, −4) at dist² 2"),
        (
            DVec2::new(5.0, 5.0),
            1.0,
            &[][..],
            "nothing within 1 of (5, 5)",
        ),
        (
            DVec2::new(5.0, 5.0),
            100.0,
            &[0, 1, 2, 3, 4, 5, 6][..],
            "everything",
        ),
    ] {
        tree.radius_indices_into(query, radius, &mut found);
        found.sort_unstable();
        assert_eq!(found, expected, "{why}");
    }
}

/// Every query against a brute-force scan, on seeded random sets of a few hundred points — where a
/// tree has the depth for pruning to go wrong. The k nearest are the brute force's first k
/// distances, the nearest is the least distance, and a radius search is exactly the points within
/// it. Ties are kept apart by the random coordinates, so the indices match too.
#[test]
fn queries_agree_with_a_brute_force_scan() {
    let mut rng = TestRng::new(17);
    for count in [300, 700] {
        let points: Vec<DVec2> = (0..count)
            .map(|_| DVec2::new(rng.next_f64() * 1000.0, rng.next_f64() * 600.0))
            .collect();
        let tree = KdTree::build(points.clone()).unwrap();
        let mut neighbours = Vec::new();
        let mut within = Vec::new();
        for _ in 0..60 {
            let query = DVec2::new(
                rng.next_f64() * 1100.0 - 50.0,
                rng.next_f64() * 700.0 - 50.0,
            );
            let mut brute: Vec<(f64, usize)> = points
                .iter()
                .enumerate()
                .map(|(i, &p)| ((query - p).length_squared(), i))
                .collect();
            brute.sort_by(|a, b| a.0.total_cmp(&b.0));

            for k in [1, 5, 17] {
                tree.k_nearest_into(query, k, &mut neighbours);
                let found: Vec<(f64, usize)> =
                    neighbours.iter().map(|n| (n.dist_sq, n.index)).collect();
                assert_eq!(found, brute[..k], "k = {k} at {query:?}");
            }
            let nearest = tree.nearest_one(query).unwrap();
            assert_eq!((nearest.dist_sq, nearest.index), brute[0]);
            for radius in [0.0, 15.5, 100.0, 500.0] {
                tree.radius_indices_into(query, radius, &mut within);
                within.sort_unstable();
                let mut expected: Vec<usize> = brute
                    .iter()
                    .take_while(|(d, _)| *d <= radius * radius)
                    .map(|&(_, i)| i)
                    .collect();
                expected.sort_unstable();
                assert_eq!(within, expected, "radius {radius} at {query:?}");
            }
        }
    }
}

#[test]
fn heap_stays_inline_up_to_its_inline_capacity() {
    // The heap is built per k-nearest query, so a small k must not reach the allocator.
    assert!(!BoundedMaxHeap::new(5).items.spilled());
    assert!(!BoundedMaxHeap::new(SMALL_HEAP_CAPACITY).items.spilled());
    assert!(BoundedMaxHeap::new(SMALL_HEAP_CAPACITY + 1).items.spilled());
}

#[test]
fn heap_empty_state() {
    let heap_small = BoundedMaxHeap::new(5);
    assert!(!heap_small.is_full());
    assert_eq!(heap_small.max_distance(), f64::INFINITY);

    let heap_large = BoundedMaxHeap::new(50);
    assert!(!heap_large.is_full());
    assert_eq!(heap_large.max_distance(), f64::INFINITY);
}

#[test]
fn heap_small_push_and_eviction() {
    let mut heap = BoundedMaxHeap::new(3);

    // Push 3 items: dist_sq = 10, 5, 15
    heap.push(Neighbor {
        index: 0,
        dist_sq: 10.0,
    });
    heap.push(Neighbor {
        index: 1,
        dist_sq: 5.0,
    });
    heap.push(Neighbor {
        index: 2,
        dist_sq: 15.0,
    });

    assert!(heap.is_full());
    // Max-heap root should be the largest: 15.0
    assert!((heap.max_distance() - 15.0).abs() < 1e-10);

    // Push smaller item (2.0) — should evict 15.0
    heap.push(Neighbor {
        index: 3,
        dist_sq: 2.0,
    });
    // New max should be 10.0
    assert!((heap.max_distance() - 10.0).abs() < 1e-10);

    // Push larger item (20.0) — should be rejected
    heap.push(Neighbor {
        index: 4,
        dist_sq: 20.0,
    });
    assert!((heap.max_distance() - 10.0).abs() < 1e-10);

    // Final contents: dist_sq = {10, 5, 2}, indices = {0, 1, 3}
    let mut result = Vec::new();
    heap.write_into(&mut result);
    assert_eq!(result.len(), 3);
    let mut dist_sqs: Vec<u64> = result.iter().map(|n| n.dist_sq.to_bits()).collect();
    dist_sqs.sort_unstable();
    let expected: Vec<u64> = [2.0_f64, 5.0, 10.0].iter().map(|d| d.to_bits()).collect();
    assert_eq!(dist_sqs, expected);

    let mut indices: Vec<usize> = result.iter().map(|n| n.index).collect();
    indices.sort_unstable();
    assert_eq!(indices, vec![0, 1, 3]);
}

#[test]
fn heap_large_push_and_eviction() {
    let capacity = SMALL_HEAP_CAPACITY + 5; // 37
    let mut heap = BoundedMaxHeap::new(capacity);
    assert!(heap.items.spilled());

    // Push capacity items with dist_sq = capacity, capacity-1, ..., 1
    for i in 0..capacity {
        heap.push(Neighbor {
            index: i,
            dist_sq: (capacity - i) as f64,
        });
    }

    assert!(heap.is_full());
    // Max should be capacity (=37)
    assert!((heap.max_distance() - capacity as f64).abs() < 1e-10);

    // Push 0.5 — should evict the max (37.0)
    heap.push(Neighbor {
        index: 100,
        dist_sq: 0.5,
    });
    // New max should be capacity-1 = 36
    assert!((heap.max_distance() - (capacity - 1) as f64).abs() < 1e-10);

    let mut result = Vec::new();
    heap.write_into(&mut result);
    assert_eq!(result.len(), capacity);
    // Should contain 0.5 and 1..36
    let has_half = result.iter().any(|n| (n.dist_sq - 0.5).abs() < 1e-10);
    assert!(has_half);
    // Should NOT contain the evicted max (37.0)
    let has_max = result
        .iter()
        .any(|n| (n.dist_sq - capacity as f64).abs() < 1e-10);
    assert!(!has_max);
}

#[test]
fn heap_capacity_one() {
    // Capacity 1: only keeps the single smallest
    let mut heap = BoundedMaxHeap::new(1);

    heap.push(Neighbor {
        index: 0,
        dist_sq: 10.0,
    });
    assert!(heap.is_full());
    assert!((heap.max_distance() - 10.0).abs() < 1e-10);

    // Push smaller — should replace
    heap.push(Neighbor {
        index: 1,
        dist_sq: 3.0,
    });
    assert!((heap.max_distance() - 3.0).abs() < 1e-10);

    // Push larger — should be rejected
    heap.push(Neighbor {
        index: 2,
        dist_sq: 50.0,
    });
    assert!((heap.max_distance() - 3.0).abs() < 1e-10);

    let mut result = Vec::new();
    heap.write_into(&mut result);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].index, 1);
    assert!((result[0].dist_sq - 3.0).abs() < 1e-10);
}
