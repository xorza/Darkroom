# common

Shared utilities used across the workspace: 2D buffers, ID types, serialization,
async synchronization primitives, primitive-type extension traits, and small
helpers. Pure leaf crate — depended on by everything, depends on nothing in-tree.

## Modules

| Module | Role |
|--------|------|
| `bit_buffer2.rs` | `BitBuffer2`: bit-packed 2D boolean buffer; u64 words, rows aligned to 128 bits, 16-byte-aligned for SIMD. 8× smaller than `Vec<bool>`. (The generic `Buffer2<T>` pixel buffer now lives in `imaginarium`.) |
| `vec2us.rs` | `Vec2us`: 2D `usize` vector for pixel coordinates/dimensions (`x`, `y`); index conversions (`to_index`/`from_index`), `Add`/`Sub`, tuple `From`. |
| `rgb.rs` | `Rgb`: an RGB color as three `f32` channels (`r`, `g`, `b`); `intensity()` (unweighted channel mean), `scale()`, `ZERO`. |
| `key_index_vec.rs` | `KeyIndexVec<K, V>`: vec + `HashMap` index giving O(1) key lookup with stable iteration order; compaction via a drop guard. |
| `shared.rs` | `Shared<T>`: `Arc<Mutex<T>>` convenience wrapper; derefs to the inner `Arc`. |
| `shared_fn.rs` | `SharedFn<F>`: `Option`-like `Arc`-wrapped callable. |
| `slot.rs` | `Slot<T>`: lockless single-slot async value channel (`ArcSwapOption` + `Notify`); only the latest value is retained. |
| `pause_gate.rs` | `PauseGate`: pause/resume gate; `close()` returns a guard that reopens on drop. |
| `ready_state.rs` | `ReadyState`: barrier-like counter that notifies waiters once `total` signals arrive. |
| `cancel_token.rs` | `CancelToken`: shared poll-only cooperative cancel token (enum over `Never` / `Live(Arc<AtomicBool>)`, encapsulated in a tuple struct). `new()` = live, `never()`/`default()` = the zero-cost "no cancellation" case — so an op takes a plain `CancelToken`, never `Option<CancelToken>`. `cancel()`/`is_cancelled()`/`reset()`; live clones share one flag. For cooperative bail-out in hot loops (`spawn_blocking`/rayon); `reset()` makes a live token reusable across operations. No async wait — use `tokio_util`'s token for that. |
| `macros.rs` | `id_type!` (strongly-typed UUID wrappers) + `cfg_x86_64!` / `cfg_aarch64!` arch-gate macros. |
| `serde.rs` | Generic `serialize`/`deserialize` dispatching over `SerdeFormat`. |
| `serde_rhai/` | Rhai text (de)serialization via a `serde_json::Value` intermediary. |
| `file_format.rs` | `SerdeFormat` enum + extension-based format detection. |
| `file_utils.rs` | Directory scanning for RAW/FITS astro image files. |
| `cpu_features.rs` | `X86Features`: cached runtime SSE/AVX2/FMA detection (x86_64; stubbed elsewhere). |
| `parallel.rs` | `par_map_limited` / `try_par_map_limited`: concurrency-capped parallel map. |
| `fnv.rs` | `FnvHasher`: deterministic FNV-1a 64-bit hasher (fixed seed). |
| `span.rs` | `Span`: compact serde `(start, len)` u32 range into a flat SoA pool; 8 bytes vs 16 for `Range<usize>`. Used by `scenarium`'s execution program. |
| `introspect.rs` | Generic struct introspection: `#[derive(Introspect)]` (from the nested `common-derive` proc-macro crate) → `fields()` (`FieldDesc`: name/label/`FieldKind`/default/required) + `from_fields(&[FieldValue])` typed rebuild. GUI/value-model agnostic — consumers map `FieldDesc` to their own widgets. Fieldless enum fields impl `IntrospectEnum` via `#[derive(IntrospectEnum)]` (lists the variants + delegates the string round-trip to `Display`/`FromStr` — typically strum's, but the derive itself is strum-agnostic). (`darkroom`/`lens` build config editors on it.) |
| `float_ext.rs` | `FloatExt::approximately_eq` for `f32`/`f64`/`Vec2` (within `EPSILON`). |
| `normalize_string.rs` | `NormalizeString::normalize`: CRLF/CR → LF, guarantees trailing newline. |
| `constants.rs` | `EPSILON: f32 = 1e-6`. |
| `debug.rs` | `is_debug()`: reports `debug_assertions`. |
| `test_utils.rs` | `workspace_root`, `test_output_path` for tests. |

## Key types

- `BitBuffer2` — bit-packed 2D bool buffer; indexes by `(x, y)` tuple and linear `usize`. No `IndexMut` (bits aren't independently addressable); mutate via `set`/`set_xy`. (The generic pixel `Buffer2<T>` moved to `imaginarium`.)
- `KeyIndexVec<K, V>` — `V: KeyIndexKey<K>` (must expose `key(&self) -> &K`). `add`/`remove_by_key`/`by_key`/`index_of_key`, plus `compact_insert_start()` returning a `CompactInsert` guard whose `insert_with` deduplicates and which truncates + reindexes on drop.
- `Slot<T>` — `send` (overwrites), `take`/`take_async`, `peek`/`peek_async` (Arc clone, keeps). For cross-task async value passing.
- `PauseGate` / `ReadyState` — race-safe via registering the `Notify` future *before* checking the flag/count.
- `SerdeFormat` — `Json | Rhai | Bitcode | Toml | Lz4`; selected by file extension via `from_file_name`. (Lua and the old `Scn`/`ScnText` formats were removed.)

`id_type!` generates a `#[repr(transparent)]` UUID wrapper deriving Clone/Copy/Eq/Ord/Hash/serde, with `unique`/`nil`/`from_u128`/`is_nil`/`as_u128`/`as_u64_pair`, `FromStr` + `From<&str>`/`From<String>` parsing, and `Display`.

## Serialization

`serialize()` returns `Vec<u8>`; `deserialize()` accepts bytes; `serialize_into`/`deserialize_from` stream over `Write`/`Read`. Text formats (JSON, Rhai, TOML) are UTF-8. `Bitcode` is binary. `Lz4` is Rhai text LZ4-compressed with a 4-byte length prefix. Rhai is the canonical text format; deserialization routes through `serde_json::Value` for lenient numeric coercion, and the Rhai engine runs sandboxed (capped ops/depth/sizes, no variables/modules).

## Dependencies

tokio, rayon, serde, serde_json, rhai, toml, bitcode, lz4_flex, glam, aligned-vec, arc-swap, ryu, anyhow, thiserror.
