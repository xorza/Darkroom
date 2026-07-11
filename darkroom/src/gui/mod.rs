pub(crate) mod canvas;
pub(crate) mod dialogs;
pub(crate) mod dock_drag;
pub(crate) mod graph_toolbar;
pub(crate) mod image_viewer;
pub(crate) mod main_window;
pub(crate) mod menu_bar;
pub(crate) mod node;
pub(crate) mod preferences_view;
pub(crate) mod tab_bar;
pub(crate) mod widgets;

pub(crate) mod app;
pub(crate) mod format;
pub(crate) mod node_values;
pub(crate) mod run_state;
pub(crate) mod scene;
pub(crate) mod status_bar;
pub(crate) mod theme;

use crate::core::document::dock::{DockDrop, TabGroupId};
use crate::core::document::{GraphRef, PortRef, TabRef};
use crate::gui::app::App;
use aperture::WindowToken;

/// darkroom is single-window; this is the token its one OS window is
/// addressed by — passed to `WinitHost::new`, handed back to
/// `App::frame`, and used for `HostHandle::request_repaint`.
pub(crate) const MAIN_WINDOW: WindowToken = WindowToken(0);

/// Aperture's `HostHandle` is generic over the app type (only its
/// `run_on_main` uses it); darkroom has exactly one app, so alias it once
/// here and let widget signatures stay `HostHandle` instead of repeating
/// `<App>`.
pub(crate) type HostHandle = aperture::HostHandle<App>;

/// A navigation request surfaced from last frame's responses (tab/chip
/// clicks) and applied by `App` in the navigation phase. Decoupled from
/// `Intent` so the UI layer doesn't need to know which requests are
/// undoable: `App` translates `ActivateTab`/`CloseTab` into the undoable
/// `Intent::Dock` ops. `OpenGraph` adds the tab to a strip directly
/// (that part isn't undoable) but focuses it through the same recorded
/// activation, so undo faithfully reverses focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UiAction {
    /// Open `target` in a tab (or focus its existing tab).
    OpenGraph(GraphRef),
    /// Make this group's tab at `index` active (and focus the group).
    ActivateTab { group: TabGroupId, index: usize },
    /// Close this group's tab at `index` (the `Main` tab never closes).
    CloseTab { group: TabGroupId, index: usize },
    /// Create a fresh empty subgraph and open it in a new tab (the "+"
    /// chip at the end of the strip).
    NewSubgraph,
    /// Show this port's cached runtime image at full resolution in the
    /// image-viewer tab (an inspector preview thumbnail was clicked).
    OpenImageViewer(PortRef),
    /// Commit a finished tab drag: move `tab` to the drop the pointer
    /// released over.
    MoveTab { tab: TabRef, to: DockDrop },
}

/// One event (emitter) port's identity. Events are indexed independently
/// of data outputs, so they get their own ref rather than a `PortRef`
/// kind. Domain-keyed like [`PortRef`] so geometry/drag code derives the
/// glyph's `WidgetId` (`event_glyph_wid`) without a cache.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct EventRef {
    pub(crate) node_id: scenarium::graph::NodeId,
    pub(crate) event_idx: usize,
}
