//! The viewer's own drawn vocabulary, past the shared [`Icon`] set: the backdrop swatches and
//! the checkerboard tile behind a transparent image.
//!
//! [`Icon`]: crate::gui::widgets::icon::Icon

use palantir::{Rect, RgbaF32, Stroke, Ui};

use glam::UVec2;
use palantir::widget::Shape;

use crate::core::io::preferences::ViewerBg;
use crate::gui::theme::Theme;
use crate::gui::widgets::toggle::Toggle;

/// The 2×2 checkerboard tile in the theme's checker inks — one full checker period, stamped
/// across the pane via `ImageFit::Tile` + `ImageFilter::Nearest`.
pub(super) fn checker_image(theme: &Theme) -> palantir::Image {
    let [light, dark] =
        [theme.viewer.checker_light, theme.viewer.checker_dark].map(RgbaF32::to_srgba);
    let mut tile = palantir::Image::blank(UVec2::splat(2));
    tile.fill_with(|at| if (at.x + at.y) % 2 == 0 { light } else { dark });
    tile
}

/// A backdrop mode's swatch, ringed with the selection accent while `state`
/// is on.
#[derive(Debug, Clone, Copy)]
pub(super) struct Swatch {
    pub(super) mode: ViewerBg,
    pub(super) state: Toggle,
}

impl Swatch {
    /// A square the toolbar icon's size centered in the `s`-sized box, filled with the mode
    /// itself (mini checker for `Checker`), rounded as a mark, and ringed at one width whose ink
    /// alone says whether it is on.
    pub(super) fn draw(self, ui: &mut Ui, s: f32, theme: &Theme) {
        let Self { mode, state } = self;
        let d = theme.icon.toolbar.icon;
        let o = (s - d) * 0.5;
        let radius = theme.radius.mark;
        let rect = Rect::new(o, o, d, d);
        if let Some(fill) = flat_fill(theme, mode) {
            ui.add_shape(Shape::rect(rect).corners(radius).fill(fill));
        } else {
            let (light, dark) = (theme.viewer.checker_light, theme.viewer.checker_dark);
            ui.add_shape(Shape::rect(rect).corners(radius).fill(dark));
            // Two light quads on the diagonal make the 2×2 mini checker.
            let h = d * 0.5;
            for cell in [Rect::new(o, o, h, h), Rect::new(o + h, o + h, h, h)] {
                ui.add_shape(Shape::rect(cell).fill(light));
            }
        }
        // Ring on top so the checker quads can't cover it.
        let ring = match state {
            Toggle::On => theme.colors.accent,
            Toggle::Off => theme.viewer.swatch_ring,
        };
        ui.add_shape(
            Shape::rect(rect)
                .corners(radius)
                .border(Stroke::new(ring, theme.stroke.line)),
        );
    }
}

/// The single color `mode` paints, or `None` for the checker, which is a
/// pattern. The one mapping the swatch and the viewer's backdrop both read.
pub(super) const fn flat_fill(theme: &Theme, mode: ViewerBg) -> Option<RgbaF32> {
    match mode {
        ViewerBg::Theme => Some(theme.canvas.bg),
        ViewerBg::Black => Some(RgbaF32::BLACK),
        ViewerBg::White => Some(RgbaF32::WHITE),
        ViewerBg::Checker => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every flat backdrop maps to its one color, and the checker, a
    /// pattern, to none.
    #[test]
    fn each_flat_backdrop_has_one_fill_and_the_checker_none() {
        let theme = Theme::default();
        assert_eq!(flat_fill(&theme, ViewerBg::Theme), Some(theme.canvas.bg));
        assert_eq!(flat_fill(&theme, ViewerBg::Black), Some(RgbaF32::BLACK));
        assert_eq!(flat_fill(&theme, ViewerBg::White), Some(RgbaF32::WHITE));
        assert_eq!(flat_fill(&theme, ViewerBg::Checker), None);
    }

    /// The tile is the theme's two checker inks, `#4d4d4d` and `#333333`, as one period.
    #[test]
    fn checker_image_is_one_2x2_period() {
        const L: u8 = 0x4d;
        const D: u8 = 0x33;
        let image = checker_image(&Theme::default());
        // Row-major light/dark, dark/light — one full checker period.
        #[rustfmt::skip]
        let expected = [
            L, L, L, 255,  D, D, D, 255,
            D, D, D, 255,  L, L, L, 255,
        ];
        assert_eq!(
            image,
            palantir::Image::from_srgba8(UVec2::splat(2), expected.to_vec())
                .expect("2×2 RGBA8 is 16 bytes")
        );
    }
}
