//! [`PaletteColors`]: the chrome colours belonging to no single widget —
//! the surround, the shared inks, and the badge roster.

use palantir::Color;

use crate::core::theme_pref::ThemePreset;
use crate::gui::theme::palette_struct::palette_struct;
use crate::gui::theme::swatches::{dark, light};

palette_struct! {
    /// Chrome colours that belong to no single widget — the surround, the
    /// shared inks, and the badge roster. Serialized as the theme's
    /// `[colors]` table.
    #[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
    pub(crate) struct PaletteColors;
    /// The selection accent: the rubber-band rectangle (translucent fill +
    /// near-opaque 1px border, both derived from this) *and* the selected-
    /// node border, so "in the selection" reads as one color from sweep to
    /// committed halo (palette accent).
    selection_rect: Color => SELECTION_RECT,
    connection_broken: Color => CONNECTION_BROKEN,
    breaker_stroke: Color => BREAKER_STROKE,
    /// Muted secondary foreground (palette `text_muted`, `#aaaaa8`). The
    /// de-emphasized accent shared across chrome: inactive/disabled header
    /// chips, the pinned-inspector outline, and active-tab text — visible
    /// without competing with the bright accent (`badge_graph`) or
    /// full-strength text.
    text_muted: Color => TEXT_MUTED,
    /// Top-chrome fill behind the menu bar + tab strip. A notch darker
    /// than the card surface, sitting between the graph (`canvas.bg`)
    /// and the nodes, so the chrome recedes and the active tab (which
    /// uses `canvas.bg`) reads as continuous with the graph below it.
    chrome_fill: Color => CHROME_FILL,
    /// Inactive tab-strip chip. A notch above `chrome_fill` toward the card
    /// surface, so an unselected tab reads as a resting chip rather than a
    /// bare label; the active tab uses `canvas.bg` + a `selection_rect`
    /// accent top-line instead.
    tab_inactive: Color => TAB_INACTIVE,
    /// Accent cyan: the inspect chip, the pinned-inspector outline, and the
    /// VRAM half of a memory readout.
    badge_graph: Color => BADGE_GRAPH,
    /// Sink chip — error red.
    badge_sink: Color => BADGE_SINK,
    /// RuntimeCache (persist-to-disk) chip — warning yellow.
    badge_cache: Color => BADGE_CACHE,
    /// Impure marker — `constant` purple. A read-only descriptor (the node
    /// recomputes every run and is never cached), not an interactive toggle.
    badge_impure: Color => BADGE_IMPURE,
}

impl PaletteColors {
    /// Rubber-band interior wash — `selection_rect` at 12%, pairing
    /// with [`Self::selection_border`] (the derivation the
    /// `selection_rect` doc promises lives in one place).
    pub(crate) fn selection_fill(&self) -> Color {
        self.selection_rect.with_alpha(0.12)
    }

    /// Rubber-band outline — `selection_rect` near-opaque.
    pub(crate) fn selection_border(&self) -> Color {
        self.selection_rect.with_alpha(0.85)
    }

    /// Soft hairline rule — `text_muted` at 18%, the peer of
    /// palantir's `Palette::border_soft`.
    pub(crate) fn border_soft(&self) -> Color {
        self.text_muted.with_alpha(0.18)
    }
}
