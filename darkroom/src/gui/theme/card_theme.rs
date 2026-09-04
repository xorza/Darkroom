//! [`CardTheme`]: the elevated rounded surface node bodies, the inspector
//! panel and the dock tabs all read from.

use palantir::{RgbaF32, Shadow};

use crate::gui::theme::palette::Palette;

/// An elevated rounded surface: node bodies, the inspector panel, the
/// dock's tabs. Named for the shape rather than the node, because all
/// three read from it — a header band derives its own tighter radius from
/// [`Self::inner_radius`] rather than carrying fields of its own.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct CardTheme {
    /// Body fill.
    pub(crate) fill: RgbaF32,
    /// Resting outline. Transparent: the ambient shadow carries the edge,
    /// and the stroke slot is reserved for the selection / breaker /
    /// missing colours.
    pub(crate) border: RgbaF32,
    /// Header band fill, a step brighter than `fill` so the band reads
    /// against the body. Doubles as the chrome lift behind a hovered strip
    /// glyph.
    pub(crate) header_fill: RgbaF32,
    /// Ambient elevation shadow cast when no status glow claims the slot —
    /// one colour so every elevated surface casts the same kind of shadow.
    pub(crate) ambient_shadow: RgbaF32,
    /// Resting outline width. The drawn stroke is always
    /// [`Self::border_width_total`] — twice this — so selecting never resizes
    /// a card.
    pub(crate) border_width: f32,
    /// How round a card is. A header derives its own from
    /// [`Self::inner_radius`].
    pub(crate) corner_radius: f32,
    /// Minimum content size for a node body. Caps how tightly a node with
    /// very short port labels can shrink horizontally so the header stays
    /// legible at any zoom.
    pub(crate) min_width: f32,
    pub(crate) min_height: f32,
}

/// Result of [`Theme::card_border`](crate::gui::theme::Theme::card_border): the
/// resolved outline color. The width is [`CardTheme::border_width_total`] —
/// constant, so selecting never resizes a card.
#[derive(Clone, Debug)]
pub(crate) struct CardBorder {
    pub(crate) color: RgbaF32,
}

impl CardTheme {
    pub(super) fn from_palette(p: &Palette) -> Self {
        Self {
            fill: p.node_fill,
            border: p.node_border,
            header_fill: p.header_fill,
            ambient_shadow: p.node_ambient_shadow,
            border_width: 1.0,
            corner_radius: 6.0,
            min_width: 160.0,
            min_height: 10.0,
        }
    }

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
