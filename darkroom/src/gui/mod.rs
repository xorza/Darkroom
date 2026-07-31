pub(crate) mod app;
pub(crate) mod dialogs;
pub(crate) mod dock;
pub(crate) mod graph_ctx;
pub(crate) mod pane;
pub(crate) mod state;
pub(crate) mod theme;
pub(crate) mod widgets;
pub(crate) mod window;

use scenarium::NodeId;

use crate::core::document::dock::{DockOp, TabGroupId};
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

/// A navigation request surfaced from last frame's responses (tab/chip
/// clicks, a released tab drag) and applied by `App` in the navigation
/// phase. Decoupled from `GraphIntent` so the UI layer doesn't need to know
/// which requests are undoable: the editor wraps `Dock` ops into the
/// undoable `DockOp`. `OpenGraph` adds the tab to a strip directly
/// (that part isn't undoable) but focuses it through the same recorded
/// activation, so undo faithfully reverses focus.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum UiAction {
    /// Record a dock-layout mutation — a tab activation or close from a
    /// strip, or a finished drag's move/split.
    Dock(DockOp),
    /// Show this preview node's full runtime image in its viewer tab.
    OpenImageViewer(NodeId),
    /// Move dock focus onto this pane, because a press landed inside it.
    /// The one navigation request that is *not* undoable — see
    /// [`DockLayout::focus`](crate::core::document::dock::DockLayout::focus).
    FocusPane(TabGroupId),
}

/// One event (emitter) port's identity. Events are indexed independently
/// of data outputs, so they get their own ref rather than a `PortRef`
/// kind. Domain-keyed like [`PortRef`](crate::core::document::PortRef) so geometry/drag code derives the
/// glyph's `WidgetId` (`event_glyph_wid`) without a cache.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct EventRef {
    pub(crate) node_id: scenarium::NodeId,
    pub(crate) event_idx: usize,
}
