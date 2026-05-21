pub mod breaker;
pub mod connection_ui;
pub mod graph_ui;
pub mod main_window;
pub mod node_ui;

/// Layout constants shared by the node widget (visual placement)
/// and the connection-endpoint computer in `view.rs`. Single source
/// of truth so the painted ports line up with the bezier endpoints.
pub const NODE_W: f32 = 160.0;
pub const PORT_SIZE: f32 = 10.0;
pub const PORT_RADIUS: f32 = PORT_SIZE * 0.5;
/// Vertical inset at the top of each port column (gap below the
/// header band before the first port).
pub const PORT_COL_PAD_TOP: f32 = 6.0;
/// Vertical gap between adjacent ports in a column.
pub const PORT_GAP: f32 = 6.0;

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
