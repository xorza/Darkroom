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

/// The token of darkroom's one OS window, given to `WinitHost::builder` and handed back to
/// `App::record`.
pub(crate) const MAIN_WINDOW: WindowToken = WindowToken(0);
