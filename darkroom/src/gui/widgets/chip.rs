//! [`Chip`]: the square glyph button a toolbar pill carries.

use palantir::{
    Button, ButtonTheme, Chrome, Configure, Corners, Rect, RgbaF32, Shortcut, SlotDefaults,
    Spacing, StatefulLook, TextInput, TextStyleOverrides, Ui, WidgetId, WidgetLook, fmt,
};

use crate::gui::theme::Theme;
use crate::gui::widgets::icon::Icon;
use crate::gui::widgets::support;

/// One square chip button riding a group pill: an opaque rounded chip whose icon is painted by a
/// caller closure in the ink the chip's state resolves, with a hover tooltip. Momentary by
/// default — neutral fill lifting on hover, a muted glyph that lights in the chip's ink under the
/// pointer; [`selected`](Self::selected) shows its on state, a wash of its ink under a glyph in
/// that ink, as an [`IconButton`]'s is. Builder chain ending in [`show`](Self::show), like a
/// palantir widget.
///
/// [`IconButton`]: crate::gui::widgets::icon_button::IconButton
#[derive(Debug)]
pub(crate) struct Chip {
    wid: WidgetId,
    tooltip: &'static str,
    shortcut: Option<Shortcut>,
    selected: bool,
    idle_glyph: Option<RgbaF32>,
    ink: Option<RgbaF32>,
}

impl Chip {
    pub(crate) const fn new(wid: WidgetId, tooltip: &'static str) -> Self {
        Self {
            wid,
            tooltip,
            shortcut: None,
            selected: false,
            idle_glyph: None,
            ink: None,
        }
    }

    /// The chord that does what the chip does, shown after the tooltip: "Run · Ctrl+R".
    pub(crate) const fn shortcut(mut self, shortcut: Shortcut) -> Self {
        self.shortcut = Some(shortcut);
        self
    }

    /// Whether the chip shows its on state: a wash of its ink under a glyph in it. Default
    /// `false`, a momentary action chip.
    pub(crate) const fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// Glyph ink while off, at rest and under the pointer. Default: the muted ink, lighting in
    /// the chip's ink under the pointer.
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

    /// Draw the chip: state-dependent fill, the icon painted in the toolbar icon box by
    /// `draw_glyph` in the state's ink, and the hover tooltip. Returns whether it was clicked
    /// this frame.
    pub(crate) fn show(
        self,
        ui: &mut Ui,
        theme: &Theme,
        draw_glyph: impl FnOnce(&mut Ui, f32, RgbaF32),
    ) -> bool {
        let s = theme.icon.toolbar.side();
        let style = self.style(theme);
        let button = Button::new()
            .id(self.wid)
            .size(s)
            .focusable(false)
            .style(&style)
            .selected(self.selected)
            .show_with(ui, |ui, text| draw_glyph(ui, s, text.color));
        let snapshot = button.response.snapshot();
        let clicked = button.response.clicked();
        let tooltip: TextInput<'_> = match self.shortcut {
            Some(shortcut) => fmt!(ui, "{} \u{b7} {shortcut}", self.tooltip).into(),
            None => self.tooltip.into(),
        };
        support::tooltip_after(ui, &snapshot, Some(tooltip));
        clicked
    }

    /// The chip's looks: off, the card's fill lifting under the pointer under the idle glyph;
    /// on, the wash of its ink under a glyph in it. A press looks as the pointer over it does.
    fn style(&self, theme: &Theme) -> ButtonTheme {
        let ink = self.ink.unwrap_or(theme.colors.accent);
        let look = |fill: RgbaF32, glyph: RgbaF32| WidgetLook {
            chrome: Chrome::rounded(fill, Corners::all(theme.radius.card)),
            text: TextStyleOverrides::NONE.with_color(glyph),
        };
        let rest = look(theme.card.fill, self.idle_glyph.unwrap_or(theme.ink.muted));
        let lit = look(theme.card.lift, self.idle_glyph.unwrap_or(ink));
        let on = look(theme.on_fill(ink, false), ink);
        let on_lit = look(theme.on_fill(ink, true), ink);
        ButtonTheme {
            looks: StatefulLook {
                normal: rest.clone(),
                hovered: lit.clone(),
                active: lit,
                disabled: rest,
            },
            selected: StatefulLook {
                normal: on.clone(),
                hovered: on_lit.clone(),
                active: on_lit,
                disabled: on,
            },
            defaults: SlotDefaults {
                padding: Spacing::ZERO,
                margin: Spacing::ZERO,
                animation: None,
            },
        }
    }
}
