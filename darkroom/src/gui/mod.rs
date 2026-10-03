pub(crate) mod app;
pub(crate) mod dialogs;
pub(crate) mod event_ref;
pub(crate) mod graph_ctx;
pub(crate) mod pane;
pub(crate) mod requests;
pub(crate) mod state;
pub(crate) mod theme;
pub(crate) mod widgets;
pub(crate) mod window;

use crate::gui::app::App;
use palantir::WindowToken;

/// darkroom is single-window; this is the token its one OS window is
/// addressed by — passed to `WinitHost::builder`, handed back to
/// `App::record`, and used for `HostHandle::request_repaint`.
pub(crate) const MAIN_WINDOW: WindowToken = WindowToken(0);

/// Palantir's `HostHandle` is generic over the app type (only its
/// `run_on_main` uses it); darkroom has exactly one app, so alias it once
/// here and let widget signatures stay `HostHandle` instead of repeating
/// `<App>`.
pub(crate) type HostHandle = palantir::HostHandle<App>;
