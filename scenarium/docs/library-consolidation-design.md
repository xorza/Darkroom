# Library consolidation + type registry — design

Status: **phases 1–3 implemented** (phases 4–5 remain).

Phase 2 (done): `CustomValue::type_def() -> Arc<TypeDef>` became `type_id() -> TypeId`
(the `Any` supertrait was dropped for an explicit `'static` to avoid the
`Any::type_id` name clash; downcasting still goes through `as_any`).

Phase 3 (done): `DataType::Custom`/`Enum` now hold only a `TypeId`; `TypeDef`/
`EnumDef` are deleted (metadata is `Library`'s `TypeDecl`). `PartialEq`/`Eq` are
derived; `Display for DataType` is gone — type names resolve via
`Library::type_name`/`type_decl`. `const_satisfies` and the editor's enum picker
read variants from `Library::enum_variants`; `DataType::default_value` returns
`None` for `Custom`/`Enum` (the enum first-variant default is seeded by
`enum_input::<E>` from the concrete type). All nominal types are registered in
their `*_library()` builder (image/blend-mode/conversion-format, masters, and the
astro config + dynamic-enum types via `add_config_builder`). The editor threads
`&Library` through `RecordCtx` / the inspector's `PanelDraw` for type display.
The dynamic-enum ids still use the `FnvHasher` hack — that's phase 5.

Decisions locked in: `DataType` goes **id-only** (Custom/Enum carry just a `TypeId`);
`FuncLib` is **renamed `Library` and moved to `library.rs`** (it holds funcs +
subgraphs + types); the output cache holds an `Arc<Library>` for codec lookup.

Phase 1 (done): `Library` gained a `types: HashMap<TypeId, TypeEntry>` table
(`TypeEntry` = serializable `TypeDecl` + `#[serde(skip)]` codec); `CustomValueRegistry`
is deleted and codecs dispatch through `Library::codec`; `OutputCache` holds
`Arc<Library>`; `lens` registers the `Image` type via `Library::register_type`.
`DataType` still carries inline `Arc<TypeDef>`/`EnumDef` — the id-only flip is
phases 2–4 below.

Consolidate the three type-aware registries into one `Library`, give every
nominal type a registered id, and make `DataType` reference types **by id**
instead of carrying their metadata inline.

## 1. Today

Three separate type-aware things, threaded on two paths:

| Thing | Location | Keyed by | Serializable | Threaded via |
|---|---|---|---|---|
| `FuncLib` | `function.rs:185` | `FuncId` / `SubgraphId` | yes (lambdas `#[serde(skip)]`) | `Arc<ArcSwap<FuncLib>>`, shared editor↔worker↔script |
| `CustomValueRegistry` | `value_codec.rs:62` | `TypeId` | no (trait objects) | owned *by value* in `OutputCache` (`output_cache.rs:54`) |
| type metadata | inlined in `DataType::Custom(Arc<TypeDef>)` / `Enum(Arc<EnumDef>)` | the `TypeId` is a buried field | yes (self-describing) | rides in every `DataType` |

There is **no `TypeId → metadata` map**. The id only exists as a field inside an
inline `DataType`, and the codec registry is keyed by `TypeId` while knowing
nothing else about the type. Assembly is duplicated: `runtime_func_lib()` and
`runtime_codec_registry()` (`darkroom/src/core/func_lib.rs:23,40`).

Consequences:

- **`unimplemented!()` in wildcard resolution** (`graph.rs:449`): a wildcard
  output fed by a *const* Enum/FsPath input can't recover its `DataType`, because
  `StaticValue::Enum(String)` / `FsPath(String)` carry only the value.
- **Unsafe enum ids**: `config_node.rs:178` mints enum `TypeId`s as
  `FnvHasher::finish() as u128` of the type name — 64 bits widened to 128,
  collision-prone, and against the `uuidgen`-only rule.

## 2. Goals / non-goals

Goals:

1. One `Library` owning funcs, subgraphs, **and** types (metadata + codec).
2. Every nominal type (`Custom`, `Enum`) has a registered `TypeId`; `DataType`
   holds only that id.
3. Resolve enum/fs-path types through const inputs (kill the `unimplemented!()`).
4. One disciplined path for minting type ids.

Non-goals: changing the execution engine's type-blindness (it still never
type-checks), or persisting the type table to the library file (stays code-built
for now — see §7).

## 3. The `Library`

`FuncLib` → `Library` (it no longer holds only funcs):

```rust
pub struct Library {
    pub funcs:     KeyIndexVec<FuncId, Func>,
    pub subgraphs: KeyIndexVec<SubgraphId, SubgraphDef>,
    pub types:     HashMap<TypeId, TypeEntry>,   // NEW — see §6 for why HashMap
}

pub struct TypeEntry {
    pub decl: TypeDecl,                            // serializable metadata
    #[serde(skip)]
    pub codec: Option<Arc<dyn CustomValueCodec>>, // runtime, re-attached like a lambda
}

pub enum TypeDecl {
    Custom { display_name: String },
    Enum   { display_name: String, variants: Vec<String> },
}
```

`TypeEntry` pairs serializable metadata with a `#[serde(skip)]` codec — exactly
the pattern `Func` already uses for its lambda. `CustomValueRegistry` is deleted;
`registry.register(id, codec)` becomes `library.register_type(id, TypeEntry{..})`.

Lookups:

```rust
impl Library {
    pub fn type_decl(&self, id: TypeId) -> Option<&TypeDecl>;
    pub fn codec(&self, id: TypeId) -> Option<&Arc<dyn CustomValueCodec>>;
    pub fn display_name(&self, ty: &DataType) -> Cow<str>;   // Null/Float/... literal; Custom/Enum via table; unknown → id
    /// Assembly-time check: every TypeId referenced by a func signature resolves.
    pub fn validate(&self);
}
```

`TypeDef`/`EnumDef` as standalone structs go away — their fields fold into
`TypeDecl`. `EnumDef::from_enum::<E>` becomes `TypeDecl::enum_from::<E>` (still
reads strum variant names) used at registration time.

## 4. `DataType` becomes id-only

```rust
pub enum DataType {
    Null, Float, Int, Bool, String,
    FsPath(Arc<FsPathConfig>),   // STAYS inline — structural, not nominal (see note)
    Custom(TypeId),              // was Custom(Arc<TypeDef>)
    Enum(TypeId),                // was Enum(Arc<EnumDef>)
}
```

**FsPath stays inline.** It is *structural config* (mode + per-port extensions),
not a nominal type with a stable identity — two ports with identical configs
aren't "the same type" in any registry sense. Keeping it inline means `Display`,
`compatible_with`, and the editor's FsPath sites (`port_row.rs:367`,
`node/mod.rs:367`, `inspector.rs:382`) are untouched. Only `Custom`/`Enum` —
which genuinely have one identity per id — become references.

### What this simplifies (no registry needed)

- **`PartialEq`** (`data.rs:509-510`): `Custom(a) == Custom(b)` is just `a == b`.
  Already compared by id today; now trivially so.
- **`compatible_with`** (`data.rs:455`): unchanged in spirit (Null wildcard,
  numeric coercion, else exact match) and still registry-free.
- **`port_color.rs:56-57`**: `def.type_id.as_u128()` → `id.as_u128()` directly.
- **`CustomValue::type_def() -> Arc<TypeDef>`** → **`type_id() -> TypeId`**.
  Runtime values only need their id (for codec lookup at `value_codec.rs:139`);
  the display name lives in the registry. Every impl (`Image`, `AstroFrame`,
  `Masters`, `ConfigValue<T>`, test `Opaque`/`Blob`) stops allocating an
  `Arc<TypeDef>` per call and just returns a const id.

### What this costs (now needs the registry)

These sites must take `&Library` (every one already has it in scope or can):

- **`Display for DataType`** (`data.rs:449-450`) — replace with
  `Library::display_name(&ty)`. Callers: graph type-mismatch errors
  (`graph.rs`, has `func_lib`), darkroom port/inspector text (has `AppContext`).
- **`DataType::default_value`** (`data.rs:424`) — Enum branch reads `variants[0]`
  → `default_value(&self, lib: &Library)`. Caller: `enum_input`
  (`config_node.rs:72`).
- **`const_satisfies`** (`graph.rs:108`) — Enum variant-membership check reads
  `def.variants` → thread `func_lib` (already present in `check_with`).
- **`value_editor.rs:120`** — enum dropdown reads `def.variants` →
  `ctx.func_lib.type_decl(id)` (`AppContext` already carries `&FuncLib`).

### Main tradeoff of the purist choice

A serialized document is **no longer self-describing about types** — it stores
ids, and rendering/validation needs those ids present in the runtime `Library`.
For our code-built library (all builtin types always registered) this is fine and
mirrors how a document already depends on `FuncId`s existing. The regression: a
doc using a type from an *unloaded* crate loses its display name. `display_name`
must therefore fall back to the raw id string for an unknown `TypeId` rather than
panic.

## 5. Type inference through const inputs

Two complementary fixes remove the `unimplemented!()` (`graph.rs:449`):

1. **Prefer the mirrored input's declared type.** In `resolve_output_type_inner`,
   when the mirrored input is `Const` and its declared `data_type` is not `Null`,
   return that declared type — it already carries the full `FsPathConfig` (for
   FsPath) or the `Enum(id)`. This alone fixes every const on a *typed* port,
   which is the normal case, and fully covers FsPath (no registry needed).

2. **Tag `StaticValue::Enum` with its id** for the residual case — an enum const
   wired into a *wildcard* (`Null`) input, where the declared type tells us
   nothing:

   ```rust
   StaticValue::Enum { type_id: TypeId, variant: String }   // was Enum(String)
   ```

   Resolution then returns `DataType::Enum(type_id)` directly. FsPath needs no
   such tag — a const path on a bare `Null` input genuinely has no config to
   recover, and rule (1) handles every real FsPath port.

`StaticValue::Enum` gaining a field touches its `PartialEq`/`Display`
(`data.rs:144,214`), `as_enum` (`data.rs:190`), and the `static_value`/
`field_value` mappers in `config_node.rs` — all mechanical.

## 6. Why `HashMap` for `types`, but `KeyIndexVec` for funcs/subgraphs

`KeyIndexVec` = `Vec` (ordered, stable iteration + deterministic serde) +
`HashMap` (O(1) by-key). It earns that second half only when *ordered iteration*
matters:

- **funcs** — iterated for the new-node menu and category list
  (`new_node_ui.rs:139`, `:198`) and scanned in order by `by_name`. Order is
  user-visible. Keep `KeyIndexVec`.
- **subgraphs** — same: listed in UI, serialized to the library file (stable
  order = clean diffs). Keep `KeyIndexVec`.
- **types** — never iterated anywhere; pure id lookup (codec dispatch, metadata
  resolution). No menu, no ordered serde artifact (code-built each run). The
  ordered `Vec` half buys nothing, so a plain `HashMap<TypeId, TypeEntry>` is the
  honest fit.

Upgrade path: if we ever serialize the type table to the library file and want
clean diffs, switch `types` to `KeyIndexVec` (insertion order — `TypeId` is a
random UUID, so a `BTreeMap`'s sort-by-id order would be meaningless).

## 7. Type-id minting

Registration is the choke point. Two id sources:

- **Static types** (`Image`, `Masters`, `BlendMode`, `ConversionFormat`, config
  values) — `uuidgen` literal consts, as today. The `LazyLock<Arc<TypeDef>>`
  statics (`image/mod.rs:51`, `masters.rs:14`) collapse to `const … : TypeId`.
- **Dynamically-discovered enums** (`config_node.rs:123`, from `Introspect`
  field reflection) — replace `stable_type_id` (FnvHash-as-u128) with a
  **UUIDv5** over a fixed namespace + type name: deterministic, full 128-bit,
  collision-resistant. (Alternative: have `#[derive(IntrospectEnum)]` carry a
  `const TYPE_ID` so even these are literals — cleaner but touches the derive
  macro and every mirror enum; defer.)

`Library::register_type` panics on a duplicate id (as the codec registry does
today), so a collision is caught at assembly, not at runtime.

## 8. Codec at execution time

The executor/`OutputCache` needs codecs but not the rest of the library (flatten
already copies lambdas into the program; `func_lib` isn't held past flatten). So
`OutputCache` stops owning a registry and instead holds an `Arc<Library>` (or a
small `Arc<CodecTable>` sliced at worker-build time) for codec lookup in
`serialize_outputs`/`deserialize_outputs` (`value_codec.rs:128,158`,
`blob.rs:49`). This keeps "the library is a compile-time input" mostly intact
while collapsing assembly into one object.

## 9. Migration phases (strictly ordered)

1. **Add the type table, keep `DataType` inline.** Introduce `Library`/
   `TypeEntry`/`TypeDecl`, register all existing types + codecs into it, delete
   `CustomValueRegistry`, repoint `OutputCache`. `DataType` still carries Arcs.
   Compiles + tests green at this checkpoint. *(consolidation done)*
2. **Flip `CustomValue::type_def` → `type_id`.** Update all impls.
3. **Flip `DataType::Custom/Enum` to id-only.** Update `PartialEq`, delete
   `Display for DataType` (→ `Library::display_name`), move `default_value`/
   `const_satisfies` to take `&Library`, fix `port_color`, `value_editor`.
4. **Inference fix.** `StaticValue::Enum { type_id, variant }`, declared-type
   rule + registry lookup in `resolve_output_type_inner`; remove the
   `unimplemented!()`.
5. **Id discipline.** UUIDv5 for dynamic enums; collapse the `LazyLock` type
   statics to `const TypeId`.
6. **Sweep:** `cargo test && cargo fmt --all && cargo clippy --all-targets -D warnings`.

## 10. Touched files (inventory)

- `scenarium/src/function.rs` — `FuncLib`→`Library` + `types`/`register_type`/
  `type_decl`/`codec`/`display_name`/`validate`.
- `scenarium/src/data.rs` — `DataType` variants id-only; `TypeDef`/`EnumDef`→
  `TypeDecl`; `CustomValue::type_id`; `StaticValue::Enum` struct variant;
  `PartialEq`/`Display`/`default_value` (`data.rs:109,240,400,416,449,509`).
- `scenarium/src/value_codec.rs` — delete `CustomValueRegistry`; codec via
  `Library`; `value.type_id()` (`:139`).
- `scenarium/src/execution/{output_cache.rs,blob.rs}` — `OutputCache` holds
  `Arc<Library>`/codec view; test `CustomValue` impls.
- `scenarium/src/graph.rs` — `const_satisfies` + resolve take `&Library`;
  inference fix (`:96,108,449`).
- `scenarium/src/elements/cache_passthrough.rs` — unaffected (FsPath stays).
- `scenarium/src/lib.rs` (prelude) — drop `CustomValueRegistry`, export
  `Library`/`TypeDecl`/`TypeEntry`.
- `lens/src/image/{mod.rs,codec.rs}` — `IMAGE_TYPE_ID` const; register type+codec
  in `image_library()`; drop `register_image_codec`.
- `lens/src/astro/masters.rs` — `MASTERS_TYPE_ID` const; register in funclib.
- `lens/src/config_node.rs` — `config_data_type` → `Custom(id)`; register custom
  + dynamic-enum types; UUIDv5 ids (`:43,63,123,178`).
- `lens/src/lib.rs` — surface changes.
- `darkroom/src/core/func_lib.rs` — fold type+codec registration into
  `runtime_func_lib()`; delete `runtime_codec_registry()`.
- `darkroom/src/core/engine.rs` — cache borrows codecs from the shared library.
- `darkroom/src/gui/node/{value_editor.rs,port_color.rs}` — enum variants via
  registry; color via raw id.

## 11. Open questions

1. ~~`Library` rename vs. keeping `FuncLib`~~ — **resolved**: renamed to `Library`,
   moved to `library.rs`.
2. ~~Codec sharing into `OutputCache`~~ — **resolved**: holds `Arc<Library>`.
3. Dynamic-enum ids: UUIDv5-from-name now vs. derive-carried `const TYPE_ID`
   later. (phase 5)
