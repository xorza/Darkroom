//! Shared chrome for floating view toolbars: the frosted group pill and
//! the square glyph buttons riding on it. Used by the graph canvas
//! toolbar and the image viewer's control panel; each caller keeps its
//! own glyphs and toggle color policy.

use aperture::{
    Background, Color, Configure, Corners, Panel, Sense, Separator, Sizing, Spacing, Ui, WidgetId,
};

use crate::gui::theme::Theme;
use crate::gui::widgets::support::tooltip_after;

/// Side of each square button, in px.
const BUTTON_SIZE: f32 = 30.0;
/// Inset of a toolbar from its view's corner.
pub(crate) const TOOLBAR_MARGIN: f32 = 8.0;
/// Gap between buttons.
pub(crate) const BUTTON_GAP: f32 = 6.0;
/// Corner radius of a button's rounded-rect background.
const BUTTON_RADIUS: f32 = 6.0;
/// Opacity of a group pill's frosted chrome backdrop. Keeps the toolbar
/// readable over an empty canvas *and* over content it happens to sit on —
/// the backdrop color sits between the canvas and node fills, so a bit of
/// translucency still contrasts against both while the content stays
/// faintly visible through it.
const PILL_BG_ALPHA: f32 = 0.7;
/// Padding between a group pill's chrome edge and the buttons inside it.
const PILL_PADDING: f32 = 4.0;
/// Corner radius of a group pill's chrome backdrop — the button radius
/// grown by the padding so the pill's rounding stays concentric with the
/// buttons'.
const PILL_RADIUS: f32 = BUTTON_RADIUS + PILL_PADDING;

/// The frosted chrome backdrop shared by toolbar group pills.
pub(crate) fn pill_background(theme: &Theme) -> Background {
    Background::rounded(
        theme.colors.chrome_fill.with_alpha(PILL_BG_ALPHA),
        Corners::all(PILL_RADIUS),
    )
}

/// One frosted group pill: `panel` (a caller-configured `Panel::hstack`
/// / `vstack` carrying its id and any alignment) dressed in the shared
/// pill chrome — hugging, chip gap, pill padding, frosted backdrop. The
/// pill senses pointer gestures itself, so a drag or scroll starting
/// between chips stays on the pill instead of falling through to the
/// canvas or image beneath.
pub(crate) fn pill(ui: &mut Ui, theme: &Theme, panel: Panel, body: impl FnOnce(&mut Ui)) {
    panel
        .size((Sizing::Hug, Sizing::Hug))
        .gap(BUTTON_GAP)
        .padding(Spacing::all(PILL_PADDING))
        .sense(Sense::CLICK | Sense::DRAG | Sense::SCROLL)
        .background(pill_background(theme))
        .show(ui, body);
}

/// Thin horizontal rule between concept groups sharing one column
/// (vstack) pill, inset from the pill chrome on both ends. Grow an axis
/// parameter when a row pill first needs one.
pub(crate) fn pill_rule(ui: &mut Ui, theme: &Theme) {
    const INSET: f32 = 5.0;
    Separator::horizontal()
        .color(theme.colors.border_soft())
        .margin(Spacing::new(INSET, 0.0, INSET, 0.0))
        .show(ui);
}

/// One square chip button riding a group pill: an opaque rounded chip
/// whose icon is painted by a caller closure, with a hover tooltip.
/// Momentary by default — neutral fill lifting on hover, muted glyph;
/// [`toggled`](Self::toggled) turns it into a toggle whose active state
/// inverts the chip (accent fill under a dark glyph). Builder chain
/// ending in [`show`](Self::show), like an aperture widget.
#[derive(Debug)]
pub(crate) struct Chip {
    wid: WidgetId,
    tip: &'static str,
    toggled: bool,
    idle_glyph: Option<Color>,
    toggled_fill: Option<Color>,
}

impl Chip {
    pub(crate) fn new(wid: WidgetId, tip: &'static str) -> Self {
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
    pub(crate) fn toggled(mut self, on: bool) -> Self {
        self.toggled = on;
        self
    }

    /// Glyph ink while idle (untoggled). Default: `text_muted`.
    pub(crate) fn idle_glyph(mut self, color: Color) -> Self {
        self.idle_glyph = Some(color);
        self
    }

    /// Chip fill while toggled. Default: the selection accent.
    pub(crate) fn toggled_fill(mut self, color: Color) -> Self {
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
        draw_glyph: impl FnOnce(&mut Ui, f32, Color),
    ) -> bool {
        let hovered = ui.response_for(self.wid).hovered;
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
            theme.colors.header_fill
        } else {
            theme.colors.node_fill
        };
        let s = BUTTON_SIZE;
        let button = Panel::zstack()
            .id(self.wid)
            .size((Sizing::Fixed(s), Sizing::Fixed(s)))
            .sense(Sense::CLICK)
            .background(Background::rounded(fill, Corners::all(BUTTON_RADIUS)))
            .show(ui, |ui| draw_glyph(ui, s, glyph));
        // Take the owned snapshot + click result so the button's `ui`
        // borrow ends before the tooltip records into `ui`.
        let snapshot = button.response.snapshot();
        let clicked = button.response.left.clicked();
        tooltip_after(ui, &snapshot, self.tip);
        clicked
    }
}
