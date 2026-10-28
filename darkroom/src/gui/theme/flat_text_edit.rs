//! [`FlatTextEdit`]: a text edit that reads as the plain text it edits.

use palantir::{Chrome, FontWeight, SlotDefaults, Spacing, TextEditTheme, TextStyleOverrides};

/// A text edit stripped to its bare surface — no padding, margin, border or fill — so the field's
/// `Hug` height equals the plain `Text` it replaces and a row does not reshape on the swap: a node
/// title's rename, the palette's search field.
#[derive(Clone, Debug)]
pub(crate) struct FlatTextEdit {
    /// In the ambient text style.
    pub(crate) plain: TextEditTheme,
    /// In [`FontWeight::BOLD`] on every state: a node's title.
    pub(crate) bold: TextEditTheme,
}

impl FlatTextEdit {
    /// The app's text edit `field`, which carries the caret, placeholder and selection, with
    /// every visual that would reshape the row stripped.
    pub(super) fn new(field: &TextEditTheme) -> Self {
        let plain = flattened(field);
        let bold = with_text(
            plain.clone(),
            TextStyleOverrides::NONE.with_weight(FontWeight::BOLD),
        );
        Self { plain, bold }
    }
}

/// `text_edit` with no padding, margin or chrome. [`Chrome::NONE`] wholesale rather than clearing
/// fill and stroke by hand: a shadow or radius the bundle carries would otherwise outlive the
/// flattening and paint around a field that is meant to read as a plain label.
fn flattened(text_edit: &TextEditTheme) -> TextEditTheme {
    let mut style = TextEditTheme {
        defaults: SlotDefaults {
            padding: Spacing::ZERO,
            margin: Spacing::ZERO,
            ..text_edit.defaults
        },
        ..text_edit.clone()
    };
    for look in style.looks.each_mut() {
        look.chrome = Chrome::NONE;
    }
    style
}

/// `style` with `text` over every state's own: all four looks, as the idle label reads `normal`
/// while the open editor resolves per state, so a state left to inherit would change the font
/// mid-rename, on the widget whose contract is that the glyphs do not move when it opens.
fn with_text(mut style: TextEditTheme, text: TextStyleOverrides) -> TextEditTheme {
    for look in style.looks.each_mut() {
        look.text = text;
    }
    style
}
