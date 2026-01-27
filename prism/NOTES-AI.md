# prism - Implementation Notes (AI)

Visual egui-based editor for graph creation and execution.

## Core Application

| Module | Description |
|--------|-------------|
| `main.rs` | eframe entry point with window setup |
| `app_data.rs` | Application state management (`AppData`) |
| `main_ui.rs` | Top-level UI rendering |

## Model Layer

| Module | Description |
|--------|-------------|
| `model/view_graph.rs` | Editor view representation with pan/zoom/selection |
| `model/view_node.rs` | Per-node view state (position) |
| `model/graph_ui_action.rs` | User actions enum for undo/redo |
| `model/config.rs` | Persistent configuration |

## GUI Components

| Module | Description |
|--------|-------------|
| `gui/graph_ui.rs` | Main graph canvas and interaction |
| `gui/node_ui.rs` | Individual node rendering |
| `gui/graph_layout.rs` | Layout computation and caching |
| `gui/connection_ui.rs` | Connection rendering with bezier curves |
| `gui/connection_breaker.rs` | Tool for breaking/rerouting connections |
| `gui/const_bind_ui.rs` | Const input binding badges and editors |
| `gui/node_details_ui.rs` | Side panel for selected node |
| `gui/new_node_ui.rs` | Dialog for adding new nodes |
| `gui/graph_background.rs` | Dotted grid background |
| `gui/polyline_mesh.rs` | Polyline mesh generation |
| `gui/log_ui.rs` | Status/log panel |
| `gui/style.rs` | Theme and styling centralization |

## UI Primitives (common/)

| Module | Description |
|--------|-------------|
| `common/button.rs` | Custom button with styling |
| `common/toggle_button.rs` | Toggle button with state |
| `common/drag_value.rs` | Inline draggable value editor |
| `common/bezier_helper.rs` | Bezier intersection and sampling |
| `common/undo_stack.rs` | Undo/redo implementations |
| `common/value_editor.rs` | `StaticValueEditor` for unified value editing |
| `common/id_salt.rs` | Standardized ID generation for egui |
| `common/drag_state.rs` | `DragState<T>` for drag operations |

## Style System

`Style` centralizes all UI appearance constants:
- Initialized from `StyleSettings` (TOML serializable)
- Scaled by zoom factor via `Style::new(scale)`
- Contains `NodeStyle`, `ConnectionStyle`, `GraphBackgroundStyle`
- Applied to egui visuals via `Style::apply_to_egui`

### Port Colors

`PortColors` struct provides unified port color selection:
- `NodeStyle::port_colors(PortKind) -> PortColors` returns base/hover color pair
- `PortColors::select(hovered: bool) -> Color32` picks the appropriate color

### Graph Interaction State

`GraphInteractionState` centralizes UI interaction mode tracking:
- `InteractionMode` enum: `Idle`, `BreakingConnections`, `DraggingNewConnection`, `PanningGraph`
- Encapsulates `ConnectionBreaker` state within the interaction state

## Undo System

Two implementations available:
- `FullSerdeUndoStack` - Stores full serialized snapshots
- `ActionUndoStack` - Stores `GraphUiAction` values with binary compression

Both support byte limits for memory management.

## Editor Features

- Node drag, selection, reordering (selected nodes render on top)
- Connection creation with bezier curves and gradient coloring
- Connection breaker tool for rerouting
- Const input badges with inline editing
- Cache toggle per node (Once behavior)
- Impure function status indicator
- Undo/redo with action-based snapshots (LZ4 compressed)
- Fit-all and view-selected camera controls
- Autorun toggle for automatic execution
- Cycle detection before applying connections
- Dotted grid background with zoom-aware spacing

## Configuration

- `config.toml` - Current graph path, window geometry
- Window position/size persists via eframe
- `StyleSettings` supports TOML persistence

## Dependencies

common, scenarium, egui, eframe, wgpu, tokio, serde, rayon, rfd, arc-swap, bumpalo, lz4_flex
