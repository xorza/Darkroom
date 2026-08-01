//! The viewer's floating chrome: the frosted readout pill, the backdrop
//! roster the control panel loops over, and the one toggle with a state of its
//! own.
//!
//! The panel's *layout* stays on [`ImageViewer`](super::ImageViewer) — it
//! reads and writes the viewer's framing — but everything it stamps out is
//! here, so adding a control means adding a row to [`BACKDROPS`] or a function
//! beside [`filter_toggle`] rather than growing the record pass.

use palantir::{Configure, ImageFilter, Panel, Sizing, Spacing, Text, TextInput, Ui, WidgetId};
use scenarium::NodeId;

use crate::core::io::preferences::ViewerBackground;
use crate::gui::pane::viewer::glyph;
use crate::gui::theme::Theme;
use crate::gui::widgets::support::muted_text;
use crate::gui::widgets::toolbar::{Chip, pill_background};

/// The backdrop radio roster — mode, widget-id key, tooltip — the one
/// table behind the controls loop and the swatch ids.
pub(super) const BACKDROPS: [(ViewerBackground, &str, &str); 4] = [
    (ViewerBackground::Theme, "bg_theme", "Theme background"),
    (ViewerBackground::Black, "bg_black", "Black background"),
    (ViewerBackground::White, "bg_white", "White background"),
    (
        ViewerBackground::Checker,
        "bg_checker",
        "Checkerboard background",
    ),
];

/// Stable id for one control-panel widget, keyed by node + role.
pub(super) fn control_wid(node_id: NodeId, key: &'static str) -> WidgetId {
    WidgetId::from_hash(("image_viewer.controls", node_id, key))
}

/// A frosted readout chip — the text sibling of the toolbar pills —
/// dressing `panel` (caller-configured id + placement) in the shared
/// chrome around one line of muted text. Used by the header readout and
/// the empty-pane hint.
pub(super) fn readout_pill<'a>(
    ui: &mut Ui,
    theme: &Theme,
    panel: Panel,
    text: impl Into<TextInput<'a>>,
) {
    let text = text.into();
    panel
        .size((Sizing::HUG, Sizing::HUG))
        .padding(Spacing::new(10.0, 6.0, 10.0, 6.0))
        .background(pill_background(theme))
        .show(ui, |ui| {
            let style = muted_text(ui, theme, theme.text.body);
            Text::new(text).style(&style).show(ui);
        });
}

/// The nearest/bilinear magnification toggle: accent-filled while nearest
/// is active. Flips `filter` on click; returns whether it changed.
pub(super) fn filter_toggle(
    ui: &mut Ui,
    theme: &Theme,
    node_id: NodeId,
    filter: &mut ImageFilter,
) -> bool {
    let nearest = *filter == ImageFilter::Nearest;
    let tip = if nearest {
        "Zoom-in sampling: nearest — click for bilinear"
    } else {
        "Zoom-in sampling: bilinear — click for nearest"
    };
    if Chip::new(control_wid(node_id, "filter"), tip)
        .toggled(nearest)
        .show(ui, theme, glyph::draw_pixels)
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
