//! The frosted glass every floating surface over content wears: the popups and the toolbar pills.

use palantir::{BackdropFilter, Chrome, RgbaF32, SrgbaU8};

use crate::gui::theme::color;

/// The tint's alpha over the glass: the content under a surface reads through as a blurred wash
/// under the surface's own color.
const TINT_ALPHA: f32 = 0.72;

/// The glass's blur σ in logical px: wide enough that a node's title under a surface reads as
/// color, not as text competing with the surface's own.
const BLUR: f32 = 20.0;

/// The contrast WCAG 1.4.3 asks of text against its ground
/// (<https://www.w3.org/WAI/WCAG22/Understanding/contrast-minimum>).
const TEXT_CONTRAST: f32 = 4.5;

/// The linear level where the sRGB curve's linear toe ends and its power segment starts.
const SRGB_TOE: f32 = 0.003_130_8;

/// `chrome` over frosted glass: its own color as the tint, made translucent, as Fluent puts
/// acrylic under flyouts, context menus and in-app surfaces over content
/// (<https://learn.microsoft.com/windows/apps/develop/ui/in-app-acrylic>) and macOS its menus
/// on a blurred material.
///
/// The glass shows at most the luminance that keeps `ink`, the dimmest ink the surface paints, at
/// [`TEXT_CONTRAST`] over any backdrop, as an 8-bit target stores it. The brightest ground `ink`
/// holds it on is `g = (Y(ink) + 0.05) / 4.5 − 0.05`; [`before_storing`] lowers it to `g'`, what
/// the composite may reach before the target stores it higher. The ground over a backdrop at the
/// ceiling `c` is `(1 − α)·c + α·Y(tint)`, both in the linear light palantir composites in; so
/// `c = (g' − α·Y(tint)) / (1 − α)`.
pub(super) fn frost(chrome: &mut Chrome, ink: RgbaF32) {
    let tint = chrome
        .fill
        .as_solid()
        .expect("a frosted surface has a solid fill");
    debug_assert!(
        TINT_ALPHA * tint.r.min(tint.g).min(tint.b) > SRGB_TOE,
        "every channel of a ground clears the sRGB curve's linear toe, as `before_storing` needs"
    );
    let ground = before_storing((color::luminance(ink) + 0.05) / TEXT_CONTRAST - 0.05);
    let tint_luminance = color::luminance(tint);
    debug_assert!(
        tint_luminance <= ground,
        "the ink holds its contrast on the opaque surface"
    );
    let ceiling = (ground - TINT_ALPHA * tint_luminance) / (1.0 - TINT_ALPHA);
    chrome.fill = tint.with_alpha(TINT_ALPHA).into();
    chrome.backdrop_filter = BackdropFilter::frosted(BLUR).with_max_luminance(ceiling);
}

/// The brightest linear level a composite may reach and still store at most `g`: an 8-bit target
/// stores a channel within one encoded step of it, half a step of palantir's dither and half of
/// the rounding to a byte. So the level is the one a step under `B`, the highest byte that
/// decodes to at most `g`. A colored pixel stores no brighter than a neutral one of its
/// luminance: on the curve's power segment, where every channel of a ground lies as it keeps
/// `α` of the tint's, the stored level `decode(encode(c) + step)` is concave in `c`, so by
/// Jensen the weighted channels gain at most what their mean does.
fn before_storing(g: f32) -> f32 {
    let level = |byte: u8| RgbaF32::from_srgba(SrgbaU8::rgb(byte, byte, byte)).r;
    let mut top = RgbaF32::new(g, g, g, 1.0).to_srgba().r;
    if level(top) > g {
        top -= 1;
    }
    level(
        top.checked_sub(1)
            .expect("the ink holds its contrast a step above black"),
    )
}
