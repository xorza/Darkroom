//! The viewer's drawn vocabulary: the four control-button glyphs and the
//! checkerboard tile behind a transparent image.
//!
//! Every one is a pure function of the button side `s` and an ink color,
//! built from `s * k` factors so a glyph fills its box at any button size.
//! That holds for the one glyph that is *text* rather than primitives
//! ([`draw_100`]) too — it takes a share of `s` like its siblings rather than
//! sitting on a [`TypeScale`] tier, because it is sized to a box and not to
//! the reading hierarchy.
//!
//! [`TypeScale`]: crate::gui::theme::type_scale::TypeScale

use palantir::SrgbaU8;
use palantir::prelude::*;
use palantir::widget::Shape;

use crate::core::io::preferences::ViewerBg;
use crate::gui::theme::Theme;
use crate::gui::widgets::support;
use crate::gui::widgets::toggle::Toggle;

/// The "1:1" glyph's share of its button box — the text peer of the `s * k`
/// factors its drawn siblings are built from, so it scales with the box rather
/// than sitting on a `TypeScale` tier.
const LABEL_GLYPH_FILL: f32 = 0.37;

/// On-screen side of one checkerboard square, logical px. Screen-fixed
/// (doesn't pan/zoom with the image) — it's a transparency reference,
/// not content.
pub(super) const CHECKER_SQUARE_PX: f32 = 8.0;

/// Checkerboard grays (sRGB bytes) — shared by the backdrop tile and
/// its control-panel swatch. Fixed regardless of theme: the checker is
/// a neutral transparency reference, not a surface.
const CHECKER_LIGHT_U8: u8 = 77; // #4d4d4d
const CHECKER_DARK_U8: u8 = 51; // #333333

/// The 2×2 checkerboard tile — one full checker period, stamped across
/// the pane via `ImageFit::Tile` + `ImageFilter::Nearest`.
pub(super) fn checker_image() -> palantir::Image {
    const L: u8 = CHECKER_LIGHT_U8;
    const D: u8 = CHECKER_DARK_U8;
    let mut tile = palantir::Image::blank(UVec2::splat(2));
    tile.fill_with(|x, y| {
        let gray = if (x + y) % 2 == 0 { L } else { D };
        SrgbaU8::rgb(gray, gray, gray)
    });
    tile
}

/// Four inward corner brackets — "fit the image to the view".
pub(super) fn draw_fit(ui: &mut Ui, s: f32, color: RgbaF32) {
    let t = s * 0.07; // bar thickness
    let len = s * 0.18; // bar length
    let o = s * 0.26; // inset from the button edge
    let far = s - o;
    // An L in each corner: horizontal bar + vertical bar.
    let bars = [
        (o, o, len, t),
        (o, o, t, len),
        (far - len, o, len, t),
        (far - t, o, t, len),
        (o, far - t, len, t),
        (o, far - len, t, len),
        (far - len, far - t, len, t),
        (far - t, far - len, t, len),
    ];
    for (x, y, w, h) in bars {
        ui.add_shape(
            Shape::rect(Rect::new(x, y, w, h))
                .corners(t * 0.5)
                .fill(color),
        );
    }
}

/// "1:1" label — zoom to 100%.
pub(super) fn draw_100(ui: &mut Ui, s: f32, color: RgbaF32) {
    let style = support::colored_text(ui, color, s * LABEL_GLYPH_FILL);
    Text::new("1:1").style(&style).align(Align::CENTER).show(ui);
}

/// 2×2 grid of hard squares — nearest (pixelated) sampling.
pub(super) fn draw_pixels(ui: &mut Ui, s: f32, color: RgbaF32) {
    let cell = s * 0.18;
    let gap = s * 0.08;
    let o = (s - (2.0 * cell + gap)) * 0.5;
    for iy in 0..2 {
        for ix in 0..2 {
            let x = o + ix as f32 * (cell + gap);
            let y = o + iy as f32 * (cell + gap);
            ui.add_shape(
                Shape::rect(Rect::new(x, y, cell, cell))
                    .corners(1.0)
                    .fill(color),
            );
        }
    }
}

/// A backdrop mode's swatch, ringed with the selection accent while `state`
/// is on.
#[derive(Debug, Clone, Copy)]
pub(super) struct Swatch {
    pub(super) mode: ViewerBg,
    pub(super) state: Toggle,
}

impl Swatch {
    /// An inset square in the `s`-sized box filled with the mode itself (mini
    /// checker for `Checker`).
    pub(super) fn draw(self, ui: &mut Ui, s: f32, theme: &Theme) {
        let Self { mode, state } = self;
        let d = s * SWATCH_SHARE;
        let o = (s - d) * 0.5;
        let rect = Rect::new(o, o, d, d);
        if let Some(fill) = flat_fill(theme, mode) {
            ui.add_shape(Shape::rect(rect).corners(SWATCH_RADIUS).fill(fill));
        } else {
            let light = RgbaF32::from_srgba(SrgbaU8::rgb(
                CHECKER_LIGHT_U8,
                CHECKER_LIGHT_U8,
                CHECKER_LIGHT_U8,
            ));
            let dark = RgbaF32::from_srgba(SrgbaU8::rgb(
                CHECKER_DARK_U8,
                CHECKER_DARK_U8,
                CHECKER_DARK_U8,
            ));
            ui.add_shape(Shape::rect(rect).corners(SWATCH_RADIUS).fill(dark));
            // Two light quads on the diagonal make the 2×2 mini checker.
            let h = d * 0.5;
            for cell in [Rect::new(o, o, h, h), Rect::new(o + h, o + h, h, h)] {
                ui.add_shape(Shape::rect(cell).fill(light));
            }
        }
        // Ring on top so the checker quads can't cover it.
        let (ring, width) = match state {
            Toggle::On => (theme.colors.selection_rect, theme.card.border_width_total()),
            Toggle::Off => (
                theme.colors.text_muted.with_alpha(SWATCH_RING_ALPHA),
                theme.card.border_width,
            ),
        };
        ui.add_shape(
            Shape::rect(rect)
                .corners(SWATCH_RADIUS)
                .border(Stroke::new(ring, width)),
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

/// How much of its button a backdrop swatch fills, leaving room for the ring.
const SWATCH_SHARE: f32 = 0.54;
/// The swatch's corner radius: a softened square, so it reads as a color
/// chip rather than a pixel.
const SWATCH_RADIUS: f32 = 2.0;
/// The resting ring's alpha: visible on every backdrop, louder on none.
const SWATCH_RING_ALPHA: f32 = 0.4;

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

    #[test]
    fn checker_image_is_one_2x2_period() {
        const L: u8 = CHECKER_LIGHT_U8;
        const D: u8 = CHECKER_DARK_U8;
        let img = checker_image();
        // Row-major light/dark, dark/light — one full checker period.
        #[rustfmt::skip]
        let expected = [
            L, L, L, 255,  D, D, D, 255,
            D, D, D, 255,  L, L, L, 255,
        ];
        assert_eq!(
            img,
            palantir::Image::from_srgba8(UVec2::splat(2), expected.to_vec())
                .expect("2×2 RGBA8 is 16 bytes")
        );
    }
}
