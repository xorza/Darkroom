# Scenarium Implementation Notes (AI)

This file captures current project overview. See crate-specific `NOTES-AI.md` files for detailed implementation notes.

## Project Overview

Scenarium is a Rust-based framework for building node-based data processing pipelines with async-capable function execution. The repository is a Cargo workspace containing a core graph library, shared utilities, a visual egui-based editor, and image processing libraries for astrophotography.

**License:** AGPL  
**Build System:** Cargo workspace with 6 member crates

## Repository Layout

```
scenarium/
├── common/          # Shared utilities and helper macros
├── scenarium/       # Core graph library with execution engine
├── imaginarium/     # GPU/CPU image processing library
├── lumos/           # Astrophotography image processing library
├── prism/           # Visual egui-based graph editor
├── palantir/        # Image processing funclib (imaginarium adapter)
├── test_resources/  # Sample graphs and media for tests
├── test_output/     # Output folder created by unit tests
└── deprecated_code/ # Older .NET/QML editors (historical)
```

## Crate Overview

| Crate | Description |
|-------|-------------|
| `common` | Shared utilities: thread-safe wrappers, async patterns, serialization, ID macros |
| `scenarium` | Core graph library: nodes, functions, execution engine, Lua scripting |
| `prism` | Visual egui-based editor for graph creation and execution |
| `imaginarium` | GPU/CPU image processing with automatic backend selection |
| `palantir` | Image processing function library adapting imaginarium to node workflow |
| `lumos` | Astrophotography library: FITS/RAW loading, stacking, star detection, registration |

## Key Architecture Patterns

### Lambda-Based Function Execution
- Functions defined as async Rust closures (`FuncLambda`)
- Lua scripting integration via mlua
- Async/await support throughout execution

### Execution Graph Scheduling
- DFS-based topological ordering with cycle detection
- Three-phase scheduling: backward (identify needs), forward (mark execution), terminal discovery
- Input state tracking for pure function optimization

### ID Generation
The `id_type!` macro generates strongly-typed UUID wrappers with Debug, Clone, Copy, Eq, Hash, serde traits.

### Serialization
- `common::serialize` returns `Vec<u8>`; `deserialize` accepts bytes
- Text formats (JSON/Lua/TOML/ScnText) are UTF-8 bytes
- Binary formats: Bitcode, Scn (LZ4-compressed Lua)

## Dependencies Overview

**Core Runtime:** tokio, serde, anyhow/thiserror, uuid  
**Data:** hashbrown, glam, bumpalo, lz4_flex  
**Scripting:** mlua (Lua 5.4)  
**UI:** egui, eframe, wgpu, rayon, rfd  
**Image Processing:** wgpu, image, tiff, fitsio, rawloader, libraw-rs
