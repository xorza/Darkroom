//! The viewer's drawn vocabulary: the four control-button glyphs and the
//! checkerboard tile behind a transparent image.
//!
//! Every one is a pure function of the button side `s` and an ink colour,
//! built from `s * k` factors so a glyph fills its box at any button size.
//! That holds for the one glyph that is *text* rather than primitives
//! ([`draw_100`]) too — it takes a share of `s` like its siblings rather than
//! sitting on a [`TypeScale`] tier, because it is sized to a box and not to
//! the reading hierarchy.
//!
//! [`TypeScale`]: crate::gui::theme::TypeScale

use palantir::{Align, Color, Configure, Rect, Text, Ui};

use crate::core::io::preferences::ViewerBackground;
use crate::gui::theme::Theme;
use crate::gui::widgets::support::{colored_text, filled_rect, stroked_rect};

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
/// a neutral transparency reference, not chrome.
const CHECKER_LIGHT_U8: u8 = 77; // #4d4d4d
const CHECKER_DARK_U8: u8 = 51; // #333333

/// The 2×2 checkerboard tile — one full checker period, stamped across
/// the pane via `ImageFit::Tile` + `ImageFilter::Nearest`.
pub(super) fn checker_image() -> palantir::Image {
    const L: u8 = CHECKER_LIGHT_U8;
    const D: u8 = CHECKER_DARK_U8;
    let px = [
        [L, L, L, 255],
        [D, D, D, 255],
        [D, D, D, 255],
        [L, L, L, 255],
    ];
    palantir::Image::from_rgba8(2, 2, px.into_iter().flatten().collect())
}

/// Four inward corner brackets — "fit the image to the view".
pub(super) fn draw_fit(ui: &mut Ui, s: f32, color: Color) {
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
        filled_rect(ui, Rect::new(x, y, w, h), t * 0.5, color);
    }
}

/// "1:1" label — zoom to 100%.
pub(super) fn draw_100(ui: &mut Ui, s: f32, color: Color) {
    let style = colored_text(ui, color, s * LABEL_GLYPH_FILL);
    Text::new("1:1").style(&style).align(Align::CENTER).show(ui);
}

/// 2×2 grid of hard squares — nearest (pixelated) sampling.
pub(super) fn draw_pixels(ui: &mut Ui, s: f32, color: Color) {
    let cell = s * 0.18;
    let gap = s * 0.08;
    let o = (s - (2.0 * cell + gap)) * 0.5;
    for iy in 0..2 {
        for ix in 0..2 {
            let x = o + ix as f32 * (cell + gap);
            let y = o + iy as f32 * (cell + gap);
            filled_rect(ui, Rect::new(x, y, cell, cell), 1.0, color);
        }
    }
}

/// A backdrop-mode swatch: an inset square filled with the mode itself
/// (mini checker for `Checker`), ringed with the selection accent when
/// active.
pub(super) fn draw_swatch(
    ui: &mut Ui,
    s: f32,
    theme: &Theme,
    mode: ViewerBackground,
    selected: bool,
) {
    let d = s * 0.54;
    let o = (s - d) * 0.5;
    let rect = Rect::new(o, o, d, d);
    // One arm per mode, so the match stays exhaustive over the enum itself
    // rather than over a flat-fill subset plus a wildcard that has to
    // re-reject the one mode it already handled.
    match mode {
        ViewerBackground::Theme => filled_rect(ui, rect, 2.0, theme.canvas.bg),
        ViewerBackground::Black => filled_rect(ui, rect, 2.0, Color::BLACK),
        ViewerBackground::White => filled_rect(ui, rect, 2.0, Color::WHITE),
        ViewerBackground::Checker => {
            let light = Color::rgb_u8(CHECKER_LIGHT_U8, CHECKER_LIGHT_U8, CHECKER_LIGHT_U8);
            let dark = Color::rgb_u8(CHECKER_DARK_U8, CHECKER_DARK_U8, CHECKER_DARK_U8);
            filled_rect(ui, rect, 2.0, dark);
            // Two light quads on the diagonal make the 2×2 mini checker.
            let h = d * 0.5;
            for cell in [Rect::new(o, o, h, h), Rect::new(o + h, o + h, h, h)] {
                filled_rect(ui, cell, 0.0, light);
            }
        }
    }
    // Ring on top so the checker quads can't cover it.
    let (ring, width) = if selected {
        (theme.colors.selection_rect, 2.0)
    } else {
        (theme.colors.text_muted.with_alpha(0.4), 1.0)
    };
    stroked_rect(ui, rect, 2.0, ring, width);
}

#[cfg(test)]
mod tests {
    use super::*;
    use palantir::Image as AptImage;

    #[test]
    fn checker_image_is_one_2x2_period() {
        let img = checker_image();
        const L: u8 = CHECKER_LIGHT_U8;
        const D: u8 = CHECKER_DARK_U8;
        // Row-major light/dark, dark/light — one full checker period.
        #[rustfmt::skip]
        let expected = [
            L, L, L, 255,  D, D, D, 255,
            D, D, D, 255,  L, L, L, 255,
        ];
        assert_eq!(img, AptImage::from_rgba8(2, 2, expected.to_vec()));
    }
}
