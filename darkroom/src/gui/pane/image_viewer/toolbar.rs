//! The viewer's floating chrome: the frosted readout pill, the backdrop
//! roster the control panel loops over, and the one toggle with a state of its
//! own.
//!
//! The panel's *layout* stays on [`ImageViewer`](crate::gui::pane::image_viewer::ImageViewer) — it
//! reads and writes the viewer's framing — but everything it stamps out is
//! here, so adding a control means adding a row to [`BACKDROPS`] or a method
//! beside [`Controls::filter_toggle`] rather than growing the record pass.

use palantir::prelude::*;
use palantir::{ImageFilter, TextInput};
use scenarium::NodeId;

use crate::core::io::preferences::ViewerBg;
use crate::gui::pane::image_viewer::glyph;
use crate::gui::theme::Theme;
use crate::gui::widgets::chip::Chip;
use crate::gui::widgets::pill;
use crate::gui::widgets::support;
use crate::gui::widgets::toggle::Toggle;

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
        WidgetId::from_hash(("image_viewer.controls", self.node_id, key))
    }

    /// The nearest/bilinear magnification toggle: accent-filled while nearest
    /// is active. Flips `filter` on click; returns whether it changed.
    pub(super) fn filter_toggle(self, ui: &mut Ui, filter: &mut ImageFilter) -> bool {
        let nearest = *filter == ImageFilter::Nearest;
        let tip = if nearest {
            "Zoom-in sampling: nearest — click for bilinear"
        } else {
            "Zoom-in sampling: bilinear — click for nearest"
        };
        if Chip::new(self.wid("filter"), tip)
            .toggled(Toggle::from(nearest))
            .show(ui, self.theme, glyph::draw_pixels)
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

/// A frosted readout chip — the text sibling of the toolbar pills —
/// dressing `panel` (caller-configured id + placement) in the shared
/// chrome around one line of muted `text`. Used by the header readout and
/// the empty-pane hint.
#[derive(Debug)]
pub(super) struct Readout<'a> {
    pub(super) panel: Panel,
    pub(super) text: TextInput<'a>,
}

impl Readout<'_> {
    pub(super) fn show(self, ui: &mut Ui, theme: &Theme) {
        let Self { panel, text } = self;
        panel
            .size((Sizing::HUG, Sizing::HUG))
            .padding(Spacing::new(10.0, 6.0, 10.0, 6.0))
            .bg(pill::bg(theme))
            .show(ui, |ui| {
                let style = support::muted_text(ui, theme, theme.text.body);
                Text::new(text).style(&style).show(ui);
            });
    }
}
