# common

Shared utilities used across the workspace: buffers, ID types, serialization, and async synchronization primitives.

## Modules

| Module | Role |
|--------|------|
| `key_index_vec.rs` | `KeyIndexVec`: key-indexed vector with HashMap lookup; supports compaction with guards. |
| `shared.rs` | `Shared<T>`: thread-safe `Arc<Mutex<T>>` wrapper; `lock()` returns the guard. |
| `lambda.rs` | Async lambda patterns for callback support. |
| `macros.rs` | `id_type!` (strongly-typed UUID wrappers) + async lambda macros. |
| `serde.rs` | Generic serialize/deserialize over `SerdeFormat` (JSON, Lua, Bitcode, TOML, Scn, ScnText). |
| `serde_lua.rs` | Lua-specific serialization support. |
| `file_format.rs` | Format detection + selection. |
| `pause_gate.rs` | `PauseGate`: pause/resume synchronization for async operations. |
| `ready_state.rs` | `ReadyState`: readiness synchronization on tokio barriers. |
| `slot.rs` | `Slot<T>`: wait-friendly slot for async value passing. |
| `bool_ext.rs` | `BoolExt` trait (`then_else` helpers). |

## Key types

- `Shared<T>` — `Arc<Mutex<T>>` wrapper.
- `Slot<T>` — wait-friendly value passing across async boundaries.
- `PauseGate` / `ReadyState` — async pause/readiness primitives.
- `SerdeFormat` — `Json | Lua | Bitcode | Toml | Scn | ScnText`; format selection + auto-detection by extension.

`id_type!` generates UUID wrappers implementing Debug/Clone/Copy/Eq/Hash/serde, `From<&str>`/`From<String>` parsing, and `from_u128` for const init.

## Serialization

`serialize()` returns `Vec<u8>`; `deserialize()` accepts bytes. Text formats (JSON/Lua/TOML/ScnText) are UTF-8; binary formats are Bitcode and Scn (LZ4-compressed Lua).

## Dependencies

tokio, serde, tracing, bumpalo, lz4_flex, bitcode.
