//! [`CanvasTheme`]: the graph canvas ground and the grid ruled across it.

use palantir::Color;

use crate::core::theme_pref::ThemePreset;
use crate::gui::theme::palette_struct::palette_struct;
use crate::gui::theme::swatches::{dark, light};

palette_struct! {
    /// The graph canvas itself — the ground everything else sits on, and the
    /// dotted grid ruled across it.
    #[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
    pub(crate) struct CanvasTheme;
    /// Ground fill behind the whole graph.
    bg: Color => CANVAS_BG,
    /// Backdrop grid dot colour.
    dot: Color => CANVAS_DOT,
    ;
    /// World-space base spacing between dots. Wrapped by a power-of-2
    /// multiplier as the user zooms so the field never collapses into noise —
    /// see `gui::pane::graph::background`.
    dot_spacing: f32 = 18.0,
    /// On-screen radius (px) of a backdrop dot.
    dot_radius: f32 = 0.6,
}
