//! [`StatusColors`]: the run-status inks a node header reports with.

use palantir::RgbaF32;

use crate::gui::theme::palette::Palette;

/// The app's semantic feedback palette: what an outcome *means*, not
/// which surface reports it.
///
/// Node execution is the largest consumer — `gui::pane::graph::node`
/// maps an `ExecStatus` onto these — but it is not the only one, which is
/// why these are not named for it. `error` is also the invalid-path
/// outline in preferences, the status-bar message and `LogLevel::Error`.
/// `warning` is `LogLevel::Warn` and an unconnected required port.
/// `success` is the toolbar's run glyph.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct StatusColors {
    /// It worked / it ran — green.
    pub(crate) success: RgbaF32,
    /// It was reused from cache — the accent cyan.
    pub(crate) info: RgbaF32,
    /// It is happening right now — teal.
    pub(crate) busy: RgbaF32,
    /// It is incomplete but not broken — orange.
    pub(crate) warning: RgbaF32,
    /// It failed — red.
    pub(crate) error: RgbaF32,
}

impl StatusColors {
    pub(super) fn from_palette(p: &Palette) -> Self {
        Self {
            success: p.status_success,
            info: p.status_info,
            busy: p.status_busy,
            warning: p.status_warning,
            error: p.status_error,
        }
    }
}
