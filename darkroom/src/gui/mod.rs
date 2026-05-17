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

#[derive(Clone, Copy, Debug)]
pub enum Side {
    Left,
    Right,
}
