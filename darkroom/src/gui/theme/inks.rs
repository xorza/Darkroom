//! [`Inks`]: the text inks.

use palantir::RgbaF32;

use crate::gui::theme::palette::Palette;

/// The inks text is drawn in, loudest first.
#[derive(Clone, Debug)]
pub(crate) struct Inks {
    /// Primary ink: titles, values, body text.
    pub(crate) text: RgbaF32,
    /// De-emphasized ink: secondary text, a resting control's glyph, hints.
    pub(crate) muted: RgbaF32,
}

impl Inks {
    pub(super) const fn from_palette(p: &Palette) -> Self {
        Self {
            text: p.text,
            muted: p.text_muted,
        }
    }
}
