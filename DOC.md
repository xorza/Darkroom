# Scenarium Project Overview

Scenarium collects the tooling required to build node based applications. The repository is a Rust workspace containing the core graph implementation and several editor front‑ends.

## Repository Layout

- **common** – shared utilities and helper macros
- **graph** – Rust library defining graphs, nodes and the runtime
- **ScenariumEditor.Svelte+Tauri** – cross‑platform editor built with Svelte (frontend) and Tauri (backend)
- **test_resources** – sample graphs, function libraries and media used by tests
- **test_output** – output folder created by unit tests
- **deprecated_code** – older .NET and QML editors and experimental code

Each directory may contain its own `Cargo.toml` or build scripts.
The root `Cargo.toml` defines the workspace and shared dependencies.

Commit messages are often prompts sent to an AI agent to request a change.

## Subprojects

### common crate
Provides small utilities such as logging helpers, macros for unique identifier types, and miscellaneous functions.
See `common/src` for implementation details.

### graph crate
Implements the data structures for graphs and nodes. Nodes are created from functions defined in a function library.
Connections between nodes are represented by `Binding::Output` values.
Data structures life graph and funtion library can be serialized to YAML files.

Runtime execution is handled by the `runtime_graph` module which determines which nodes should run each tick.

### Svelte + Tauri editor
Located in `ScenariumEditor.Svelte+Tauri`. The `frontend` folder contains the Svelte UI while `src-tauri` contains the Rust code that exposes commands to the UI. The editor displays nodes and connections, allows panning/zooming and exposes a **Function Library** panel for inserting nodes. Functions in this panel are highlighted on hover, showing their description as a tooltip, and presented in a compact list for easier browsing. Dragging a function out of the panel spawns a new node that follows the cursor until the mouse button is released.

## Common Terms

- **Graph connection** – a link from an output pin on one node to an input pin on another.
- The editor ensures each input has at most one incoming connection.
- **Connection breaker** – a tool in the Svelte editor that lets you draw a path with the right mouse button to sever any connections it intersects.
- **Function library** – the list of available node types.
- It can be fetched in the editor via the `get_func_library` Tauri command and is backed by data such as `test_resources/test_funcs.yml`.
- **Graph view** – contains the current nodes and connections. Use `get_graph_view` to fetch it and `add_node_to_graph_view` to persist new nodes created in the editor.
- Use `add_connection_to_graph_view` to persist a single connection and `remove_connections_from_graph_view` to delete one or more connections.
- **Pending connection** is a connection that has not yet been confirmed and currently being edited by user.


