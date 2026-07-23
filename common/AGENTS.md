# common

Cross-crate contracts and small shared utilities: typed IDs, serialization,
introspection, cancellation, file discovery/publication, spans, and numerical
extension traits. Pure leaf crate — depended on by everything, depends on
nothing in-tree.

## Modules

| Module | Role |
|--------|------|
| `cancel_token.rs` | `CancelToken`: shared poll-only cooperative cancel token (enum over `Never` / `Live(Arc<AtomicBool>)`, encapsulated in a tuple struct). `new()` = live, `never()`/`default()` = the zero-cost "no cancellation" case — so an op takes a plain `CancelToken`, never `Option<CancelToken>`. `cancel()` trips every clone; `reset()` rearms them after the prior operation has joined. For cooperative bail-out in hot loops (`spawn_blocking`/rayon). No async wait — use `tokio_util`'s token for that. |
| `macros.rs` | `id_type!` (strongly-typed UUID wrappers) + `cfg_x86_64!` / `cfg_aarch64!` arch-gate macros. |
| `serde.rs` | Generic `serialize`/`deserialize` dispatching over `SerdeFormat`, with typed `SerializeError` / `DeserializeError` failures and private consuming TOML text normalization. |
| `file_format.rs` | `SerdeFormat` enum + extension-based format detection. |
| `file_utils/` | Generic fallible, sorted directory scanning by file extension plus one-shot and two-phase atomic same-directory publication with durable and rebuildable-cache modes. |
| `span.rs` | `Span`: compact serde `(start, len)` u32 range into a flat SoA pool; 8 bytes vs 16 for `Range<usize>`. Used by `scenarium`'s execution program. |
| `introspect/` | Generic struct introspection: the traits and value types are always available; the nested `common-derive` proc-macro and its `#[derive(Introspect)]` / `#[derive(IntrospectEnum)]` re-exports require the `introspect-derive` feature. `fields()` reports `FieldDesc` name/label/concrete numeric `FieldKind`/default/required, and checked `from_fields` rebuilds the value. Signed and unsigned integer defaults remain lossless; out-of-range authored values error instead of casting. Enum derives require an explicit stable `#[config(type_id = "…")]` UUID and delegate the string round-trip to `Display`/`FromStr`. |
| `float_ext.rs` | `FloatExt::approximately_eq` for `f32`/`f64`/`Vec2` (within `EPSILON`). |
| `constants.rs` | `EPSILON: f32 = 1e-6`. |
| `debug.rs` | `is_debug()`: reports `debug_assertions`. |
| `test_utils.rs` | `workspace_root`, `test_output_path` for tests. |

## Key types

- `SerdeFormat` — `Json | Bitcode | Toml | Lz4`; selected by file extension via `from_file_name`. (Lua, Rhai, and the old `Scn`/`ScnText` formats were removed.)

`id_type!` generates a `#[repr(transparent)]` UUID wrapper deriving Clone/Copy/Eq/Ord/Hash/serde, with `unique`/`nil`/`from_u128`/`is_nil`/`as_u128`/`as_u64_pair`, `FromStr` + `From<&str>`/`From<String>` parsing, and `Display`.

## Serialization

`serialize()` returns `Result<Vec<u8>, SerializeError>`; `deserialize()` dispatches directly from a byte slice; `serialize_into`/`deserialize_from` stream over `Write`/`Read`. JSON and TOML are UTF-8. `Bitcode` is binary. `Lz4` is compact JSON LZ4-compressed with a 4-byte length prefix.

## Dependencies

serde, serde_json, toml, bitcode, lz4_flex, glam, thiserror, optional
common-derive, and `windows-sys` on Windows.
