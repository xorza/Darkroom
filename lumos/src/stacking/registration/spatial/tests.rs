//! Tests for the spatial module (k-d tree).

use crate::stacking::registration::spatial::*;

/// Helper: collect radius search indices into a sorted Vec.
fn radius_search_indices(tree: &KdTree, query: DVec2, radius: f64) -> Vec<usize> {
    let mut buf = Vec::new();
    tree.radius_indices_into(query, radius, &mut buf);
    buf.sort();
    buf
}

#[test]
fn build_empty() {
    let tree = KdTree::build(&[]);
    assert!(tree.is_none());
}

#[test]
fn build_single_point() {
    let points = [DVec2::new(1.0, 2.0)];
    let tree = KdTree::build(&points).unwrap();
    assert_eq!(tree.len(), 1);
    // get_point should return the original point
    let p = tree.get_point(0);
    assert_eq!(p.x, 1.0);
    assert_eq!(p.y, 2.0);
}

#[test]
fn build_preserves_all_points() {
    // Points are stored by original index regardless of internal permutation.
    let points = [
        DVec2::new(3.0, 1.0),
        DVec2::new(1.0, 3.0),
        DVec2::new(2.0, 2.0),
        DVec2::new(4.0, 0.0),
    ];
    let tree = KdTree::build(&points).unwrap();
    assert_eq!(tree.len(), 4);
    for (i, p) in points.iter().enumerate() {
        let stored = tree.get_point(i);
        assert_eq!(stored.x, p.x);
        assert_eq!(stored.y, p.y);
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
                let step = (i % 5) as f64 * 0.1;
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
        let tree = KdTree::build(&case.points).expect("every fixture has points");
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

#[test]
fn nearest_one_exact_match() {
    let points = [
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 10.0),
        DVec2::new(5.0, 5.0),
    ];
    let tree = KdTree::build(&points).unwrap();

    let nn = tree.nearest_one(DVec2::new(5.0, 5.0)).unwrap();
    assert_eq!(nn.index, 2);
    assert_eq!(nn.dist_sq, 0.0);
}

#[test]
fn nearest_one_single_point() {
    // Single point at (3, 7). Query at origin.
    // dist_sq = 3^2 + 7^2 = 9 + 49 = 58
    let points = [DVec2::new(3.0, 7.0)];
    let tree = KdTree::build(&points).unwrap();
    let nn = tree.nearest_one(DVec2::new(0.0, 0.0)).unwrap();
    assert_eq!(nn.index, 0);
    assert!((nn.dist_sq - 58.0).abs() < 1e-10);
}

#[test]
fn nearest_one_equidistant() {
    // idx 0: (3,4), idx 1: (5,5)
    // Query: (4, 4.5)
    //   dist_sq to idx0: (4-3)^2 + (4.5-4)^2 = 1 + 0.25 = 1.25
    //   dist_sq to idx1: (4-5)^2 + (4.5-5)^2 = 1 + 0.25 = 1.25
    // Both equidistant — either is valid
    let points = [DVec2::new(3.0, 4.0), DVec2::new(5.0, 5.0)];
    let tree = KdTree::build(&points).unwrap();

    let nn = tree.nearest_one(DVec2::new(4.0, 4.5)).unwrap();
    assert!((nn.dist_sq - 1.25).abs() < 1e-10);
    assert!(nn.index == 0 || nn.index == 1);
}

#[test]
fn nearest_one_agrees_with_k_nearest_1() {
    // Verify nearest_one returns the same result as k_nearest(q, 1)
    let points = [
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 10.0),
        DVec2::new(5.0, 5.0),
        DVec2::new(3.0, 4.0),
    ];
    let tree = KdTree::build(&points).unwrap();

    let query = DVec2::new(7.0, 8.0);
    // dist_sq to idx0: 49+64=113, idx1: 9+4=13, idx2: 4+9=13, idx3: 16+16=32
    // idx1 and idx2 tie at 13; nearest_one and k_nearest should agree
    let nn = tree.nearest_one(query).unwrap();
    let kn = tree.k_nearest(query, 1);
    assert_eq!(nn.index, kn[0].index);
    assert!((nn.dist_sq - kn[0].dist_sq).abs() < 1e-10);
    assert!((nn.dist_sq - 13.0).abs() < 1e-10);
}

#[test]
fn nearest_one_empty_tree_not_possible() {
    // KdTree::build returns None for empty input, so nearest_one on an empty
    // tree can't happen through the public API. This test documents that
    // build(&[]) returns None.
    assert!(KdTree::build(&[]).is_none());
}

#[test]
fn radius_finds_correct_points() {
    // idx 0: (0,0), idx 1: (1,0), idx 2: (0,1), idx 3: (5,5), idx 4: (10,10)
    // Query: (0,0), radius: 2.0, radius_sq = 4.0
    //   dist_sq to idx0: 0 <= 4 => included
    //   dist_sq to idx1: 1 <= 4 => included
    //   dist_sq to idx2: 1 <= 4 => included
    //   dist_sq to idx3: 50 > 4 => excluded
    //   dist_sq to idx4: 200 > 4 => excluded
    let points = [
        DVec2::new(0.0, 0.0),
        DVec2::new(1.0, 0.0),
        DVec2::new(0.0, 1.0),
        DVec2::new(5.0, 5.0),
        DVec2::new(10.0, 10.0),
    ];
    let tree = KdTree::build(&points).unwrap();

    let indices = radius_search_indices(&tree, DVec2::new(0.0, 0.0), 2.0);
    assert_eq!(indices, vec![0, 1, 2]);
}

#[test]
fn radius_empty_result() {
    // All points far from query
    // idx 0: (0,0), idx 1: (10,10)
    // Query: (5, 5), radius: 1.0, radius_sq = 1.0
    //   dist_sq to idx0: 25+25=50 > 1
    //   dist_sq to idx1: 25+25=50 > 1
    let points = [DVec2::new(0.0, 0.0), DVec2::new(10.0, 10.0)];
    let tree = KdTree::build(&points).unwrap();

    let indices = radius_search_indices(&tree, DVec2::new(5.0, 5.0), 1.0);
    assert!(indices.is_empty());
}

#[test]
fn radius_all_points_included() {
    // idx 0: (0,0), idx 1: (1,0), idx 2: (0,1), idx 3: (1,1)
    // Query: (0.5, 0.5), radius: 10.0
    // All within radius
    let points = [
        DVec2::new(0.0, 0.0),
        DVec2::new(1.0, 0.0),
        DVec2::new(0.0, 1.0),
        DVec2::new(1.0, 1.0),
    ];
    let tree = KdTree::build(&points).unwrap();

    let indices = radius_search_indices(&tree, DVec2::new(0.5, 0.5), 10.0);
    assert_eq!(indices, vec![0, 1, 2, 3]);
}

#[test]
fn radius_boundary_inclusion() {
    // idx 0: (0,0), idx 1: (1,0), idx 2: (2,0)
    // Query: (0, 0), radius: 1.0, radius_sq = 1.0
    //   dist_sq to idx0: 0 <= 1 => included
    //   dist_sq to idx1: 1 <= 1 => included (boundary: dist_sq == radius_sq)
    //   dist_sq to idx2: 4 > 1 => excluded
    let points = [
        DVec2::new(0.0, 0.0),
        DVec2::new(1.0, 0.0),
        DVec2::new(2.0, 0.0),
    ];
    let tree = KdTree::build(&points).unwrap();

    let indices = radius_search_indices(&tree, DVec2::new(0.0, 0.0), 1.0);
    assert_eq!(indices, vec![0, 1]);
}

#[test]
fn radius_zero() {
    // radius=0 means only exact matches (dist_sq=0 <= 0)
    let points = [DVec2::new(0.0, 0.0), DVec2::new(1.0, 1.0)];
    let tree = KdTree::build(&points).unwrap();

    let indices = radius_search_indices(&tree, DVec2::new(0.0, 0.0), 0.0);
    assert_eq!(indices, vec![0]);
}

#[test]
fn radius_negative_coordinates() {
    // idx 0: (-3, -4), idx 1: (0, 0), idx 2: (3, 4)
    // Query: (-2, -3), radius: 2.0, radius_sq = 4.0
    //   dist_sq to idx0: (-2+3)^2 + (-3+4)^2 = 1+1 = 2 <= 4 => included
    //   dist_sq to idx1: 4+9 = 13 > 4 => excluded
    //   dist_sq to idx2: 25+49 = 74 > 4 => excluded
    let points = [
        DVec2::new(-3.0, -4.0),
        DVec2::new(0.0, 0.0),
        DVec2::new(3.0, 4.0),
    ];
    let tree = KdTree::build(&points).unwrap();

    let indices = radius_search_indices(&tree, DVec2::new(-2.0, -3.0), 2.0);
    assert_eq!(indices, vec![0]);
}

#[test]
fn radius_buffer_reuse_clears() {
    let points = [
        DVec2::new(0.0, 0.0),
        DVec2::new(1.0, 0.0),
        DVec2::new(10.0, 10.0),
    ];
    let tree = KdTree::build(&points).unwrap();

    let mut buf = Vec::new();

    // First query near origin: should find idx 0 and 1
    tree.radius_indices_into(DVec2::new(0.0, 0.0), 2.0, &mut buf);
    buf.sort();
    assert_eq!(buf, vec![0, 1]);

    // Second query near (10,10): should find only idx 2 (buffer cleared)
    tree.radius_indices_into(DVec2::new(10.0, 10.0), 0.5, &mut buf);
    assert_eq!(buf, vec![2]);
}

#[test]
fn radius_different_radii_different_results() {
    // idx 0: (0,0), idx 1: (2,0), idx 2: (5,0)
    // Query: (0,0)
    //   radius=1.5: radius_sq=2.25 → only idx0 (dist_sq=0)
    //   radius=3.0: radius_sq=9.0  → idx0 (0) + idx1 (4)
    //   radius=6.0: radius_sq=36.0 → all three: idx0 (0), idx1 (4), idx2 (25)
    let points = [
        DVec2::new(0.0, 0.0),
        DVec2::new(2.0, 0.0),
        DVec2::new(5.0, 0.0),
    ];
    let tree = KdTree::build(&points).unwrap();

    let r1 = radius_search_indices(&tree, DVec2::new(0.0, 0.0), 1.5);
    let r2 = radius_search_indices(&tree, DVec2::new(0.0, 0.0), 3.0);
    let r3 = radius_search_indices(&tree, DVec2::new(0.0, 0.0), 6.0);

    assert_eq!(r1, vec![0]);
    assert_eq!(r2, vec![0, 1]);
    assert_eq!(r3, vec![0, 1, 2]);
}

#[test]
fn get_point_returns_original_coordinates() {
    let points = [
        DVec2::new(3.125, 2.71),
        DVec2::new(-1.0, 42.0),
        DVec2::new(0.0, 0.0),
    ];
    let tree = KdTree::build(&points).unwrap();

    for (i, p) in points.iter().enumerate() {
        let stored = tree.get_point(i);
        assert_eq!(stored.x, p.x);
        assert_eq!(stored.y, p.y);
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
    dist_sqs.sort();
    let expected: Vec<u64> = [2.0_f64, 5.0, 10.0].iter().map(|d| d.to_bits()).collect();
    assert_eq!(dist_sqs, expected);

    let mut indices: Vec<usize> = result.iter().map(|n| n.index).collect();
    indices.sort();
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

#[test]
fn k_nearest_and_radius_agree() {
    // 5 points on x-axis. Query at origin, radius=4.5 (radius_sq=20.25).
    // idx 0: (0,0) dist_sq=0, idx 1: (2,0) dist_sq=4, idx 2: (4,0) dist_sq=16,
    // idx 3: (6,0) dist_sq=36, idx 4: (8,0) dist_sq=64
    // Radius should find: idx 0 (0), idx 1 (4), idx 2 (16) — all <= 20.25
    // k_nearest(3) from origin should find the same three
    let points = [
        DVec2::new(0.0, 0.0),
        DVec2::new(2.0, 0.0),
        DVec2::new(4.0, 0.0),
        DVec2::new(6.0, 0.0),
        DVec2::new(8.0, 0.0),
    ];
    let tree = KdTree::build(&points).unwrap();
    let query = DVec2::new(0.0, 0.0);

    let radius_result = radius_search_indices(&tree, query, 4.5);
    assert_eq!(radius_result, vec![0, 1, 2]);

    let knn_result = tree.k_nearest(query, 3);
    let mut knn_indices: Vec<usize> = knn_result.iter().map(|n| n.index).collect();
    knn_indices.sort();
    assert_eq!(knn_indices, vec![0, 1, 2]);
}

#[test]
fn horizontal_line_exact_distances() {
    // 10 points on x-axis: idx i at (10*i, 0), i=0..10
    // Query: (45, 0)
    //   dist_sq to idx4 (40,0): (45-40)^2 = 25
    //   dist_sq to idx5 (50,0): (45-50)^2 = 25
    //   dist_sq to idx3 (30,0): (45-30)^2 = 225
    // k=2: idx4 and idx5, both at dist_sq=25
    let points: Vec<DVec2> = (0..10).map(|i| DVec2::new(i as f64 * 10.0, 0.0)).collect();
    let tree = KdTree::build(&points).unwrap();

    let neighbors = tree.k_nearest(DVec2::new(45.0, 0.0), 2);
    assert_eq!(neighbors.len(), 2);
    assert!((neighbors[0].dist_sq - 25.0).abs() < 1e-10);
    assert!((neighbors[1].dist_sq - 25.0).abs() < 1e-10);
    let mut indices: Vec<usize> = neighbors.iter().map(|n| n.index).collect();
    indices.sort();
    assert_eq!(indices, vec![4, 5]);
}

#[test]
fn vertical_line_exact_distances() {
    // 10 points on y-axis: idx i at (0, 10*i), i=0..10
    // Query: (0, 45)
    //   dist_sq to idx4 (0,40): (45-40)^2 = 25
    //   dist_sq to idx5 (0,50): (45-50)^2 = 25
    // k=2: idx4 and idx5, both at dist_sq=25
    let points: Vec<DVec2> = (0..10).map(|i| DVec2::new(0.0, i as f64 * 10.0)).collect();
    let tree = KdTree::build(&points).unwrap();

    let neighbors = tree.k_nearest(DVec2::new(0.0, 45.0), 2);
    assert_eq!(neighbors.len(), 2);
    assert!((neighbors[0].dist_sq - 25.0).abs() < 1e-10);
    assert!((neighbors[1].dist_sq - 25.0).abs() < 1e-10);
    let mut indices: Vec<usize> = neighbors.iter().map(|n| n.index).collect();
    indices.sort();
    assert_eq!(indices, vec![4, 5]);
}

#[test]
fn large_coordinates() {
    // idx 0: (1024.5, 768.3), idx 1: (2048.1, 1536.7), idx 2: (512.9, 384.2), idx 3: (3072.0, 2304.5)
    // Query: (1024.5, 768.3) — exact match with idx 0
    // k=2: idx 0 (dist_sq=0), next closest:
    //   dist_sq to idx1: (1024.5-2048.1)^2 + (768.3-1536.7)^2 = 1047564.96 + 590790.76 = 1638355.72
    //   dist_sq to idx2: (1024.5-512.9)^2 + (768.3-384.2)^2 = 261793.56 + 147464.81 = 409258.37  (closest)
    //   dist_sq to idx3: (1024.5-3072)^2 + (768.3-2304.5)^2 = 4197556.25 + 2361564.84 = 6559121.09
    let points = [
        DVec2::new(1024.5, 768.3),
        DVec2::new(2048.1, 1536.7),
        DVec2::new(512.9, 384.2),
        DVec2::new(3072.0, 2304.5),
    ];
    let tree = KdTree::build(&points).unwrap();

    let neighbors = tree.k_nearest(DVec2::new(1024.5, 768.3), 2);
    assert_eq!(neighbors.len(), 2);
    assert_eq!(neighbors[0].index, 0);
    assert_eq!(neighbors[0].dist_sq, 0.0);
    assert_eq!(neighbors[1].index, 2);
    // (1024.5-512.9)^2 + (768.3-384.2)^2 = 511.6^2 + 384.1^2 = 261734.56 + 147532.81 = 409267.37
    let dx = 1024.5 - 512.9;
    let dy = 768.3 - 384.2;
    let expected = dx * dx + dy * dy;
    assert!((neighbors[1].dist_sq - expected).abs() < 1e-6);
}
