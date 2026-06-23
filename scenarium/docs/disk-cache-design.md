# Disk-backed node output cache — design notes

Status: investigation / proposal. Not yet implemented.

Motivation: today a node's "cache the result" option keeps outputs only in RAM;
reopening a project drops them. Expensive deterministic work (e.g. stacking light
frames) is recomputed every session. We want an option to persist outputs to disk
so an unchanged graph reloads its results instead of recomputing, and so results
can travel with a project to another machine — without ever serving a stale value.

## How caching works today

- The "C" badge sets `Node.behavior = NodeBehavior::Once` (`scenarium/src/graph.rs:102`).
  At flatten time this becomes `ExecutionBehavior::Once`; otherwise functions inherit
  `Pure`/`Impure` from `FuncBehavior`.
- The cache is `RuntimeSlot.output_values: Option<Vec<DynamicValue>>`, held in
  `Executor.slots`, keyed by the flattened `NodeId`
  (`scenarium/src/execution/executor.rs:33`). It lives only in the live `Executor`
  and is never serialized.
- Invalidation is **session-relative**: `input_dirty` bits are set when a binding
  changes during `update`; the planner decides `cached` vs `wants_execute` from those
  bits plus whether a slot already holds outputs (`scenarium/src/execution/planner.rs:260`).
  There is no persisted notion of *what produced* a value.
- What gets saved is only topology + scalar params + editor view
  (`darkroom/src/core/io/persistence.rs`, `Document`). `DynamicValue` is deliberately
  **not** `Serialize`, and heavy payloads (`Image`, stacked frames) ride inside
  `DynamicValue::Custom(Arc<dyn CustomValue>)`, a trait with no serialization
  (`scenarium/src/data.rs:36,224`).
- No content-hash infrastructure exists. There is only a 64-bit FNV used for
  flatten-id stability (`common/src/fnv.rs`, `scenarium/src/execution/flatten.rs:172`).

So the gap is clean: no on-disk value store, no content addressing.

## Key reframe: a disk cache is a *build cache*

Session dirty bits work today only because the live `Executor` remembers what it last
ran. Once a cache must survive reopen and travel between machines, that memory is gone.
This is the ccache / Bazel / Nix / Salsa problem, and the robust answer is
**content-addressed caching**.

A node's output is a pure function of:

1. the **function identity + a version stamp** (the algorithm),
2. the **resolved parameter values** for its inputs,
3. the **content digests of its upstream producers** (recursively),
4. the **content of any external files** it reads.

Hash all of that into a `digest`, store the output blob at `cache/<digest>`. The digest
*is* the invalidation.

```
digest(node, port) = blake3(
   "scenarium-cache-v1",
   func_id, func_version, port_idx,
   for each input in order:
     None      -> H("none", default)
     Const(sv) -> H("const", sv, file_content_digest if FsPath)
     Bind(out) -> H("bind", digest(out.node, out.port))   // recursive, memoized
)
```

A DAG fold (cycles already rejected by the planner), memoized per node. `StaticValue`
is `Serialize`, so hash its canonical bytes (e.g. bitcode) rather than relying on `Hash`
(floats). Use `blake3` (256-bit) for the store — FNV's 64 bits is fine for ids but too
narrow for a content-addressed store with many entries.

## The three hard questions

### 1. Saving outputs for many nodes

The hardest *engineering* part: `DynamicValue::Custom` is not serializable by design.
Give `CustomValue` an optional disk codec:

```rust
fn cache_blob(&self) -> Option<Vec<u8>>;        // None = not disk-cacheable
// plus a TypeId/TypeDef-keyed registry: fn(&[u8]) -> Arc<dyn CustomValue>
```

Only opt-in types are persistable; everything else silently falls back to recompute.
For stacked lights this is a *good* story: the output is an `Image`, so the codec reuses
imaginarium's encoders — write lossless float EXR. The user gets the stacked result as a
real file, and content-addressing dedups it. Each output port gets its own blob at
`H(node_digest, port_idx)`. An authoring-`NodeId` → digest manifest is optional UI sugar.

### 2. Invalidating them

You don't, explicitly — the digest does it. Change a param, swap an upstream node, edit
a feeding file → every downstream digest changes → old blobs are never looked up again.
Two correctness traps:

- **Impure nodes must never be disk-cached.** Gate on the existing `FuncBehavior::Pure`.
- **Function versioning.** If the stacking algorithm improves, last week's blob is now
  *wrong*. Add an explicit `version: u64` to `Func` and fold it into the digest; bumping
  it is a universal cache miss. Without this, two machines on different binaries silently
  swap stale results. This is the subtle one.

### 3. Cross-machine: open on A, change values, send project to C which holds cached values

Content-addressing handles this with no special logic:

- A changes an upstream param → every downstream digest changes.
- Ship the project to C, which still has blobs keyed by the *old* digests.
- C recomputes digests from the received project → no match on disk → **cache miss →
  recompute.** Correct, automatically.
- Stale blobs on C are dead weight, GC'd later. There is **no path** to serving a wrong
  value.

Contrast the naive scheme — key by `NodeId` + a monotonic "dirty version" counter. That
breaks across machines: counters aren't comparable between machines and don't encode
*what* changed.

**Shared-files trap (same family).** A stacking node reads raw frames; if C has
*different* files at the same path, the digest must include file content (or a
`(size, mtime)` proxy), or C serves A's stack for C's frames. File reads happen inside
node functions, so hook this at `DataType::FsPath` inputs: when hashing an `FsPath` value,
fold in the file's content digest (`mtime + size` by default, full-hash opt-in).

## Two design decisions

### A. Orthogonal persistence flag, not a third `NodeBehavior` variant

"Cache to disk" is an orthogonal *persistence* axis, not a sibling of
`AsFunction`/`Once`:

| mode | recompute on input change | persist across sessions |
|---|---|---|
| `Pure` + memory (today) | yes | no |
| `Pure` + **disk** | yes | **yes** ← stacked lights |
| `Once` + disk | no (compute once ever) | yes |

Stacked lights is "`Pure` + disk": add a frame and you *want* recompute, but reopening
unchanged should hit disk. Model it as a separate `persist: CachePersistence::{Memory, Disk}`
flag on `Node`, gated to `Pure`/`Once`, rather than overloading `NodeBehavior`. The "C"
badge grows a third visual state.

### B. Where blobs live

Offer both, made interchangeable by content addressing:

- A machine-global CAS (`~/.cache/scenarium/<digest>`) for fast local reopen, with an
  LRU size cap + GC of unreferenced digests.
- An exportable **sidecar bundle** (`project.cache/` next to the `.rhai`), so "send
  project + results to another machine" is just zipping a folder. C drops them into its
  CAS; matching digests hit, the rest are inert.

## Recommended scope

Both need the same two foundations first: the `CustomValue` serialization codec (+ type
registry) and a `blake3`-based content-digest pass (add the dep).

- **Explicit "Checkpoint" node (MVP).** User inserts a node that persists its input to a
  named sidecar `{digest, blob}`. On open, recompute the upstream digest, compare to the
  header, load or recompute. Explicit, no GC surprises, forces the codec work that *any*
  version needs.
- **Transparent `persist: Disk` flag (target).** The matrix above, content-addressed CAS,
  full digest pass. Better UX; needs GC and function-versioning discipline.

Build the two foundations and the checkpoint MVP and the transparent flag are mostly the
same machinery wearing different UI.

## Sequenced implementation plan (target design)

Eight phases, dependency-ordered. Each phase compiles, passes
`cargo test && cargo fmt --all && cargo check && cargo clippy --all-targets -- -D warnings`,
and ships behind no behavior change until Phase 6 wires it in. Phases 1–3 are the
foundations and are mutually independent (parallelizable); 4 is independent plumbing;
5 builds on 1+3; 6 is the integration that needs everything; 7–8 are exposure/UX.

Dependency graph:

```
0 deps+version ─┬─► 1 digest ───────┐
                ├─► 2 codec ──┐      ├─► 6 integration ─► 7 sidecar/GC ─► 8 UI
                │             ├─► 5 store + load/store path
                └─► 3 store ──┘
4 persist-field plumbing ────────────┘
```

### Phase 0 — deps & function version stamp
- Add `blake3` to the workspace (`Cargo.toml`).
- Add `Func.version: u64` with `#[serde(default)]`; thread default `0` through
  `Func::new` / the builder — `scenarium/src/function.rs`.
- *Test:* `Func` serde round-trip carries `version`; default applies to old documents.
- Pure-additive; nothing reads `version` yet.

### Phase 1 — content-digest pass *(foundation A; depends on 0)*
- New module `scenarium/src/execution/digest.rs`: per-`(node, port)` blake3 digest over
  the **flattened** `ExecutionProgram` — folds `func_id`, `func_version`, `port_idx`, and
  per-input contributions (`None`→default, `Const`→canonical `StaticValue` bytes via
  bitcode, `Bind`→recursive upstream digest). Memoize per node over the DAG (cycles
  already rejected by the planner).
- `FsPath` file identity: when an input value is `StaticValue::FsPath`, fold in
  `(size, mtime)` by default (full content-hash opt-in). Inject the stat fn for testing.
- *Test (pure, no execution):* identical graph → identical digest; changing a `Const`,
  `func_version`, or an upstream node propagates to all downstream digests; two structurally
  identical subgraphs produce the same digest (dedup); `FsPath` mtime change flips the digest.

### Phase 2 — `CustomValue` disk codec + type registry *(foundation B; depends on 0)*
- Extend the `CustomValue` trait — `scenarium/src/data.rs`:
  `fn cache_blob(&self) -> Option<Vec<u8>>` (`None` = not disk-cacheable) plus a
  `TypeDef`/`TypeId`-keyed reconstruct registry `fn(&[u8]) -> Arc<dyn CustomValue>`.
- `serialize_outputs(&[DynamicValue]) -> Option<Vec<u8>>` / `deserialize_outputs(..)`:
  handle `Static` directly, `Custom` via the codec; return `None` if **any** value is
  non-cacheable (caller falls back to recompute).
- Implement the codec for `Image` via imaginarium encoders (lossless float EXR).
- *Test:* `Image` round-trips `cache_blob` → reconstruct pixel-equal; mixed `Static` vec
  round-trips; a non-cacheable custom type yields `None`.

### Phase 3 — content-addressed store *(depends on 0)*
- New store abstraction (`scenarium/src/execution/cache_store.rs` or a small sibling
  crate): `get(digest) -> Option<Vec<u8>>`, `put(digest, bytes)`, blake3-addressed.
  Backed by a global CAS dir (`~/.cache/scenarium/<digest>`).
- *Test (temp dir, dummy bytes):* put/get hit, miss on unknown digest, idempotent put.
- GC/LRU and the exportable sidecar are deferred to Phase 7 — keep the API ready for them.

### Phase 4 — `Node.persist` data-model plumbing *(independent)*
- Add `persist: CachePersistence::{Memory, Disk}` to `Node` (`#[serde(default)]` →
  `Memory`) — `scenarium/src/graph.rs`. Validation rejects/clamps `Disk` on `Impure`
  effective behavior.
- Carry `persist` through flatten into `ExecutionNode` (same path `behavior` takes today)
  — `scenarium/src/execution/flatten.rs`, `scenarium/src/execution/program.rs`.
- *Test:* serde round-trip; flatten propagates `persist` to execution nodes; `Impure + Disk`
  is rejected/clamped by `validate_with`.

### Phase 5 — disk load/store path *(depends on 1, 2, 3)*
- Glue layer that, given a flattened node, computes its digest (1), and on hit
  deserializes (2) the bytes from the store (3) into a `Vec<DynamicValue>`; on store, the
  reverse. No scheduler changes yet — unit-testable in isolation.
- *Test:* run-free round-trip — synthesize outputs, store under digest, reload identical;
  digest mismatch ⇒ miss.

### Phase 6 — planner/executor integration *(depends on 4, 5)*
- Planner: for a `persist = Disk` node whose `RuntimeSlot.output_values` is empty, look up
  the disk path before scheduling; on hit, populate the slot and mark it `cached`
  (RAM stays L1, disk is L2) — `scenarium/src/execution/planner.rs`.
- Executor: after a `persist = Disk` node runs, serialize and `put` its outputs under the
  digest — `scenarium/src/execution/executor.rs`.
- *Test (end-to-end):* run a graph, drop the `Executor` (simulate reopen), rebuild against
  the same store ⇒ persist node loads from disk + reports `cached`, non-persist recomputes;
  mutate a param ⇒ digest miss ⇒ recompute; `Impure` never persists. Cross-machine sim:
  build the store, point a fresh `Executor` with a *mutated* graph at it ⇒ miss, no stale
  serve.

### Phase 7 — sidecar bundle + GC
- Exportable sidecar (`project.cache/` next to the document) that imports into / exports
  from the global CAS, so results travel with a zipped project. LRU size cap + GC of
  unreferenced digests.
- *Test:* export → fresh CAS import → hit; GC evicts past the size cap, keeps referenced.

### Phase 8 — editor UX *(depends on 6)*
- Third "C" badge state + a `SetCachePersistence` intent (or extend `SetCacheBehavior`) —
  `darkroom/src/gui/node/header.rs`, `darkroom/src/gui/scene.rs`, intent handling.
- GUI: manual verification (no unit tests per the GUI-code rule).
