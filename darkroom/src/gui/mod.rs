pub mod breaker;
pub mod connection_ui;
pub mod graph_ui;
pub mod main_window;
pub mod menu_bar;
pub mod new_node_ui;
pub mod node_ui;
pub mod selection_ui;
pub mod value_editor;

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
