//! [`StrokeScale`]: the theme's line widths.

/// The widths every stroke reads, by what the stroke is — the line half of a
/// [`Theme`](crate::gui::theme::Theme), beside the space and radius scales. A literal width at a
/// call site is a missing step.
#[derive(Clone, Debug)]
pub(crate) struct StrokeScale {
    /// A rule or a panel's border: a menu's, the dock ghost's, the rubber band's.
    pub(crate) hairline: f32,
    /// A tie's halo and selection ring.
    pub(crate) fine: f32,
    /// An accent bar, a port's ring, the focus ring and a tab's cap.
    pub(crate) line: f32,
    /// A wire, and every mark drawn at wire weight: the drag preview, the subscription pin's
    /// leader and the breaker scribble that cuts them, so the blade reads at the weight of what it
    /// severs.
    pub(crate) wire: f32,
}

impl StrokeScale {
    /// A wire's width in world units at `zoom`: [`Self::wire`] on screen from 100 % down, and
    /// zoomed with the canvas above, as Blender draws a link at `2.5 * max(zoom, 1)` px
    /// ([drawnode.cc](https://github.com/blender/blender/blob/main/source/blender/editors/space_node/drawnode.cc)),
    /// so a zoomed-out wire never thins under its width.
    pub(crate) fn wire_at(&self, zoom: f32) -> f32 {
        debug_assert!(zoom > 0.0, "a zoom is positive");
        self.wire * zoom.max(1.0) / zoom
    }

    pub(super) const DEFAULT: Self = Self {
        hairline: 1.0,
        fine: 1.5,
        line: 2.0,
        wire: 2.5,
    };
}
