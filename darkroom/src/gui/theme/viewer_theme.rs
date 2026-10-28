//! [`ViewerTheme`]: what the image viewer paints past the shared widgets.

use palantir::RgbaF32;

use crate::gui::theme::inks::Inks;
use crate::gui::theme::palette::Palette;
use crate::gui::theme::space_scale::SpaceScale;

/// The resting swatch ring's alpha: visible on every backdrop, louder on none. A translucent
/// layer, not a wash, as the swatch it rings shows any of four backdrops.
const SWATCH_RING_ALPHA: f32 = 0.4;

/// The image viewer's own inks: the checker behind a transparent image and the ring around a
/// backdrop swatch.
#[derive(Clone, Debug)]
pub(crate) struct ViewerTheme {
    /// The checker's light square.
    pub(crate) checker_light: RgbaF32,
    /// The checker's dark square.
    pub(crate) checker_dark: RgbaF32,
    /// The side of one checker square on screen, fixed as the image pans and zooms: it is a
    /// transparency reference, not content.
    pub(crate) checker_square: f32,
    /// The ring around a backdrop swatch that is not picked.
    pub(crate) swatch_ring: RgbaF32,
}

impl ViewerTheme {
    pub(super) const fn new(p: &Palette, space: &SpaceScale, ink: &Inks) -> Self {
        Self {
            checker_light: p.checker_light,
            checker_dark: p.checker_dark,
            checker_square: space.sm,
            swatch_ring: ink.muted.with_alpha(SWATCH_RING_ALPHA),
        }
    }
}
