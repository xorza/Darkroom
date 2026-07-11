//! Tab drag-and-drop between dock panes: the gesture state plus the
//! pure pointer→drop-zone classification. The gesture itself is driven
//! by `MainWindow` — armed in the navigation scan off a chip's
//! `drag_started`, resolved on `drag_stopped` into a
//! `UiAction::MoveTab`, and painted during record as a drop-zone
//! highlight + a ghost chip on the tooltip layer. Everything
//! decision-shaped lives here as rect math so it's testable without a
//! `Ui`.

use aperture::{Rect, SmolStr};
use glam::Vec2;

use crate::core::document::TabRef;
use crate::core::document::dock::{DockDrop, SplitSide, TabGroupId};

/// A tab mid-drag: armed when a movable chip's drag latches, cleared on
/// release or Esc.
#[derive(Debug)]
pub(crate) struct TabDrag {
    pub(crate) tab: TabRef,
    /// Where the chip lived when the drag latched — polled for the
    /// release edge (the layout can't change mid-drag, so the address
    /// stays valid).
    pub(crate) source: (TabGroupId, usize),
    /// Label for the ghost chip, snapshotted at arm time.
    pub(crate) text: SmolStr,
}

/// Where a drop over a pane would land, plus the region to highlight
/// while hovering it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct DropTarget {
    pub(crate) drop: DockDrop,
    pub(crate) highlight: Rect,
}

/// Insertion-caret breadth in the strip, logical px.
const CARET_W: f32 = 3.0;

/// Classify pointer `p` against one pane (the caller already
/// established `p` is over it): the tab strip yields an insertion slot
/// between chips, the content's inner half joins the group (append),
/// and the outer band splits toward the nearest edge — unless the pane
/// sits at the nesting cap (`can_split` false), where everything
/// degrades to a join. `chips` are the strip's chip rects in tab order.
pub(crate) fn classify_drop(
    group: TabGroupId,
    pane: Rect,
    strip: Rect,
    chips: &[Rect],
    can_split: bool,
    p: Vec2,
) -> DropTarget {
    if strip.contains(p) {
        let index = chips.iter().filter(|c| c.center().x < p.x).count();
        return DropTarget {
            drop: DockDrop::Into { group, index },
            highlight: caret_rect(strip, chips, index),
        };
    }

    let content = Rect::new(
        pane.min.x,
        strip.max().y,
        pane.size.w,
        (pane.max().y - strip.max().y).max(0.0),
    );
    let join = DropTarget {
        drop: DockDrop::Into {
            group,
            index: chips.len(),
        },
        highlight: content,
    };
    if !can_split || center_box(content).contains(p) {
        return join;
    }

    // Outer band: split toward the nearest edge (normalized, so wide
    // panes don't bias toward top/bottom).
    let w = content.size.w.max(1.0);
    let h = content.size.h.max(1.0);
    let edges = [
        (SplitSide::Left, (p.x - content.min.x) / w),
        (SplitSide::Right, (content.max().x - p.x) / w),
        (SplitSide::Top, (p.y - content.min.y) / h),
        (SplitSide::Bottom, (content.max().y - p.y) / h),
    ];
    let (side, _) = edges
        .into_iter()
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .expect("four candidate edges");
    DropTarget {
        drop: DockDrop::Split { group, side },
        highlight: half_rect(content, side),
    }
}

/// The inner 50%-per-axis box of `content` — the join zone.
fn center_box(content: Rect) -> Rect {
    Rect::new(
        content.min.x + content.size.w * 0.25,
        content.min.y + content.size.h * 0.25,
        content.size.w * 0.5,
        content.size.h * 0.5,
    )
}

/// The half of `content` a split on `side` would give the dragged tab.
fn half_rect(content: Rect, side: SplitSide) -> Rect {
    let Rect { min, size } = content;
    match side {
        SplitSide::Left => Rect::new(min.x, min.y, size.w * 0.5, size.h),
        SplitSide::Right => Rect::new(min.x + size.w * 0.5, min.y, size.w * 0.5, size.h),
        SplitSide::Top => Rect::new(min.x, min.y, size.w, size.h * 0.5),
        SplitSide::Bottom => Rect::new(min.x, min.y + size.h * 0.5, size.w, size.h * 0.5),
    }
}

/// The insertion caret between the strip's chips: on the boundary of
/// slot `index` (before `chips[index]`, or after the last chip for an
/// append). An empty strip can't happen (no group is empty), but
/// degrade to the strip's left inset if it ever does.
fn caret_rect(strip: Rect, chips: &[Rect], index: usize) -> Rect {
    let x = match (chips.get(index), chips.last()) {
        (Some(next), _) => next.min.x - 1.5,
        (None, Some(last)) => last.max().x + 1.5,
        (None, None) => strip.min.x + 6.0,
    };
    Rect::new(
        x - CARET_W * 0.5,
        strip.min.y + 2.0,
        CARET_W,
        (strip.size.h - 2.0).max(0.0),
    )
}

#[cfg(test)]
mod tests {
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
        classify_drop(group(), pane, strip, &chips, can_split, p)
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
}
