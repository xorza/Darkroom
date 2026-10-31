//! The viewer's overlay: the frosted readout pill, the backdrop
//! roster the control panel loops over, and the one toggle with a state of its
//! own.
//!
//! The panel's *layout* stays on [`ImageViewer`](crate::gui::pane::image_viewer::ImageViewer) — it
//! reads and writes the viewer's framing — but everything it stamps out is
//! here, so adding a control means adding a row to [`BACKDROPS`] or a method
//! beside [`Controls::filter_toggle`] rather than growing the record pass.

use palantir::{
    Align, Configure, ImageFilter, Panel, RgbaF32, Sizing, Spacing, Text, TextInput, TextWrap, Ui,
    VAlign, WidgetId,
};
use scenarium::NodeId;

use crate::core::io::preferences::ViewerBg;
use crate::gui::theme::Theme;
use crate::gui::widgets::chip::Chip;
use crate::gui::widgets::icon::Icon;
use crate::gui::widgets::pill;
use crate::gui::widgets::support;

/// The backdrop radio roster — mode, widget-id key, tooltip — the one
/// table behind the controls loop and the swatch ids.
pub(super) const BACKDROPS: [(ViewerBg, &str, &str); 4] = [
    (ViewerBg::Theme, "bg_theme", "Theme background"),
    (ViewerBg::Black, "bg_black", "Black background"),
    (ViewerBg::White, "bg_white", "White background"),
    (ViewerBg::Checker, "bg_checker", "Checkerboard background"),
];

/// One viewer's control panel as its widgets are stamped out: the node keying
/// their ids, and the theme they paint with.
#[derive(Debug, Clone, Copy)]
pub(super) struct Controls<'a> {
    pub(super) node_id: NodeId,
    pub(super) theme: &'a Theme,
}

impl Controls<'_> {
    /// Stable id for one control-panel widget, keyed by node + role.
    pub(super) fn wid(self, key: &'static str) -> WidgetId {
        controls_wid(self.node_id, key)
    }

    /// The nearest/bilinear magnification toggle: accent-filled while nearest
    /// is active. Flips `filter` on click; returns whether it changed.
    pub(super) fn filter_toggle(self, ui: &mut Ui, filter: &mut ImageFilter) -> bool {
        let nearest = *filter == ImageFilter::Nearest;
        let tooltip = if nearest {
            "Zoom-in sampling: nearest — click for bilinear"
        } else {
            "Zoom-in sampling: bilinear — click for nearest"
        };
        if Chip::new(self.wid("filter"), tooltip)
            .selected(nearest)
            .show_icon(ui, self.theme, Icon::Pixels)
        {
            *filter = if nearest {
                ImageFilter::Linear
            } else {
                ImageFilter::Nearest
            };
            return true;
        }
        false
    }
}

/// [`Controls::wid`] for `node_id`'s panel, read without building one.
pub(super) fn controls_wid(node_id: NodeId, key: &'static str) -> WidgetId {
    WidgetId::from_hash(("darkroom.image_viewer.controls", node_id, key))
}

/// A frosted readout pill — the text sibling of the toolbar pills, at their height —
/// dressing `panel` (caller-configured id + placement) in the shared
/// pill chrome around one line of `text` in `ink`, cut with an ellipsis past `max_width`. Used
/// by the header readout and the empty-pane hint.
#[derive(Debug)]
pub(super) struct Readout<'a> {
    pub(super) panel: Panel,
    pub(super) text: TextInput<'a>,
    pub(super) ink: RgbaF32,
    pub(super) max_width: f32,
}

impl Readout<'_> {
    pub(super) fn show(self, ui: &mut Ui, theme: &Theme) {
        let Self {
            panel,
            text,
            ink,
            max_width,
        } = self;
        panel
            .size((Sizing::HUG, Sizing::fixed(pill::height(theme))))
            .max_size((max_width, f32::INFINITY))
            .padding(Spacing::xy(theme.space.md, 0.0))
            .child_align(Align::v(VAlign::Center))
            .chrome(theme.pill.clone())
            .show(ui, |ui| {
                let style = support::colored_text(ui, ink, theme.font.body);
                Text::new(text)
                    .style(&style)
                    .text_wrap(TextWrap::Ellipsis)
                    .show(ui);
            });
    }
}
