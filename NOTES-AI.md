# Scenarium Implementation Notes (AI)

This file captures current implementation details and internal structure for AI agents.

## Project Overview

Scenarium is a Rust-based framework for building node-based data processing pipelines with async-capable function
execution. The repository is a Cargo workspace containing a core graph library, shared utilities, and a visual
egui-based editor.

**License:** AGPL  
**Build System:** Cargo workspace with 4 member crates

## Repository Layout

```
scenarium/
├── common/          # Shared utilities and helper macros
├── graph/           # Core graph library with execution engine
├── editor/          # Visual egui-based graph editor
├── vision/          # Image processing funclib (imaginarium adapter)
├── test_resources/  # Sample graphs and media for tests
├── test_output/     # Output folder created by unit tests
└── deprecated_code/ # Older .NET/QML editors (historical)
```

## Crates

### common

Shared utilities used across the workspace.

**Key modules:**
- `key_index_vec.rs` - Generic key-indexed vector with HashMap lookup; supports compaction with guards
- `shared.rs` - Thread-safe `Shared<T>` wrapper around `Arc<Mutex<T>>`
- `lambda.rs` - Async lambda patterns for callback support
- `macros.rs` - ID type generation macros (`id_type!`), async lambda macros
- `serde.rs` - Generic serialization (YAML, JSON, binary via bincode)
- `serde_lua.rs` - Lua-specific serialization support
- `file_format.rs` - Format detection and selection
- `pause_gate.rs` - Synchronization primitive for pausing/resuming async operations
- `ready_state.rs` - Readiness synchronization using tokio barriers
- `slot.rs` - Wait-friendly slot for async value passing
- `bool_ext.rs` - `BoolExt` trait with `then_else` helpers

**Dependencies:** tokio, serde, tracing, bumpalo, lz4_flex, bitcode

### graph

Core data structures for node-based graph execution with async lambdas.

**Key modules:**

- `graph.rs` - Core `Graph` struct with `NodeId`, `Node`, `Input`, `Binding`, `Event`
- `function.rs` - Function library definitions (`FuncLib`, `Func`, `FuncInput`, `FuncOutput`)
- `data.rs` - Data type and value systems (`DataType`, `StaticValue`, `DynamicValue`)
- `execution_graph.rs` - Execution scheduling and state management
- `func_lambda.rs` - Async function lambdas (`FuncLambda`, `AsyncLambdaFn`)
- `event_lambda.rs` - Async event callbacks (`EventLambda`)
- `context.rs` - Context registry for invocation environments (`ContextManager`)
- `worker.rs` - Background tokio thread for graph execution
- `compute.rs` - Node invocation and value conversion

**Elements (Built-in Functions):**
- `elements/basic_funclib.rs` - Math operations (add, subtract, multiply, divide, power, log, etc.)
- `elements/worker_events_funclib.rs` - Timer/frame events with FPS tracking
- `elements/lua/` - Lua function loading and invocation via mlua

**Dependencies:** common, tokio, serde, uuid, anyhow, thiserror, strum, mlua (Lua54), glam, hashbrown, criterion

### scenarium-editor

Visual egui-based editor for graph creation and execution.

**Core Application:**
- `main.rs` - eframe entry point with window setup
- `app_data.rs` - Application state management (`AppData`)
- `main_ui.rs` - Top-level UI rendering

**Model Layer:**
- `model/view_graph.rs` - Editor view representation with pan/zoom/selection
- `model/view_node.rs` - Per-node view state (position)
- `model/graph_ui_action.rs` - User actions enum for undo/redo
- `model/config.rs` - Persistent configuration

**GUI Components:**
- `gui/graph_ui.rs` - Main graph canvas and interaction
- `gui/node_ui.rs` - Individual node rendering
- `gui/graph_layout.rs` - Layout computation and caching
- `gui/connection_ui.rs` - Connection rendering with bezier curves
- `gui/connection_breaker.rs` - Tool for breaking/rerouting connections
- `gui/const_bind_ui.rs` - Const input binding badges and editors
- `gui/node_details_ui.rs` - Side panel for selected node
- `gui/new_node_ui.rs` - Dialog for adding new nodes
- `gui/graph_background.rs` - Dotted grid background
- `gui/polyline_mesh.rs` - Polyline mesh generation
- `gui/log_ui.rs` - Status/log panel
- `gui/style.rs` - Theme and styling centralization

**UI Primitives:**
- `common/button.rs` - Custom button with styling
- `common/toggle_button.rs` - Toggle button with state
- `common/drag_value.rs` - Inline draggable value editor
- `common/bezier_helper.rs` - Bezier intersection and sampling
- `common/undo_stack.rs` - Undo/redo implementations

**Dependencies:** common, graph, egui, eframe, wgpu, tokio, serde, rayon, rfd, arc-swap, bumpalo, lz4_flex

### vision

Image processing function library adapting imaginarium operations to the node-based workflow.

**Key modules:**
- `image_funclib.rs` - Image processing functions (brightness_contrast, etc.)
- `vision_ctx.rs` - `VisionCtx` with `ProcessingContext` for GPU/CPU image operations

**Key types:**
- `ImageFuncLib` - Function library with image processing nodes
- `VisionCtx` - Context holding `imaginarium::ProcessingContext` for GPU/CPU dispatch
- `VISION_CTX_TYPE` - Lazy-initialized `ContextType` for context manager

**Current functions:**
- `brightness_contrast` - Adjusts image brightness and contrast using imaginarium

**Dependencies:** common, graph, imaginarium

## Key Data Structures

### Graph Elements

```rust
NodeId(uuid)      // Unique node identifier
FuncId(uuid)      // Unique function identifier  
TypeId(uuid)      // Custom type identifier
PortAddress       // { target_id: NodeId, port_idx: usize }
EventRef          // { node_id: NodeId, event_idx: usize }
```

### Binding System

```rust
Binding:
  - None                    // Unconnected
  - Const(StaticValue)      // Constant value
  - Bind(PortAddress)       // Data flow from another node's output
```

### Execution State

```rust
InputState: Changed | Unchanged
OutputUsage: Skip | Needed
ExecutionBehavior: Impure | Pure | Once
NodeBehavior: AsFunction | Once
```

### Value Types

```rust
DataType: Null | Float | Int | Bool | String | Array | Custom(TypeId)
StaticValue: Serializable constants (f64 equality via to_bits())
DynamicValue: Runtime values including Arc<dyn Any + Send + Sync> for Custom variant (shallow clone)
```

## Architecture Patterns

### Lambda-Based Function Execution
- Functions defined as async Rust closures (`FuncLambda`)
- Lua scripting integration via mlua
- Async/await support throughout execution
- `async_lambda!` macro reduces boilerplate

### Execution Graph Scheduling
- DFS-based topological ordering with cycle detection
- Three-phase scheduling: backward (identify needs), forward (mark execution), terminal discovery
- Input state tracking for pure function optimization
- Output usage counting to skip unnecessary computations
- Reusable `processing_stack` cache for cycle detection

### Event System
- Nodes emit named events with subscriber lists
- Event lambdas run in async worker loop
- Frame events with configurable frequency (`FpsEventState`)

### Worker Pattern
- Background tokio task receives messages (Update, Event, Clear, ExecuteTerminals)
- Execution callbacks with stats (elapsed time, node timings, missing inputs)
- Event loop with stop flag and notify broadcast
- `EventLoopHandle` for controlling event loops

### UI Architecture
- Immediate-mode with egui
- Arena allocation (bumpalo) for frame-local data
- Persistent layout caching with in-place updates (`CompactInsert`)
- Undo/redo via action replay with LZ4 compression
- Cycle detection prevents invalid graph connections

### State Management
- `AnyState` - Type-erased HashMap for per-node state
- `SharedAnyState` - Arc<Mutex<AnyState>> for event state sharing
- `ContextManager` - Lazy-initialized context registry
- `Slot` - Wait-friendly value passing between async boundaries

## ID Generation

The `id_type!` macro generates strongly-typed UUID wrappers:
- Implements Debug, Clone, Copy, Eq, Hash, serde traits
- `From<&str>`/`From<String>` for parsing UUIDs
- `from_u128` for const-friendly initialization

## Serialization

- `common::serialize` returns `Vec<u8>`; `deserialize` accepts bytes
- Text formats (YAML/JSON) are UTF-8 bytes
- Binary format uses bincode
- `FileFormat` enum for format selection and auto-detection by extension
- `StaticValue` uses `f64::to_bits()` for deterministic float equality

## Testing

- Unit tests in-module via `#[cfg(test)]` blocks
- `test_graph()` and `test_func_lib()` fixtures for reproducible testing
- Criterion benchmarks in `graph/benches/b1.rs`
- `TestFuncHooks` with Arc callbacks for async test support

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

## Style System

`Style` centralizes all UI appearance constants:
- Initialized from `StyleSettings` (TOML serializable)
- Scaled by zoom factor via `Style::new(scale)`
- Contains `NodeStyle`, `ConnectionStyle`, `GraphBackgroundStyle`
- Applied to egui visuals via `Style::apply_to_egui`

## Undo System

Two implementations available:
- `FullSerdeUndoStack` - Stores full serialized snapshots
- `ActionUndoStack` - Stores `GraphUiAction` values with binary compression

Both support byte limits for memory management.

## Configuration

- `config.toml` - Current graph path, window geometry
- Window position/size persists via eframe
- `StyleSettings` supports TOML persistence

## Dependencies Overview

**Core Runtime:** tokio, serde, anyhow/thiserror, uuid  
**Data:** hashbrown, glam, bumpalo, lz4_flex  
**Scripting:** mlua (Lua 5.4)  
**UI:** egui, eframe, wgpu, rayon, rfd  
**Utilities:** strum, arc-swap, tracing
