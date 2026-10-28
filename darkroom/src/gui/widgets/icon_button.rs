//! [`IconButton`]: the small ghost button and the read-only mark a node's header carries.

use palantir::{Chrome, Configure, Corners, Panel, Rect, RgbaF32, Sense, Sizing, Ui, WidgetId};

use crate::gui::theme::Theme;
use crate::gui::widgets::icon::Icon;
use crate::gui::widgets::support;
use crate::gui::widgets::toggle::Toggle;

/// What the button is: a control the user presses, or a mark that only tells.
#[derive(Clone, Copy, Debug)]
enum Kind {
    Press {
        state: Toggle,
    },
    /// Senses hover alone, for its tooltip, so a click falls through to the node.
    Mark,
}

/// One icon in the control icon box with a tooltip. A control is a ghost button: no fill at rest, the
/// hover fill when the pointer is over it, and a tint of its color while it is on. Its ink is
/// the muted ink at rest and `color` while it is on or under the pointer; the hover fill is the
/// one a toolbar chip lifts to. Builder chain ending in [`show`](Self::show), like a palantir
/// widget.
#[derive(Debug)]
pub(crate) struct IconButton {
    icon: Icon,
    wid: WidgetId,
    color: RgbaF32,
    tooltip: &'static str,
    kind: Kind,
}

impl IconButton {
    /// A control, clickable through `wid`, showing `state`.
    pub(crate) const fn press(
        icon: Icon,
        wid: WidgetId,
        color: RgbaF32,
        tooltip: &'static str,
        state: Toggle,
    ) -> Self {
        Self {
            icon,
            wid,
            color,
            tooltip,
            kind: Kind::Press { state },
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
        }
    }

    /// Record the button; `true` when a control was clicked this frame.
    pub(crate) fn show(self, ui: &mut Ui, theme: &Theme) -> bool {
        let Self {
            icon,
            wid,
            color,
            tooltip,
            kind,
        } = self;
        // Last frame's hover: this frame's response is known only after `show`.
        let hovered = ui.response_for(wid).hovered();
        let (fill, ink, sense) = match kind {
            Kind::Mark => (None, color, Sense::HOVER),
            Kind::Press { state } => {
                let fill = press_fill(theme, color, state, hovered);
                let ink = if hovered || state == Toggle::On {
                    color
                } else {
                    theme.ink.muted
                };
                (fill, ink, Sense::CLICK)
            }
        };
        let mut chrome = Chrome {
            corners: Corners::all(theme.radius.control),
            ..Chrome::default()
        };
        // Left without a fill rather than a transparent one, so a resting button paints no quad.
        if let Some(fill) = fill {
            chrome.fill = fill.into();
        }
        let control = theme.icon.control;
        let (inset, side) = (control.inset, control.icon);
        let button = Panel::zstack()
            .id(wid)
            .size((Sizing::fixed(control.side()), Sizing::fixed(control.side())))
            .sense(sense)
            .chrome(chrome)
            .show(ui, |ui| {
                icon.draw(ui, Rect::new(inset, inset, side, side), ink);
            });
        // The owned snapshot ends the button's `ui` borrow before the tooltip records.
        let snapshot = button.response.snapshot();
        let clicked = matches!(kind, Kind::Press { .. }) && button.response.left.clicked();
        support::tooltip_after(ui, &snapshot, Some(tooltip));
        clicked
    }
}

/// A control's fill: none at rest, the hover lift under the pointer, and while it is on a wash of
/// its own `color` over the card it sits on, a step stronger under the pointer. A tint, not a
/// solid swatch, since a setting is passive configuration and solid fills belong to live status.
fn press_fill(theme: &Theme, color: RgbaF32, state: Toggle, hovered: bool) -> Option<RgbaF32> {
    match (state, hovered) {
        (Toggle::On, hovered) => Some(theme.on_fill(color, hovered)),
        (Toggle::Off, true) => Some(theme.card.lift),
        (Toggle::Off, false) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn luminance(c: RgbaF32) -> f32 {
        0.2126 * c.r + 0.7152 * c.g + 0.0722 * c.b
    }

    /// An "on" button rests under the hover lift an "off" one takes, so "on" never reads as
    /// "hovered", and lifts past it under the pointer, so hovering it still shows. For the two
    /// inks a node's toggles take: the cache's and the disable's.
    #[test]
    fn an_on_button_rests_under_the_hover_lift_and_rises_past_it() {
        let theme = Theme::default();
        let lift =
            press_fill(&theme, theme.ink.muted, Toggle::Off, true).expect("a hovered button fills");
        assert_eq!(
            press_fill(&theme, theme.ink.muted, Toggle::Off, false),
            None
        );
        for (ink, name) in [(theme.badge.cache, "cache"), (theme.ink.muted, "disable")] {
            let rest = press_fill(&theme, ink, Toggle::On, false).expect("on fills");
            let hover = press_fill(&theme, ink, Toggle::On, true).expect("on fills");
            assert!(
                luminance(rest) < luminance(lift),
                "{name}: on reads as hovered"
            );
            assert!(
                luminance(hover) > luminance(lift),
                "{name}: hovering on does not show"
            );
        }
    }
}
