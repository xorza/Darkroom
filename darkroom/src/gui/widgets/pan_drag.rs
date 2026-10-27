//! A pan drag measured from where it latched.

use glam::Vec2;

/// Write into `pan` the pan a drag of total `delta` gives from the pan `anchor` latched, and end
/// the drag on a `None` delta, its release. Measured from the latch rather than integrated, so a
/// dropped frame cannot drift; an idle anchor drives nothing.
pub(crate) fn fold(anchor: &mut Option<Vec2>, delta: Option<Vec2>, pan: &mut Vec2) {
    let Some(delta) = delta else {
        *anchor = None;
        return;
    };
    if let Some(start) = *anchor {
        *pan = start + delta;
    }
}
