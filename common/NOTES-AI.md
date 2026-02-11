# common - Implementation Notes (AI)

Shared utilities used across the workspace.

## Key Modules

| Module | Description |
|--------|-------------|
| `key_index_vec.rs` | Generic key-indexed vector with HashMap lookup; supports compaction with guards |
| `shared.rs` | Thread-safe `Shared<T>` wrapper around `Arc<Mutex<T>>` |
| `lambda.rs` | Async lambda patterns for callback support |
| `macros.rs` | ID type generation macros (`id_type!`), async lambda macros |
| `serde.rs` | Generic serialization (JSON, Lua, Bitcode, TOML, SCN, ScnText) |
| `serde_lua.rs` | Lua-specific serialization support |
| `file_format.rs` | Format detection and selection |
| `pause_gate.rs` | Synchronization primitive for pausing/resuming async operations |
| `ready_state.rs` | Readiness synchronization using tokio barriers |
| `slot.rs` | Wait-friendly slot for async value passing |
| `bool_ext.rs` | `BoolExt` trait with `then_else` helpers |

## Key Types

```rust
Shared<T>          // Arc<Mutex<T>> wrapper with lock() returning MutexGuard
Slot<T>            // Wait-friendly value passing between async boundaries
PauseGate          // Synchronization for pausing/resuming async operations
ReadyState         // Readiness synchronization using tokio barriers
SerdeFormat        // Json | Lua | Bitcode | Toml | Scn | ScnText - format selection and auto-detection
```

## ID Generation

The `id_type!` macro generates strongly-typed UUID wrappers:
- Implements Debug, Clone, Copy, Eq, Hash, serde traits
- `From<&str>`/`From<String>` for parsing UUIDs
- `from_u128` for const-friendly initialization

## Serialization

- `serialize()` returns `Vec<u8>`; `deserialize()` accepts bytes
- Text formats (JSON/Lua/TOML/ScnText) are UTF-8 bytes
- Binary formats: Bitcode, Scn (LZ4-compressed Lua)
- `SerdeFormat` enum for format selection and auto-detection by extension

## Dependencies

tokio, serde, tracing, bumpalo, lz4_flex, bitcode
