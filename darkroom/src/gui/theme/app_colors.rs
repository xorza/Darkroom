//! [`AppColors`]: the colors belonging to no single widget — the selection accent, the surround
//! and the link.

use palantir::RgbaF32;

use crate::gui::theme::color;
use crate::gui::theme::palette::Palette;

/// How far a hovered link pulls toward white, further than a port's hover lift: a link is text,
/// whose thin strokes need the larger step to read as lit.
const LINK_HOVER_LIFT: f32 = 0.5;

/// The colors that belong to no single widget.
#[derive(Clone, Debug)]
pub(crate) struct AppColors {
    /// The accent: the inspect button, the inspector's pin, a memory readout's VRAM pool, and the
    /// selection: the rubber band (a translucent fill and a near-opaque border, both
    /// derived from this) and the selected node's border, so "in the selection" reads as one
    /// color from sweep to committed halo.
    pub(crate) accent: RgbaF32,
    /// The band the menu bar, the status bar and every tab strip sit on: between the graph
    /// (`canvas.bg`) and the nodes, so the surround recedes and the selected tab, which takes
    /// `canvas.bg`, reads as one with the graph below it.
    pub(crate) surround_bg: RgbaF32,
    /// A text link at rest, and lifted halfway to white under the pointer.
    pub(crate) link: RgbaF32,
    pub(crate) link_hovered: RgbaF32,
}

impl AppColors {
    pub(super) const fn from_palette(p: &Palette) -> Self {
        Self {
            accent: p.accent,
            surround_bg: p.surround_bg,
            link: p.link,
            link_hovered: color::toward(p.link, RgbaF32::WHITE, LINK_HOVER_LIFT),
        }
    }

    /// Rubber-band interior — `accent` at 12%, pairing with [`Self::selection_border`].
    /// A translucent layer, not a wash: the nodes and wires the band sweeps show through it.
    pub(crate) const fn selection_fill(&self) -> RgbaF32 {
        self.accent.with_alpha(0.12)
    }

    /// Rubber-band outline — `accent` near-opaque.
    pub(crate) const fn selection_border(&self) -> RgbaF32 {
        self.accent.with_alpha(0.85)
    }
}
