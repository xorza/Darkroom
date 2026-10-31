//! [`IconButton`]: the small ghost button and the read-only mark a node's header carries.

use palantir::{
    Brush, Button, ButtonTheme, Chrome, Configure, Corners, Rect, RgbaF32, Sense, SlotDefaults,
    Spacing, StatefulLook, TextStyleOverrides, Ui, WidgetId, WidgetLook,
};

use crate::gui::theme::Theme;
use crate::gui::widgets::icon::Icon;
use crate::gui::widgets::support;

/// What the button is: a control the user presses, or a mark that only tells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Press,
    /// Senses hover alone, for its tooltip, so a click falls through to the node.
    Mark,
}

/// One icon in the control icon box with a tooltip. A control is a ghost button: no fill at rest,
/// the hover fill when the pointer is over it, and a tint of its color while it is
/// [selected](Self::selected). Its ink is the muted ink at rest and `color` while it is on or
/// under the pointer; the hover fill is the one a toolbar chip lifts to. Builder chain ending in
/// [`show`](Self::show), like a palantir widget.
#[derive(Debug)]
pub(crate) struct IconButton {
    icon: Icon,
    wid: WidgetId,
    color: RgbaF32,
    tooltip: &'static str,
    kind: Kind,
    selected: bool,
}

impl IconButton {
    /// A control, clickable through `wid`.
    pub(crate) const fn press(
        icon: Icon,
        wid: WidgetId,
        color: RgbaF32,
        tooltip: &'static str,
    ) -> Self {
        Self {
            icon,
            wid,
            color,
            tooltip,
            kind: Kind::Press,
            selected: false,
        }
    }

    /// A read-only mark in `color`.
    pub(crate) const fn mark(
        icon: Icon,
        wid: WidgetId,
        color: RgbaF32,
        tooltip: &'static str,
    ) -> Self {
        Self {
            icon,
            wid,
            color,
            tooltip,
            kind: Kind::Mark,
            selected: false,
        }
    }

    /// Whether a control shows its on state. Default `false`.
    pub(crate) const fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// Record the button; `true` when a control was clicked this frame.
    pub(crate) fn show(self, ui: &mut Ui, theme: &Theme) -> bool {
        let Self {
            icon,
            wid,
            color,
            tooltip,
            kind,
            selected,
        } = self;
        let control = theme.icon.control;
        let (inset, side) = (control.inset, control.icon);
        let (style, sense) = match kind {
            Kind::Press => (press_style(theme, color), Sense::CLICK),
            Kind::Mark => (mark_style(theme, color), Sense::HOVER),
        };
        let button = Button::new()
            .id(wid)
            .size(control.side())
            .sense(sense)
            .focusable(false)
            .style(&style)
            .selected(selected)
            .show_with(ui, |ui, text| {
                icon.draw(ui, Rect::new(inset, inset, side, side), text.color);
            });
        let snapshot = button.response.snapshot();
        let clicked = kind == Kind::Press && button.response.clicked();
        support::tooltip_after(ui, &snapshot, Some(tooltip));
        clicked
    }
}

/// A look of `fill` under `ink`, control-rounded; a `None` fill paints no quad.
fn look(theme: &Theme, fill: Option<RgbaF32>, ink: RgbaF32) -> WidgetLook {
    WidgetLook {
        chrome: Chrome::rounded(
            fill.map_or(Brush::TRANSPARENT, Brush::from),
            Corners::all(theme.radius.control),
        ),
        text: TextStyleOverrides::NONE.with_color(ink),
    }
}

/// A control's looks: no fill and the muted ink at rest, the hover lift under `color` under the
/// pointer, and while it is on a wash of its own `color` over the card it sits on, a step
/// stronger under the pointer. A tint, not a solid swatch, since a setting is passive
/// configuration and solid fills belong to live status. A press looks as the pointer over it
/// does.
fn press_style(theme: &Theme, color: RgbaF32) -> ButtonTheme {
    let rest = look(theme, None, theme.ink.muted);
    let lit = look(theme, Some(theme.card.lift), color);
    let on = look(theme, Some(theme.on_fill(color, false)), color);
    let on_lit = look(theme, Some(theme.on_fill(color, true)), color);
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
        defaults: flush(),
    }
}

/// A mark's looks: `color` and no fill in every state.
fn mark_style(theme: &Theme, color: RgbaF32) -> ButtonTheme {
    let still = look(theme, None, color);
    let looks = StatefulLook {
        normal: still.clone(),
        hovered: still.clone(),
        active: still.clone(),
        disabled: still,
    };
    ButtonTheme {
        selected: looks.clone(),
        looks,
        defaults: flush(),
    }
}

/// No padding or margin, as the icon box carries its own inset, and no transition: darkroom's
/// controls snap.
const fn flush() -> SlotDefaults {
    SlotDefaults {
        padding: Spacing::ZERO,
        margin: Spacing::ZERO,
        animation: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::theme::color;

    /// The fill a look paints.
    fn fill(look: &WidgetLook) -> RgbaF32 {
        look.chrome.fill.as_solid().expect("a solid fill")
    }

    /// An "on" button rests under the hover lift an "off" one takes, so "on" never reads as
    /// "hovered", and lifts past it under the pointer, so hovering it still shows. For the two
    /// inks a node's toggles take: the cache's and the disable's.
    #[test]
    fn an_on_button_rests_under_the_hover_lift_and_rises_past_it() {
        let theme = Theme::default();
        let off = press_style(&theme, theme.ink.muted);
        let lift = fill(&off.looks.hovered);
        assert!(
            off.looks.normal.chrome.fill.is_noop(),
            "a resting button paints no quad"
        );
        for (ink, name) in [(theme.badge.cache, "cache"), (theme.ink.muted, "disable")] {
            let on = press_style(&theme, ink).selected;
            assert!(
                color::luminance(fill(&on.normal)) < color::luminance(lift),
                "{name}: on reads as hovered"
            );
            assert!(
                color::luminance(fill(&on.hovered)) > color::luminance(lift),
                "{name}: hovering on does not show"
            );
        }
    }
}
