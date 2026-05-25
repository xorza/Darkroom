pub mod background;
pub mod breaker;
pub mod connection_ui;
pub mod graph_ui;
pub mod main_window;
pub mod menu_bar;
pub mod new_node_ui;
pub mod node_ui;
pub mod port_frame;
pub mod selection_ui;
pub mod tab_bar;
pub mod value_editor;

use crate::document::GraphRef;

/// A navigation request surfaced from last frame's responses (tab/chip
/// clicks) and applied by `App` in the navigation phase. Decoupled from
/// `Intent` so the UI layer doesn't need to know which requests are
/// undoable: `App` translates `ActivateTab`/`CloseTab` into the undoable
/// `Intent::SwitchTab`/`CloseTab`, while `OpenGraph` mutates the tab list
/// directly (opening a tab isn't undoable — only switching/closing is).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiAction {
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
/// (`Output`). Mirrors deprecated darkroom's `PortKind` — same domain
/// vocabulary, scoped to the data-port subset until Trigger/Event are
/// reintroduced. `Input` ports live in the left column, `Output` in
/// the right; `opposite` flips between them for snap-target tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PortKind {
    Input,
    Output,
}

impl PortKind {
    pub fn opposite(self) -> Self {
        match self {
            PortKind::Input => PortKind::Output,
            PortKind::Output => PortKind::Input,
        }
    }
}

/// One port's identity in the graph. Domain-keyed so prepass / record
/// can derive its `WidgetId` via [`crate::gui::node_ui::port_circle_wid`]
/// without threading any cache.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PortRef {
    pub node_id: scenarium::prelude::NodeId,
    pub kind: PortKind,
    pub port_idx: usize,
}
