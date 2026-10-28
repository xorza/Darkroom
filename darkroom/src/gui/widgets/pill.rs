//! Shared layout for floating view toolbars: the frosted group pill the
//! [`Chip`](crate::gui::widgets::chip::Chip) buttons ride on. Used by the
//! graph canvas toolbar and the image viewer's control panel; each caller
//! keeps its own glyphs and toggle color policy.

use palantir::{Configure, Panel, Sense, Separator, Sizing, Spacing, Ui};

use crate::gui::theme::Theme;

/// One frosted group pill: `panel` (a caller-configured `Panel::hstack`
/// / `vstack` carrying its id and any alignment) dressed in the shared
/// pill layout — hugging, chip gap, pill padding, frosted backdrop — for the
/// caller to show. The pill senses pointer gestures itself, so a drag or
/// scroll starting between chips stays on the pill instead of falling
/// through to the canvas or image beneath.
pub(crate) fn dress(panel: Panel, theme: &Theme) -> Panel {
    panel
        .size((Sizing::HUG, Sizing::HUG))
        .gap(theme.space.xs)
        .padding(Spacing::all(theme.space.xs))
        .sense(Sense::CLICK | Sense::DRAG | Sense::SCROLL)
        .chrome(theme.pill.clone())
}

/// A row pill's height: a toolbar chip and the pill's padding above and below it, which a
/// readout pill beside it takes so the two line up.
pub(crate) fn height(theme: &Theme) -> f32 {
    theme.icon.toolbar.side() + 2.0 * theme.space.xs
}

/// Thin horizontal rule between concept groups sharing one column
/// (vstack) pill, inset from the pill's chrome on both ends. Grow an axis
/// parameter when a row pill first needs one.
pub(crate) fn rule(ui: &mut Ui, theme: &Theme) {
    Separator::horizontal()
        .margin(Spacing::new(theme.space.xs, 0.0, theme.space.xs, 0.0))
        .show(ui);
}
