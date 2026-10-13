use palantir::Size;

use super::*;

/// A scribble started at `p`, the way `BreakerUI::apply` starts one — the
/// unit under test in everything below, which is about the polyline rather
/// than about the gesture that drives it.
fn scribble_at(p: Vec2) -> Scribble {
    let mut s = Scribble::default();
    s.restart(p);
    s
}

#[test]
fn begin_frame_clears_every_broken_collection() {
    // A target crossed once mid-drag must not stay marked after the scribble
    // moves away, or the release severs it anyway: `begin_frame` is the one
    // place all three collections clear.
    let mut b = scribble_at(Vec2::ZERO);
    let node = NodeId::from_u128(1);
    b.broken.push(InputPort::new(node, 0));
    b.broken_nodes.push(node);
    b.broken_subscriptions.push(Subscription {
        emitter: node,
        event_idx: 0,
        subscriber: node,
    });

    b.begin_frame();

    assert!(b.broken.is_empty());
    assert!(b.broken_nodes.is_empty());
    assert!(b.broken_subscriptions.is_empty());
}

#[test]
fn add_point_skips_short_segments() {
    // Samples below MIN_POINT_DISTANCE are dropped — a slow drag
    // that crawls 1px/frame must not accumulate one point per frame.
    let mut b = scribble_at(Vec2::ZERO);
    b.add_point(Vec2::new(1.0, 0.0));
    b.add_point(Vec2::new(2.0, 0.0));
    b.add_point(Vec2::new(3.0, 0.0));
    assert_eq!(b.points.len(), 1, "sub-4px samples must be dropped");
    b.add_point(Vec2::new(10.0, 0.0));
    assert_eq!(b.points.len(), 2);
}

#[test]
fn add_point_caps_total_length() {
    // Past MAX_BREAKER_LENGTH the last segment is clamped and further pushes
    // are no-ops. From 0, a push to (3000, 0) has seg = 3000 > remaining =
    // 2000, so t = 2000/3000. That rounds up to 0.66666669 in f32, and
    // 3000 · t = 2000.00006 is closer to 2000 than to the next f32 (2000.000122),
    // so the clamped point lands at exactly (2000, 0).
    let mut b = scribble_at(Vec2::ZERO);
    b.add_point(Vec2::new(3000.0, 0.0));
    assert_eq!(b.points, [Vec2::ZERO, Vec2::new(MAX_BREAKER_LENGTH, 0.0)]);
    assert_eq!(b.length, MAX_BREAKER_LENGTH);
    b.add_point(Vec2::new(4000.0, 0.0));
    assert_eq!(b.points.len(), 2, "no append past cap");
}

/// A polyline crosses a wire only when one of its segments properly crosses
/// one of the wire's chords.
#[test]
fn intersects_cubic_finds_a_proper_crossing() {
    let straight = [
        Vec2::new(0.0, 0.0),
        Vec2::new(33.0, 0.0),
        Vec2::new(66.0, 0.0),
        Vec2::new(100.0, 0.0),
    ];
    let cases: [(&[Vec2], bool, &str); 3] = [
        (
            &[Vec2::new(50.0, -10.0), Vec2::new(50.0, 10.0)],
            true,
            "a vertical stroke crosses at (50, 0), clear of any chord's end",
        ),
        (
            &[Vec2::new(0.0, 50.0), Vec2::new(100.0, 50.0)],
            false,
            "a parallel stroke 50 below never meets the wire",
        ),
        (
            &[Vec2::new(50.0, 0.0)],
            false,
            "a single point has no segment to cross with",
        ),
    ];
    for (points, expected, why) in cases {
        let mut b = scribble_at(points[0]);
        for &p in &points[1..] {
            b.add_point(p);
        }
        let [p0, p1, p2, p3] = straight;
        assert_eq!(b.intersects_cubic(p0, p1, p2, p3), expected, "{why}");
    }
}

/// The point of keeping the scribble beside the slot rather than inside it: a
/// finished gesture leaves its buffers for the next one instead of returning
/// them to the allocator.
///
/// 49 samples 10 units apart — clear of `MIN_POINT_DISTANCE` (4) so every one
/// is kept, and 490 total, well under `MAX_BREAKER_LENGTH` (2000) so none is
/// clamped away. That gives 50 points including the start, so the buffer has
/// grown past any small-vec default by the time the gesture ends.
#[test]
fn restarting_a_scribble_keeps_the_buffer_the_last_one_grew() {
    let mut s = scribble_at(Vec2::ZERO);
    for i in 1..50 {
        s.add_point(Vec2::new(i as f32 * 10.0, 0.0));
    }
    assert_eq!(s.points.len(), 50, "every sample cleared both thresholds");
    let grown = s.points.capacity();
    assert!(grown >= 50, "the buffer holds what it collected: {grown}");

    s.restart(Vec2::new(7.0, 7.0));
    assert_eq!(
        s.points.capacity(),
        grown,
        "a fresh gesture reuses the last one's allocation"
    );
    assert_eq!(
        s.points.as_slice(),
        &[Vec2::new(7.0, 7.0)],
        "and starts a genuinely new polyline in it"
    );
    assert_eq!(s.length, 0.0, "with the length accumulator back to zero");
}

/// The bounding-box rejection only skips work: over targets placed all
/// around a zigzag scribble — clear of its box, touching its edges exactly,
/// and across it — every answer equals the segment tests run alone.
#[test]
fn the_box_rejection_answers_as_the_segment_tests_do() {
    let mut b = scribble_at(Vec2::new(0.0, 0.0));
    for p in [(20.0, 30.0), (40.0, -10.0), (60.0, 25.0), (80.0, 0.0)] {
        b.add_point(Vec2::from(p));
    }
    assert_eq!((b.lo, b.hi), (Vec2::new(0.0, -10.0), Vec2::new(80.0, 30.0)));

    let brute_rect = |rect: Rect| {
        let (min, max) = (rect.min, rect.max());
        let corners = [min, Vec2::new(max.x, min.y), max, Vec2::new(min.x, max.y)];
        b.points
            .iter()
            .any(|p| p.x >= min.x && p.x <= max.x && p.y >= min.y && p.y <= max.y)
            || b.segments().any(|(a, c)| {
                (0..4).any(|i| segments_intersect(a, c, corners[i], corners[(i + 1) % 4]))
            })
    };
    let brute_cubic = |p0: Vec2, p1: Vec2, p2: Vec2, p3: Vec2| {
        (1..=BEZIER_SAMPLES).any(|i| {
            let t0 = (i - 1) as f32 / BEZIER_SAMPLES as f32;
            let t1 = i as f32 / BEZIER_SAMPLES as f32;
            let (a, c) = (
                if i == 1 {
                    p0
                } else {
                    cubic_point(p0, p1, p2, p3, t0)
                },
                cubic_point(p0, p1, p2, p3, t1),
            );
            b.segments()
                .any(|(s0, s1)| segments_intersect(a, c, s0, s1))
        })
    };

    // Corners on a grid from well outside the box to well inside it, with the
    // box's own edges (-10, 0, 30, 80) among the coordinates.
    let coords = [-30.0, -10.0, 0.0, 15.0, 30.0, 50.0, 80.0, 100.0];
    let (mut rect_hits, mut cubic_hits) = (0, 0);
    for &x in &coords {
        for &y in &coords {
            let rect = Rect {
                min: Vec2::new(x, y),
                size: Size::new(12.0, 12.0),
            };
            assert_eq!(
                b.intersects_rect(rect),
                brute_rect(rect),
                "rect at ({x}, {y})"
            );
            rect_hits += usize::from(brute_rect(rect));

            let (p0, p3) = (Vec2::new(x, y), Vec2::new(x + 40.0, y + 20.0));
            let (p1, p2) = (p0 + Vec2::new(15.0, -5.0), p3 - Vec2::new(15.0, -5.0));
            assert_eq!(
                b.intersects_cubic(p0, p1, p2, p3),
                brute_cubic(p0, p1, p2, p3),
                "cubic from ({x}, {y})"
            );
            cubic_hits += usize::from(brute_cubic(p0, p1, p2, p3));
        }
    }
    // Both kinds of answer occur, so the sweep tests the rejection and the
    // segment tests behind it alike.
    assert!(rect_hits > 0 && rect_hits < coords.len() * coords.len());
    assert!(cubic_hits > 0 && cubic_hits < coords.len() * coords.len());
}
