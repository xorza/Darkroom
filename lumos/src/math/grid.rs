//! The nodes of a sampling grid laid over a frame's pixels.

/// The nodes a grid of `step` lays along an axis of `length` pixels: every `step` from 0, and the
/// last pixel, `length − 1`, once, so the far edge is sampled and no node lies past it.
pub(crate) fn nodes(length: usize, step: usize) -> impl Iterator<Item = usize> + Clone {
    debug_assert!(
        length > 0 && step > 0,
        "{length} pixels at a step of {step}"
    );
    (0..length)
        .step_by(step)
        .chain((!(length - 1).is_multiple_of(step)).then_some(length - 1))
}

#[cfg(test)]
mod tests {
    use crate::math::grid;

    /// Every `step` from 0, then the last pixel once: 100 at 30 adds 99 after 90; 91 at 30 ends on
    /// 90, its last pixel, which is not repeated; a step of 1 is every pixel; one pixel is node 0.
    #[test]
    fn nodes_end_on_the_last_pixel_once() {
        let nodes = |length, step| grid::nodes(length, step).collect::<Vec<_>>();
        assert_eq!(nodes(100, 30), [0, 30, 60, 90, 99]);
        assert_eq!(nodes(91, 30), [0, 30, 60, 90]);
        assert_eq!(nodes(120, 30), [0, 30, 60, 90, 119]);
        assert_eq!(nodes(4, 1), [0, 1, 2, 3]);
        assert_eq!(nodes(1, 30), [0]);
    }
}
