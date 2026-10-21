pub(crate) mod app;
pub(crate) mod dialogs;
pub(crate) mod frame;
pub(crate) mod graph_ctx;
pub(crate) mod main_window;
pub(crate) mod node_key;
pub(crate) mod pane;
pub(crate) mod port_ref;
pub(crate) mod state;
pub(crate) mod theme;
pub(crate) mod widgets;

use palantir::WindowToken;

/// darkroom is single-window; this is the token its one OS window is
/// addressed by — passed to `WinitHost::builder`, handed back to
/// `App::record`, and used for `HostHandle::request_repaint`.
pub(crate) const MAIN_WINDOW: WindowToken = WindowToken(0);
