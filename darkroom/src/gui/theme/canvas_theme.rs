//! [`CanvasTheme`]: the graph canvas ground and the grid ruled across it.

use palantir::Color;

use crate::gui::theme::palette::Palette;

/// The graph canvas itself — the ground everything else sits on, and the
/// dotted grid ruled across it.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct CanvasTheme {
    /// Ground fill behind the whole graph.
    pub(crate) bg: Color,
    /// Backdrop grid dot colour.
    pub(crate) dot: Color,
    /// World-space base spacing between dots. Wrapped by a power-of-2
    /// multiplier as the user zooms so the field never collapses into noise —
    /// see `gui::pane::graph::background`.
    pub(crate) dot_spacing: f32,
    /// On-screen radius (px) of a backdrop dot.
    pub(crate) dot_radius: f32,
}

impl CanvasTheme {
    pub(super) fn from_palette(p: &Palette) -> Self {
        Self {
            bg: p.canvas_bg,
            dot: p.canvas_dot,
            dot_spacing: 18.0,
            dot_radius: 0.6,
        }
    }
}
