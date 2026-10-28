//! [`ElevationScale`]: the theme's shadows.

use glam::Vec2;
use palantir::{RgbaF32, Shadow};

use crate::gui::theme::palette::Palette;

/// The shadows a surface casts, by how far it sits above the canvas: one recipe a tier, so every
/// card casts the same kind of shadow and every floating surface another.
#[derive(Clone, Debug)]
pub(crate) struct ElevationScale {
    /// A card at rest on the canvas or in a pane: a node, an inspector card.
    pub(crate) resting: Shadow,
    /// A surface over the others: a menu, a tooltip, a modal.
    pub(crate) floating: Shadow,
}

/// The blur of a status frame: a crisp halo, not a bloom.
const FRAME_BLUR: f32 = 3.0;
/// The status frame's spread, so the halo shows past the card's stroke.
const FRAME_SPREAD: f32 = 0.5;

impl ElevationScale {
    pub(super) const fn new(p: &Palette) -> Self {
        Self {
            resting: Shadow::drop(p.shadow, Vec2::new(0.0, 3.0), 10.0),
            floating: Shadow::drop(p.shadow, Vec2::new(0.0, 8.0), 16.0),
        }
    }

    /// The halo a card casts in `ink` in place of its resting shadow, centered so it reads as a
    /// frame around the card.
    pub(crate) const fn frame(&self, ink: RgbaF32) -> Shadow {
        Shadow::drop(ink, Vec2::ZERO, FRAME_BLUR).with_spread(FRAME_SPREAD)
    }
}
