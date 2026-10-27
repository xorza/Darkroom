//! [`IconButton`]: the small ghost button and the read-only mark a node's header carries.

use palantir::prelude::*;

use crate::gui::theme::Theme;
use crate::gui::widgets::icon::Icon;
use crate::gui::widgets::support;
use crate::gui::widgets::toggle::Toggle;

/// Side of the button's box. The icon inside it is [`ICON_SIZE`], so the hover fill clears the
/// artwork by 3.5 px a side.
const BUTTON_SIZE: f32 = 22.0;
const ICON_SIZE: f32 = 15.0;

/// The tint behind an "on" button's icon, in its own color: a tint, not a solid swatch, since a
/// setting is passive configuration and solid fills belong to live status.
const ON_ALPHA: f32 = 0.16;
const ON_HOVER_ALPHA: f32 = 0.26;

/// What the button is: a control the user presses, or a mark that only tells.
#[derive(Clone, Copy, Debug)]
enum Kind {
    Press {
        state: Toggle,
    },
    /// Senses hover alone, for its tooltip, so a click falls through to the node.
    Mark,
}

/// One icon in a 22 px box with a tooltip. A control is a ghost button: no fill at rest, the
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
                let fill = match (state, hovered) {
                    (Toggle::On, true) => Some(color.with_alpha(ON_HOVER_ALPHA)),
                    (Toggle::On, false) => Some(color.with_alpha(ON_ALPHA)),
                    (Toggle::Off, true) => Some(theme.card.header_fill),
                    (Toggle::Off, false) => None,
                };
                let ink = if hovered || state == Toggle::On {
                    color
                } else {
                    theme.colors.text_muted
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
        let inset = (BUTTON_SIZE - ICON_SIZE) * 0.5;
        let button = Panel::zstack()
            .id(wid)
            .size((Sizing::fixed(BUTTON_SIZE), Sizing::fixed(BUTTON_SIZE)))
            .sense(sense)
            .chrome(chrome)
            .show(ui, |ui| {
                icon.draw(ui, Rect::new(inset, inset, ICON_SIZE, ICON_SIZE), ink);
            });
        // The owned snapshot ends the button's `ui` borrow before the tooltip records.
        let snapshot = button.response.snapshot();
        let clicked = matches!(kind, Kind::Press { .. }) && button.response.left.clicked();
        support::tooltip_after(ui, &snapshot, Some(tooltip));
        clicked
    }
}
