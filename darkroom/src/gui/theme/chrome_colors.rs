//! [`ChromeColors`]: the colours belonging to no single widget — the
//! surround, the shared inks, and the badge roster.

use palantir::RgbaF32;

use crate::gui::theme::palette::Palette;

/// Chrome colours that belong to no single widget — the surround, the
/// shared inks, and the badge roster. Serialized as the theme's
/// `[colors]` table.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct ChromeColors {
    /// The selection accent: the rubber-band rectangle (translucent fill +
    /// near-opaque 1px border, both derived from this) *and* the selected-
    /// node border, so "in the selection" reads as one color from sweep to
    /// committed halo (palette accent).
    pub(crate) selection_rect: RgbaF32,
    pub(crate) connection_broken: RgbaF32,
    pub(crate) breaker_stroke: RgbaF32,
    /// Muted secondary foreground (palette `text_muted`). The
    /// de-emphasized accent shared across chrome: inactive/disabled header
    /// chips, the pinned-inspector outline, and active-tab text — visible
    /// without competing with the bright accent (`badge_graph`) or
    /// full-strength text.
    pub(crate) text_muted: RgbaF32,
    /// Top-chrome fill behind the menu bar + tab strip. A notch darker
    /// than the card surface, sitting between the graph (`canvas.bg`)
    /// and the nodes, so the chrome recedes and the active tab (which
    /// uses `canvas.bg`) reads as continuous with the graph below it.
    pub(crate) chrome_fill: RgbaF32,
    /// Inactive tab-strip chip. A notch above `chrome_fill` toward the card
    /// surface, so an unselected tab reads as a resting chip rather than a
    /// bare label; the active tab uses `canvas.bg` + a `selection_rect`
    /// accent top-line instead.
    pub(crate) tab_inactive: RgbaF32,
    /// Accent cyan: the inspect chip, the pinned-inspector outline, and the
    /// VRAM half of a memory readout.
    pub(crate) badge_graph: RgbaF32,
    /// Sink chip — error red.
    pub(crate) badge_sink: RgbaF32,
    /// RuntimeCache (persist-to-disk) chip — warning yellow.
    pub(crate) badge_cache: RgbaF32,
    /// Impure marker. A read-only descriptor (the node recomputes every run
    /// and is never cached), not an interactive toggle.
    pub(crate) badge_impure: RgbaF32,
}

impl ChromeColors {
    pub(super) fn from_palette(p: &Palette) -> Self {
        Self {
            selection_rect: p.selection_rect,
            connection_broken: p.connection_broken,
            breaker_stroke: p.breaker_stroke,
            text_muted: p.text_muted,
            chrome_fill: p.chrome_fill,
            tab_inactive: p.tab_inactive,
            badge_graph: p.badge_graph,
            badge_sink: p.badge_sink,
            badge_cache: p.badge_cache,
            badge_impure: p.badge_impure,
        }
    }

    /// Rubber-band interior wash — `selection_rect` at 12%, pairing
    /// with [`Self::selection_border`] (the derivation the
    /// `selection_rect` doc promises lives in one place).
    pub(crate) fn selection_fill(&self) -> RgbaF32 {
        self.selection_rect.with_alpha(0.12)
    }

    /// Rubber-band outline — `selection_rect` near-opaque.
    pub(crate) fn selection_border(&self) -> RgbaF32 {
        self.selection_rect.with_alpha(0.85)
    }

    /// Soft hairline rule — `text_muted` at 18%, the peer of
    /// palantir's `Palette::border_soft`.
    pub(crate) fn border_soft(&self) -> RgbaF32 {
        self.text_muted.with_alpha(0.18)
    }
}
