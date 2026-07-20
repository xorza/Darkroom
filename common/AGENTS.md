# common

Cross-crate contracts and small shared utilities: typed IDs, serialization,
introspection, cancellation, file discovery/publication, spans, and numerical
extension traits. Pure leaf crate — depended on by everything, depends on
nothing in-tree.

## Modules

| Module | Role |
|--------|------|
| `cancel_token.rs` | `CancelToken`: shared poll-only cooperative cancel token (enum over `Never` / `Live(Arc<AtomicBool>)`, encapsulated in a tuple struct). `new()` = live, `never()`/`default()` = the zero-cost "no cancellation" case — so an op takes a plain `CancelToken`, never `Option<CancelToken>`. `cancel()`/`is_cancelled()`/`reset()`; live clones share one flag. For cooperative bail-out in hot loops (`spawn_blocking`/rayon); `reset()` makes a live token reusable across operations. No async wait — use `tokio_util`'s token for that. |
| `macros.rs` | `id_type!` (strongly-typed UUID wrappers) + `cfg_x86_64!` / `cfg_aarch64!` arch-gate macros. |
| `serde.rs` | Generic `serialize`/`deserialize` dispatching over `SerdeFormat`, with typed `SerializeError` / `DeserializeError` failures. |
| `file_format.rs` | `SerdeFormat` enum + extension-based format detection. |
| `file_utils/` | Generic fallible, sorted directory scanning by file extension plus atomic same-directory file publication with durable and rebuildable-cache modes. |
| `span.rs` | `Span`: compact serde `(start, len)` u32 range into a flat SoA pool; 8 bytes vs 16 for `Range<usize>`. Used by `scenarium`'s execution program. |
| `introspect/` | Generic struct introspection: `#[derive(Introspect)]` (from the nested `common-derive` proc-macro crate) → `fields()` (`FieldDesc`: name/label/concrete numeric `FieldKind`/default/required) + checked `from_fields(&[FieldValue]) -> Result<_, IntrospectError>`. Signed and unsigned integer defaults remain lossless; out-of-range authored values error instead of casting. GUI/value-model agnostic — consumers map `FieldDesc` to their own widgets. Fieldless enum fields impl `IntrospectEnum` via `#[derive(IntrospectEnum)]` plus an explicit stable `#[config(type_id = "…")]` UUID (lists the variants + delegates the string round-trip to `Display`/`FromStr` — typically strum's, but the derive itself is strum-agnostic). (`darkroom`/`lens` build config editors on it.) |
| `float_ext.rs` | `FloatExt::approximately_eq` for `f32`/`f64`/`Vec2` (within `EPSILON`). |
| `normalize_string.rs` | `NormalizeString::normalize`: CRLF/CR → LF, guarantees trailing newline. |
| `constants.rs` | `EPSILON: f32 = 1e-6`. |
| `debug.rs` | `is_debug()`: reports `debug_assertions`. |
| `test_utils.rs` | `workspace_root`, `test_output_path` for tests. |

## Key types

- `SerdeFormat` — `Json | Bitcode | Toml | Lz4`; selected by file extension via `from_file_name`. (Lua, Rhai, and the old `Scn`/`ScnText` formats were removed.)

`id_type!` generates a `#[repr(transparent)]` UUID wrapper deriving Clone/Copy/Eq/Ord/Hash/serde, with `unique`/`nil`/`from_u128`/`is_nil`/`as_u128`/`as_u64_pair`, `FromStr` + `From<&str>`/`From<String>` parsing, and `Display`.

## Serialization

`serialize()` returns `Result<Vec<u8>, SerializeError>`; `deserialize()` accepts bytes and returns `Result<T, DeserializeError>`; `serialize_into`/`deserialize_from` stream over `Write`/`Read`. JSON and TOML are UTF-8. `Bitcode` is binary. `Lz4` is compact JSON LZ4-compressed with a 4-byte length prefix.

## Dependencies

common-derive, serde, serde_json, toml, bitcode, lz4_flex, glam, thiserror, and
`windows-sys` on Windows.
