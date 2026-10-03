//! [`Chip`]: the square glyph button a toolbar pill carries.

use palantir::prelude::*;

use crate::gui::theme::Theme;
use crate::gui::widgets::support::tooltip_after;

/// Side of each square button, in px.
const BUTTON_SIZE: f32 = 30.0;
/// Corner radius of a button's rounded-rect background — which the pill
/// around it grows by its padding, so the two roundings stay concentric.
pub(super) const BUTTON_RADIUS: f32 = 6.0;

/// One square chip button riding a group pill: an opaque rounded chip
/// whose icon is painted by a caller closure, with a hover tooltip.
/// Momentary by default — neutral fill lifting on hover, muted glyph;
/// [`toggled`](Self::toggled) turns it into a toggle whose active state
/// inverts the chip (accent fill under a dark glyph). Builder chain
/// ending in [`show`](Self::show), like an palantir widget.
#[derive(Debug)]
pub(crate) struct Chip {
    wid: WidgetId,
    tip: &'static str,
    toggled: bool,
    idle_glyph: Option<RgbaF32>,
    toggled_fill: Option<RgbaF32>,
}

impl Chip {
    pub(crate) const fn new(wid: WidgetId, tip: &'static str) -> Self {
        Self {
            wid,
            tip,
            toggled: false,
            idle_glyph: None,
            toggled_fill: None,
        }
    }

    /// Toggle state: while `true` the chip inverts — the toggled fill
    /// under a dark glyph. Default `false` (a momentary action chip).
    pub(crate) const fn toggled(mut self, on: bool) -> Self {
        self.toggled = on;
        self
    }

    /// Glyph ink while idle (untoggled). Default: `text_muted`.
    pub(crate) const fn idle_glyph(mut self, color: RgbaF32) -> Self {
        self.idle_glyph = Some(color);
        self
    }

    /// Chip fill while toggled. Default: the selection accent.
    pub(crate) const fn toggled_fill(mut self, color: RgbaF32) -> Self {
        self.toggled_fill = Some(color);
        self
    }

    /// Draw the chip: state-dependent fill, the icon painted centered in
    /// the `BUTTON_SIZE` box by `draw_glyph`, and the hover tooltip.
    /// Returns whether it was clicked this frame.
    pub(crate) fn show(
        self,
        ui: &mut Ui,
        theme: &Theme,
        draw_glyph: impl FnOnce(&mut Ui, f32, RgbaF32),
    ) -> bool {
        let hovered = ui.response_for(self.wid).hovered();
        // Glyph and fill vary on different axes: the glyph only inverts
        // for the toggled state, the fill also lifts on hover.
        let glyph = if self.toggled {
            theme.colors.chrome_fill
        } else {
            self.idle_glyph.unwrap_or(theme.colors.text_muted)
        };
        let fill = if self.toggled {
            self.toggled_fill.unwrap_or(theme.colors.selection_rect)
        } else if hovered {
            theme.card.header_fill
        } else {
            theme.card.fill
        };
        let s = BUTTON_SIZE;
        let button = Panel::zstack()
            .id(self.wid)
            .size((Sizing::fixed(s), Sizing::fixed(s)))
            .sense(Sense::CLICK)
            .background(Background::rounded(fill, Corners::all(BUTTON_RADIUS)))
            .show(ui, |ui| draw_glyph(ui, s, glyph));
        // Take the owned snapshot + click result so the button's `ui`
        // borrow ends before the tooltip records into `ui`.
        let snapshot = button.response.snapshot();
        let clicked = button.response.left.clicked();
        tooltip_after(ui, &snapshot, Some(self.tip));
        clicked
    }
}
