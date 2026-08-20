//! [`CardTheme`]: the elevated rounded surface node bodies, the inspector
//! panel and the dock tabs all read from.

use palantir::Color;
use palantir::Shadow;

use crate::core::theme_pref::ThemePreset;
use crate::gui::theme::palette_struct::palette_struct;
use crate::gui::theme::swatches::{dark, light};

palette_struct! {
    /// An elevated rounded surface: node bodies, the inspector panel, the
    /// dock's tabs. Named for the shape rather than the node, because all
    /// three read from it — a header band derives its own tighter radius from
    /// [`Self::inner_radius`] rather than carrying fields of its own.
    #[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
    pub(crate) struct CardTheme;
    /// Body fill.
    fill: Color => NODE_FILL,
    /// Resting outline. Transparent in the dark preset, where the ambient
    /// shadow carries the edge and the stroke slot is reserved for the
    /// selection / breaker / missing colours.
    border: Color => NODE_BORDER,
    /// Header band fill, a step brighter than `fill` so the band reads
    /// against the body. Doubles as the chrome lift behind a hovered strip
    /// glyph.
    header_fill: Color => HEADER_FILL,
    /// Ambient elevation shadow cast when no status glow claims the slot —
    /// one swatch so every elevated surface casts the same kind of shadow.
    ambient_shadow: Color => NODE_AMBIENT_SHADOW,
    ;
    /// Resting outline width. The drawn stroke is always
    /// [`Self::border_width_total`] — twice this — so selecting never resizes
    /// a card.
    border_width: f32 = 1.0,
    /// How round a card is. A header derives its own from
    /// [`Self::inner_radius`].
    corner_radius: f32 = 6.0,
    /// Minimum content size for a node body. Caps how tightly a node with
    /// very short port labels can shrink horizontally so the header stays
    /// legible at any zoom.
    min_width: f32 = 160.0,
    min_height: f32 = 10.0,
}

/// Result of [`Theme::card_border`](crate::gui::theme::Theme::card_border): the resolved outline color. The width is
/// [`CardTheme::border_width_total`] — constant, so selecting never resizes a
/// card.
#[derive(Clone, Debug)]
pub(crate) struct CardBorder {
    pub(crate) color: Color,
}

impl CardTheme {
    /// The stroke width a card actually draws — always the *selection* width
    /// (`border_width * 2`) regardless of selection state, so selecting one
    /// never resizes it (only its colour changes). Named so the doubling
    /// can't drift between the call sites that must agree on it: the stroke
    /// itself, [`Self::inner_radius`], and [`Theme::port_overhang_for`](crate::gui::theme::Theme::port_overhang_for).
    #[inline]
    pub(crate) fn border_width_total(&self) -> f32 {
        self.border_width * 2.0
    }

    /// Inner corner radius for a header or footer strip seating flush against
    /// the card's own outer stroke — a node body rounds its header/footer band
    /// to this, not the raw `corner_radius`, else the strip's corner leaves a
    /// wedge of plain fill showing between it and the (selection-lit) stroke.
    #[inline]
    pub(crate) fn inner_radius(&self) -> f32 {
        (self.corner_radius - self.border_width_total()).max(0.0)
    }

    /// Ambient elevation shadow shared by every card — node bodies, inspector
    /// panels — so they all read as the same kind of surface. Only the blur
    /// scales with how high a surface sits; colour and offset are fixed.
    #[inline]
    pub(crate) fn elevation_shadow(&self, blur: f32) -> Shadow {
        Shadow::drop(self.ambient_shadow, glam::Vec2::new(0.0, 3.0), blur)
    }
}
