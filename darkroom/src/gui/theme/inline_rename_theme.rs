//! [`InlineRenameTheme`]: how an in-place rename field is styled.

use palantir::{Background, Spacing, TextEditTheme, TextStyle};

/// Per-widget theme bundle for the inline-rename label⇄field widget
/// (node title, boundary-port name, graph tab). The `text_edit`
/// look is stripped to the bare editor surface (no padding/margin, no
/// border, transparent fill) so the field's `Hug` height equals its
/// plain `Text` twin and the row doesn't reshape on a swap.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct InlineRenameTheme {
    pub(crate) text_edit: TextEditTheme,
}

impl InlineRenameTheme {
    /// Shared shape: start from the palette's text-edit recipe (which
    /// already carries the right caret / placeholder / selection), then
    /// strip every visual that would reshape the row (padding, margin,
    /// border, fill) so the field reads against whichever canvas hosts
    /// it.
    pub(super) fn from_palette(p: &palantir::Palette) -> Self {
        Self::flattened(&TextEditTheme::from_palette(p))
    }

    /// The flattening half of [`Self::from_palette`], over an existing
    /// text-edit bundle rather than a palette — how an
    /// [`InlineRename`](crate::gui::widgets::inline_rename::InlineRename)
    /// with no `.style(…)` derives its look from ambient
    /// [`palantir::Theme::text_edit`].
    ///
    /// Split out so the ambient path and this theme's own slot are
    /// stripped by one function: a visual added here can't be flattened
    /// in the configured bundle and left standing in the fallback.
    ///
    /// [`Background::NONE`] wholesale rather than clearing fill and stroke
    /// by hand: `text_edit` may be an app-configured bundle, and a shadow
    /// or radius it carries would otherwise outlive the flattening and
    /// paint around a field that is supposed to read as a plain label.
    pub(crate) fn flattened(text_edit: &TextEditTheme) -> Self {
        let mut style = TextEditTheme {
            padding: Spacing::ZERO,
            margin: Spacing::ZERO,
            ..text_edit.clone()
        };
        for look in [
            &mut style.looks.normal,
            &mut style.looks.hovered,
            &mut style.looks.active,
            &mut style.looks.disabled,
        ] {
            look.background = Background::NONE;
        }
        Self { text_edit: style }
    }

    /// The same bundle with `text` pinned on every state.
    ///
    /// All four looks, not just `normal`: the idle label reads `normal`
    /// while the open editor resolves per state, so leaving the others
    /// to inherit would change the font when the field is hovered or
    /// focused — mid-rename, on the widget whose whole contract is that
    /// the glyphs don't move when it opens.
    pub(crate) fn with_text(mut self, text: TextStyle) -> Self {
        for look in [
            &mut self.text_edit.looks.normal,
            &mut self.text_edit.looks.hovered,
            &mut self.text_edit.looks.active,
            &mut self.text_edit.looks.disabled,
        ] {
            look.text = Some(text.clone());
        }
        self
    }
}
