//! Tab drag-and-drop between dock panes: the gesture state plus the
//! pure pointer→drop-zone classification. The gesture itself is driven
//! by [`DockUi`](super::DockUi) — armed in the navigation scan off a chip's
//! `drag_started`, resolved on `drag_stopped` into a
//! `DockOp::MoveTab`, and painted during record as a drop-zone
//! highlight + a ghost chip on the tooltip layer. Everything
//! decision-shaped lives here as rect math so it's testable without a
//! `Ui`.

use glam::Vec2;
use palantir::Rect;

use crate::core::document::TabRef;
use crate::core::document::dock::TabGroupId;
use crate::core::document::dock::dock_op::DockDrop;
use crate::core::document::dock::split_side::SplitSide;

/// A tab mid-drag: armed when the tab's chip latches, cleared on release or
/// Esc. Nothing here is positional — `tab` is keyed by identity, never by strip
/// slot, so an undo that rearranges the strip mid-drag can't strand the gesture
/// on a slot the tab has left.
#[derive(Debug)]
pub(super) struct TabDrag {
    pub(super) tab: TabRef,
    /// Label for the ghost chip, snapshotted at arm time.
    pub(super) text: String,
}

/// Where a drop over a pane would land, plus the region to highlight
/// while hovering it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct DropTarget {
    pub(super) drop: DockDrop,
    pub(super) highlight: Rect,
}

/// Insertion-caret breadth in the strip, logical px.
const CARET_W: f32 = 3.0;

/// One pane's last-frame geometry, as [`classify_drop`] needs it. A
/// struct rather than four positional parameters: `pane` and `strip` are
/// both `Rect`, so transposing them type-checks and yields a plausible
/// but wrong classification.
#[derive(Clone, Copy, Debug)]
pub(super) struct PaneGeometry<'a> {
    pub(super) group: TabGroupId,
    /// The whole pane — strip row and content together.
    pub(super) pane: Rect,
    /// The strip row alone, along the pane's top edge.
    pub(super) strip: Rect,
    /// The strip's chip rects, in tab order.
    pub(super) chips: &'a [Rect],
    /// Whether this pane may still split (the nesting cap) — when it
    /// can't, every edge zone degrades to a join.
    pub(super) can_split: bool,
}

/// Classify pointer `p` against one pane (the caller already
/// established `p` is over it): the tab strip yields an insertion slot
/// between chips, the content's inner half joins the group (append),
/// and the outer band splits toward the nearest edge — unless the pane
/// sits at the nesting cap (`can_split` false), where everything
/// degrades to a join. `chips` are the strip's chip rects in tab order.
pub(super) fn classify_drop(pane: PaneGeometry<'_>, p: Vec2) -> DropTarget {
    let PaneGeometry {
        group,
        pane,
        strip,
        chips,
        can_split,
    } = pane;
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
mod tests;
