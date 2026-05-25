pub mod background;
pub mod breaker;
pub mod connection_ui;
pub mod graph_ui;
pub mod main_window;
pub mod menu_bar;
pub mod new_node_ui;
pub mod node_ui;
pub mod selection_ui;
pub mod tab_bar;
pub mod value_editor;

use crate::document::GraphRef;

/// A view-state request raised during the record pass and applied by
/// `App` after the frame. Distinct from `Intent` because none of these
/// mutate the document — they change *what the editor shows* (which tab
/// is active, which graphs are open), so they're not undoable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiAction {
    /// Open `target` in a tab (or focus its existing tab).
    OpenGraph(GraphRef),
    /// Make the tab at this index active.
    ActivateTab(usize),
    /// Close the tab at this index (the `Main` tab is never closable).
    CloseTab(usize),
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
