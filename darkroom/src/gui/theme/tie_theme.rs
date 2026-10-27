//! [`TieTheme`]: the inks of a wire tie's band.

use palantir::RgbaF32;

use crate::gui::theme::palette::Palette;

/// A tie's band in the neutral inks, so the wire colours, which are the types, stay clear:
/// `darkroom/AGENTS.md`, "Wire ties", "The band is neutral and paints over wires, under nodes".
#[derive(Clone, Debug)]
pub(crate) struct TieTheme {
    /// The band and its head at rest (palette `text_muted`).
    pub(crate) band: RgbaF32,
    /// The band and its head hovered or selected (palette `text`).
    pub(crate) band_lit: RgbaF32,
    /// The gap between the band and the wires under it (palette `canvas_bg`).
    pub(crate) halo: RgbaF32,
    /// The selection ring, the stem and an auto-angle knob's rim (palette `selection_rect`).
    pub(crate) ring: RgbaF32,
    /// A pinned knob's fill (palette `status_warning`).
    pub(crate) pinned: RgbaF32,
}

impl TieTheme {
    pub(super) const fn from_palette(p: &Palette) -> Self {
        Self {
            band: p.text_muted,
            band_lit: p.text,
            halo: p.canvas_bg,
            ring: p.selection_rect,
            pinned: p.status_warning,
        }
    }
}
