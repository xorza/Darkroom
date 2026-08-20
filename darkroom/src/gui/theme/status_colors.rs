//! [`StatusColors`]: the run-status inks a node header reports with.

use palantir::Color;

use crate::core::theme_pref::ThemePreset;
use crate::gui::theme::palette_struct::palette_struct;
use crate::gui::theme::swatches::{dark, light};

palette_struct! {
    /// The app's semantic feedback palette: what an outcome *means*, not
    /// which surface reports it.
    ///
    /// Node execution is the largest consumer — `gui::pane::graph::node`
    /// maps an `ExecStatus` onto these — but it is not the only one, which is
    /// why these are not named for it: `error` is also the invalid-path
    /// outline in preferences, the status-bar message and `LogLevel::Error`;
    /// `warning` is `LogLevel::Warn` and an unconnected required port;
    /// `success` is the toolbar's run glyph.
    #[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
    pub(crate) struct StatusColors;
    /// It worked / it ran — palette `success` (green).
    success: Color => STATUS_SUCCESS,
    /// It was reused from cache — palette `accent` (cyan).
    info: Color => STATUS_INFO,
    /// It is happening right now — palette `constant` (purple).
    busy: Color => STATUS_BUSY,
    /// It is incomplete but not broken — palette `syn_keyword` (orange).
    warning: Color => STATUS_WARNING,
    /// It failed — palette `error` (red).
    error: Color => STATUS_ERROR,
}
