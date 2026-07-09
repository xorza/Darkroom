pub(crate) mod canvas;
pub(crate) mod dialogs;
pub(crate) mod graph_toolbar;
pub(crate) mod main_window;
pub(crate) mod menu_bar;
pub(crate) mod node;
pub(crate) mod preferences_view;
pub(crate) mod tab_bar;
pub(crate) mod widgets;

pub(crate) mod app;
pub(crate) mod node_values;
pub(crate) mod run_state;
pub(crate) mod scene;
pub(crate) mod status_bar;
pub(crate) mod theme;

use crate::core::document::GraphRef;
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
/// `Intent::SwitchTab`/`CloseTab`. `OpenGraph` adds the tab to the strip
/// directly (that part isn't undoable) but focuses it through the same
/// recorded `SwitchTab`, so undo faithfully reverses focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UiAction {
    /// Open `target` in a tab (or focus its existing tab).
    OpenGraph(GraphRef),
    /// Make the tab at this index active.
    ActivateTab(usize),
    /// Close the tab at this index (the `Main` tab is never closable).
    CloseTab(usize),
    /// Create a fresh empty subgraph and open it in a new tab (the "+"
    /// chip at the end of the strip).
    NewSubgraph,
}

/// Whether a port consumes a binding (`Input`) or produces a value
/// (`Output`). Scoped to the data-port subset until Trigger/Event are
/// reintroduced. `Input` ports live in the left column, `Output` in
/// the right; `opposite` flips between them for snap-target tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum PortKind {
    Input,
    Output,
}

impl PortKind {
    pub(crate) fn opposite(self) -> Self {
        match self {
            PortKind::Input => PortKind::Output,
            PortKind::Output => PortKind::Input,
        }
    }
}

/// One port's identity in the graph. Domain-keyed so prepass / record
/// can derive its `WidgetId` via [`crate::gui::node::port_row::port_circle_wid`]
/// without threading any cache.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct PortRef {
    pub(crate) node_id: scenarium::graph::NodeId,
    pub(crate) kind: PortKind,
    pub(crate) port_idx: usize,
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
