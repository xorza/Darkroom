pub(crate) mod canvas;
pub(crate) mod dialogs;
pub(crate) mod dock;
pub(crate) mod graph_toolbar;
pub(crate) mod image_viewer;
pub(crate) mod main_window;
pub(crate) mod menu_bar;
pub(crate) mod node;
pub(crate) mod preferences_view;
pub(crate) mod preview_store;
pub(crate) mod widgets;
use scenarium::NodeId;

pub(crate) mod app;
pub(crate) mod color;
pub(crate) mod format;
pub(crate) mod process_memory;
pub(crate) mod run_state;
pub(crate) mod scene;
pub(crate) mod status_bar;
pub(crate) mod theme;

use crate::core::document::GraphRef;
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
/// phase. Decoupled from `Intent` so the UI layer doesn't need to know
/// which requests are undoable: the editor wraps `Dock` ops into the
/// undoable `DocIntent::Dock`. `OpenGraph` adds the tab to a strip directly
/// (that part isn't undoable) but focuses it through the same recorded
/// activation, so undo faithfully reverses focus.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum UiAction {
    /// Open `target` in a tab (or focus its existing tab).
    OpenGraph(GraphRef),
    /// Record a dock-layout mutation — a tab activation or close from a
    /// strip, or a finished drag's move/split.
    Dock(DockOp),
    /// Create a fresh empty graph and open it in a new tab (the "+"
    /// chip at the end of the strip).
    NewGraph,
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
