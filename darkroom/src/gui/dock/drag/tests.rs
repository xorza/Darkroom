use super::*;
use crate::core::document::dock::TabGroupId;

fn group() -> TabGroupId {
    TabGroupId::nil()
}

/// Pane 400×300 with a 24-tall strip and two 60-wide chips
/// (centers at 36 and 99); content is (0,24)–(400,300).
fn fixture() -> (Rect, Rect, [Rect; 2]) {
    let pane = Rect::new(0.0, 0.0, 400.0, 300.0);
    let strip = Rect::new(0.0, 0.0, 400.0, 24.0);
    let chips = [
        Rect::new(6.0, 4.0, 60.0, 20.0),
        Rect::new(69.0, 4.0, 60.0, 20.0),
    ];
    (pane, strip, chips)
}

fn classify(p: Vec2, can_split: bool) -> DropTarget {
    let (pane, strip, chips) = fixture();
    classify_drop(
        PaneGeometry {
            group: group(),
            pane,
            strip,
            chips: &chips,
            can_split,
        },
        p,
    )
}

#[test]
fn strip_hover_picks_insertion_slots_by_chip_centers() {
    // Left of chip 0's center (36) → slot 0, caret on its left edge.
    let t = classify(Vec2::new(20.0, 10.0), true);
    assert_eq!(
        t.drop,
        DockDrop::Into {
            group: group(),
            index: 0
        }
    );
    assert_eq!(t.highlight, Rect::new(3.0, 2.0, 3.0, 22.0));

    // Between the centers (36..99) → slot 1, caret at chip 1's edge.
    let t = classify(Vec2::new(50.0, 10.0), true);
    assert_eq!(
        t.drop,
        DockDrop::Into {
            group: group(),
            index: 1
        }
    );
    assert_eq!(t.highlight, Rect::new(66.0, 2.0, 3.0, 22.0));

    // Right of every center → append (slot 2), caret after chip 1
    // (max x 129).
    let t = classify(Vec2::new(300.0, 10.0), true);
    assert_eq!(
        t.drop,
        DockDrop::Into {
            group: group(),
            index: 2
        }
    );
    assert_eq!(t.highlight, Rect::new(129.0, 2.0, 3.0, 22.0));
}

#[test]
fn content_center_joins_and_edges_split() {
    // Content (0,24,400,276); center box (100,93,200,138). Dead
    // center joins with an append and highlights the whole content.
    let t = classify(Vec2::new(200.0, 160.0), true);
    assert_eq!(
        t.drop,
        DockDrop::Into {
            group: group(),
            index: 2
        }
    );
    assert_eq!(t.highlight, Rect::new(0.0, 24.0, 400.0, 276.0));

    // Each outer band splits toward its edge, highlighting that
    // half of the content.
    let cases = [
        (
            Vec2::new(30.0, 160.0),
            SplitSide::Left,
            Rect::new(0.0, 24.0, 200.0, 276.0),
        ),
        (
            Vec2::new(370.0, 160.0),
            SplitSide::Right,
            Rect::new(200.0, 24.0, 200.0, 276.0),
        ),
        (
            Vec2::new(200.0, 40.0),
            SplitSide::Top,
            Rect::new(0.0, 24.0, 400.0, 138.0),
        ),
        (
            Vec2::new(200.0, 290.0),
            SplitSide::Bottom,
            Rect::new(0.0, 162.0, 400.0, 138.0),
        ),
    ];
    for (p, side, half) in cases {
        let t = classify(p, true);
        assert_eq!(
            t.drop,
            DockDrop::Split {
                group: group(),
                side
            },
            "{side:?} zone at {p}"
        );
        assert_eq!(t.highlight, half, "{side:?} highlights its half");
    }
}

#[test]
fn nesting_cap_degrades_edges_to_a_join() {
    // Same left-band pointer, but the pane can't split: join.
    let t = classify(Vec2::new(30.0, 160.0), false);
    assert_eq!(
        t.drop,
        DockDrop::Into {
            group: group(),
            index: 2
        }
    );
    assert_eq!(t.highlight, Rect::new(0.0, 24.0, 400.0, 276.0));
}
