//! [`Chip`]: the square glyph button a toolbar pill carries.

use palantir::{
    Chrome, Configure, Corners, Panel, Rect, RgbaF32, Sense, Shortcut, Sizing, TextInput, Ui,
    WidgetId, fmt,
};

use crate::gui::theme::Theme;
use crate::gui::widgets::icon::Icon;
use crate::gui::widgets::support;
use crate::gui::widgets::toggle::Toggle;

/// One square chip button riding a group pill: an opaque rounded chip
/// whose icon is painted by a caller closure, with a hover tooltip.
/// Momentary by default — neutral fill lifting on hover, a muted glyph that lights in the chip's
/// ink under the pointer; [`toggled`](Self::toggled) turns it into a toggle whose active state is
/// a wash of its ink under a glyph in that ink, as an [`IconButton`]'s is. Builder chain ending in
/// [`show`](Self::show), like a palantir widget.
///
/// [`IconButton`]: crate::gui::widgets::icon_button::IconButton
#[derive(Debug)]
pub(crate) struct Chip {
    wid: WidgetId,
    tooltip: &'static str,
    shortcut: Option<Shortcut>,
    toggled: Toggle,
    idle_glyph: Option<RgbaF32>,
    ink: Option<RgbaF32>,
}

impl Chip {
    pub(crate) const fn new(wid: WidgetId, tooltip: &'static str) -> Self {
        Self {
            wid,
            tooltip,
            shortcut: None,
            toggled: Toggle::Off,
            idle_glyph: None,
            ink: None,
        }
    }

    /// The chord that does what the chip does, shown after the tooltip: "Run · Ctrl+R".
    pub(crate) const fn shortcut(mut self, shortcut: Shortcut) -> Self {
        self.shortcut = Some(shortcut);
        self
    }

    /// Toggle state: while [`Toggle::On`] the chip takes a wash of its ink under a glyph in it.
    /// Default [`Toggle::Off`] (a momentary action chip).
    pub(crate) const fn toggled(mut self, state: Toggle) -> Self {
        self.toggled = state;
        self
    }

    /// Glyph ink while idle (untoggled), at rest and under the pointer. Default: `text_muted`,
    /// lighting in the chip's ink under the pointer.
    pub(crate) const fn idle_glyph(mut self, color: RgbaF32) -> Self {
        self.idle_glyph = Some(color);
        self
    }

    /// The chip's ink: its glyph while on or under the pointer, and the wash behind it while on.
    /// Default: the accent.
    pub(crate) const fn ink(mut self, color: RgbaF32) -> Self {
        self.ink = Some(color);
        self
    }

    /// [`Self::show`] with `icon` centered in the button.
    pub(crate) fn show_icon(self, ui: &mut Ui, theme: &Theme, icon: Icon) -> bool {
        let toolbar = theme.icon.toolbar;
        self.show(ui, theme, |ui, _, color| {
            let (inset, side) = (toolbar.inset, toolbar.icon);
            icon.draw(ui, Rect::new(inset, inset, side, side), color);
        })
    }

    /// Draw the chip: state-dependent fill, the icon painted centered in
    /// the toolbar icon box by `draw_glyph`, and the hover tooltip.
    /// Returns whether it was clicked this frame.
    pub(crate) fn show(
        self,
        ui: &mut Ui,
        theme: &Theme,
        draw_glyph: impl FnOnce(&mut Ui, f32, RgbaF32),
    ) -> bool {
        let hovered = ui.response_for(self.wid).hovered();
        let ink = self.ink.unwrap_or(theme.colors.accent);
        let (glyph, fill) = match self.toggled {
            Toggle::On => (ink, theme.on_fill(ink, hovered)),
            Toggle::Off => {
                let lit = if hovered { ink } else { theme.ink.muted };
                let fill = if hovered {
                    theme.card.lift
                } else {
                    theme.card.fill
                };
                (self.idle_glyph.unwrap_or(lit), fill)
            }
        };
        let s = theme.icon.toolbar.side();
        let button = Panel::zstack()
            .id(self.wid)
            .size((Sizing::fixed(s), Sizing::fixed(s)))
            .sense(Sense::CLICK)
            .chrome(Chrome::rounded(fill, Corners::all(theme.radius.card)))
            .show(ui, |ui| draw_glyph(ui, s, glyph));
        // Take the owned snapshot + click result so the button's `ui`
        // borrow ends before the tooltip records into `ui`.
        let snapshot = button.response.snapshot();
        let clicked = button.response.left.clicked();
        let tooltip: TextInput<'_> = match self.shortcut {
            Some(shortcut) => fmt!(ui, "{} \u{b7} {shortcut}", self.tooltip).into(),
            None => self.tooltip.into(),
        };
        support::tooltip_after(ui, &snapshot, Some(tooltip));
        clicked
    }
}
