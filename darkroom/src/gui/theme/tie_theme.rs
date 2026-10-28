//! [`TieTheme`]: the inks of a wire tie's band.

use palantir::RgbaF32;

use crate::gui::theme::palette::Palette;
use crate::gui::theme::tint_scale::TintScale;

/// A tie's band in the neutral inks, so the wire colours, which are the types, stay clear:
/// `darkroom/AGENTS.md`, "Wire ties", "The band is neutral and paints over wires, under nodes".
#[derive(Clone, Debug)]
pub(crate) struct TieTheme {
    /// The band and its head at rest (palette `text_muted`).
    pub(crate) band: RgbaF32,
    /// The band and its head hovered or selected (palette `text`).
    pub(crate) band_lit: RgbaF32,
    /// The band a tie line in flight will put, over the wires it ties: [`Self::band`] at the
    /// tint scale's `strong`, translucent, as the wires under it are content the band does not
    /// know.
    pub(crate) band_preview: RgbaF32,
    /// The gap between the band and the wires under it (palette `canvas_bg`).
    pub(crate) halo: RgbaF32,
    /// The selection ring, the stem and an auto-angle knob's rim (palette `accent`).
    pub(crate) ring: RgbaF32,
    /// A pinned knob's fill (palette `status_warning`).
    pub(crate) pinned: RgbaF32,
}

impl TieTheme {
    pub(super) const fn from_palette(p: &Palette, tint: &TintScale) -> Self {
        Self {
            band: p.text_muted,
            band_lit: p.text,
            band_preview: p.text_muted.with_alpha(tint.strong),
            halo: p.canvas_bg,
            ring: p.accent,
            pinned: p.status_warning,
        }
    }
}
