//! [`CanvasTheme`]: the graph canvas ground, the grid ruled across it, and the alarm inks drawn
//! on it.

use palantir::RgbaF32;

use palantir::SpaceSteps;

use crate::gui::theme::palette::Palette;

/// The graph canvas itself — the ground everything else sits on, and the
/// dotted grid ruled across it.
#[derive(Clone, Debug)]
pub(crate) struct CanvasTheme {
    /// Ground fill behind the whole graph.
    pub(crate) bg: RgbaF32,
    /// Backdrop grid dot color.
    pub(crate) dot: RgbaF32,
    /// World-space base spacing between dots, the spacing scale's `lg`, so the grid falls on the
    /// 4 px grid. Wrapped by a power-of-2 multiplier as the user zooms so the field never
    /// collapses into noise — see `gui::pane::graph_ui::canvas_bg`.
    pub(crate) dot_spacing: f32,
    /// A backdrop dot's on-screen radius (px) where the wrapped tile is [`Self::dot_spacing`]
    /// wide. The dot scales with its tile across a wrap band, to twice this where the tile is
    /// twice as wide.
    pub(crate) dot_radius: f32,
    /// A wire whose endpoint no longer resolves, and a node a breaker crosses.
    pub(crate) wire_broken: RgbaF32,
    /// The bar across the middle of a wire the breaker will cut: its length, a port's side, and
    /// its width, a wire's, in [`Self::wire_broken`].
    pub(crate) cut_bar_length: f32,
    pub(crate) cut_bar_width: f32,
    /// The scribble that cuts wires.
    pub(crate) breaker_stroke: RgbaF32,
    /// A wire's width, and every mark drawn at wire weight: the drag preview, the subscription
    /// pin's leader and the breaker scribble that cuts them, so the blade reads at the weight of
    /// what it severs.
    pub(crate) wire: f32,
}

impl CanvasTheme {
    pub(super) const fn from_palette(p: &Palette, space: &SpaceSteps) -> Self {
        Self {
            bg: p.canvas_bg,
            dot: p.canvas_dot,
            dot_spacing: space.lg,
            dot_radius: 0.6,
            wire_broken: p.wire_broken,
            cut_bar_length: space.md,
            cut_bar_width: WIRE,
            breaker_stroke: p.breaker_stroke,
            wire: WIRE,
        }
    }

    /// A wire's width in world units at `zoom`: [`Self::wire`] on screen from 100 % down, and
    /// zoomed with the canvas above, as Blender draws a link at `2.5 * max(zoom, 1)` px
    /// ([drawnode.cc](https://github.com/blender/blender/blob/main/source/blender/editors/space_node/drawnode.cc)),
    /// so a zoomed-out wire never thins under its width.
    pub(crate) fn wire_at(&self, zoom: f32) -> f32 {
        debug_assert!(zoom > 0.0, "a zoom is positive");
        self.wire * zoom.max(1.0) / zoom
    }
}

/// A wire's width: `darkroom/AGENTS.md`, "The look", "A wire is 2.5 px".
const WIRE: f32 = 2.5;
