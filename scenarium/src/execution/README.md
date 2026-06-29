# `execution` — design notes

The compile→plan→execute pipeline and its caches. This README is the long-form
design home that the module's `//!` docs point at. Three parts:

- **A. Subgraphs** — how composite nodes are authored and flattened away
  (`flatten.rs`; authoring types in `subgraph.rs`/`graph.rs`).
- **B. Disk cache** — the content-addressed output cache for `persist = Disk` nodes
  (`digest.rs`, `cache.rs`, `output_cache.rs`, `blob.rs`, `value_codec.rs`).
- **C. File cache** — the explicit-path `file cache` passthrough node
  (`elements/cache_passthrough.rs`, `special.rs`).

Parts B and C are two policies over one storage primitive (`blob.rs`); they differ
only in how a node maps to a file and whether a present blob is rewritten.

---

# Part A — Subgraphs (composite nodes)

How a reusable composite — a sub-graph packaged as a single node — is authored,
referenced, and lowered to flat execution nodes.

## A.1 The problem

A user wants to package a chunk of a graph (several wired nodes) into one reusable
node with a clean interface, instance it many times, and nest instances. The runtime
must not pay for any of that abstraction: scheduling, caching, dead-branch pruning,
and cycle detection all operate on a flat func-only graph. So composites exist **only
in the authoring model** and are dissolved before execution.

## A.2 Two representations

- **Authoring graph** (`Graph`, `graph.rs`) — what the editor edits. Holds composite
  instances (`NodeKind::Subgraph`), boundary nodes, and nested definitions.
  Serializable.
- **Execution program** (`ExecutionProgram`, `program.rs`) — the compiled, flattened,
  immutable form. No composites, no boundaries: flat func nodes only.

The compile step (§A.5) lowers the first into the second. Everything below the
compile boundary is composite-agnostic.

## A.3 Authoring representation

A **`SubgraphDef`** (`subgraph.rs`) is a reusable definition: an interior `Graph` plus
the interface it exposes.

```
SubgraphDef {
    id, name, category,
    graph:   Graph,            // the interior
    inputs:  Vec<FuncInput>,   // exposed inputs  (port order)
    outputs: Vec<FuncOutput>,  // exposed outputs (port order)
    events:  Vec<SubgraphEvent>,
    origin:  Option<SubgraphId>,
}
```

A **composite instance** is a `Node` whose `kind` is `NodeKind::Subgraph(SubgraphRef)`.
Its port/event arity is shaped from the referenced def's interface, exactly as a func
node is shaped from a `Func` — see `Node::subgraph_instance`.

## A.4 Definitions and references

### §4.1 The interface is a function signature

A composite's interface is structurally a function signature, so it reuses the func
types: exposed inputs are `FuncInput` (name / type / required / default), exposed
outputs are `FuncOutput` (name / type). An output carries no `required`/`default`,
which is exactly the func distinction — reusing the types keeps that asymmetry and
lets an instance node be shaped from a def like a func node is shaped from a `Func`.

Behavior (`Pure`/`Impure`) and terminal-ness are **derived from the interior at
flatten time, never stored on the def** — so they can't drift from the interior.

### §4.2 Boundary nodes

Two node kinds may appear *only* inside a `SubgraphDef.graph`:

- `NodeKind::SubgraphInput` — the inbound boundary. Its **outputs** are the def's
  exposed inputs: interior node bound to `SubgraphInput` output port *i* reads exposed
  input *i*.
- `NodeKind::SubgraphOutput` — the outbound boundary. Its **inputs** are the def's
  exposed outputs: exposed output *j* is whatever feeds `SubgraphOutput` input *j*.

Their port arity comes from the enclosing def's interface, not from a func. Each is
**optional**: a def that exposes no inputs omits `SubgraphInput`; one that exposes no
outputs omits `SubgraphOutput`. Boundaries are routing only — they emit no execution
node.

### §4.3 Instancing a definition

`Node::subgraph_instance(def, ref)` builds an instance node whose interface mirrors
`def`. The instance is wired in its parent graph like any node: its input ports take
bindings, its exposed-event ports take subscribers.

### §4.4 Local vs. shared definitions

A reference resolves a def from one of two places, and *that* is the only difference
between a private and a shared composite:

- **Local** — the def lives in the owning `Graph.subgraphs`
  (`KeyIndexVec<SubgraphId, SubgraphDef>`, `graph.rs`). Private to this graph; editing
  it affects only this graph; it travels with the document on serialize.
- **Shared (linked)** — the def lives in `Library.subgraphs` (`library.rs`). One def,
  many instances across many documents; editing it propagates to **every** linked
  instance.

Resolution is `Graph::resolve_def(ref, library)`: `Local` → `Graph.subgraphs`,
`Linked` → `Library.subgraphs`.

### §4.5 References (`SubgraphRef`) and exposed events (`SubgraphEvent`)

`SubgraphRef` *is* the local/shared distinction — the variant carries the
`SubgraphId` and says where to resolve it:

```
enum SubgraphRef { Linked(SubgraphId), Local(SubgraphId) }
```

**Exposed events are always outgoing.** A `SubgraphEvent` re-exports an interior
emitter outward so a parent node can subscribe to the composite:

```
SubgraphEvent { name, emitter: NodeId, emitter_event_idx: usize }
```

`emitter` names an interior node in `SubgraphDef.graph` and which of its events. An
instance node carries one event port per `SubgraphEvent`, holding the parent's
subscribers — exactly like a func node's events.

There is **no incoming-event interface element**. Routing an event *into* a composite
is just the ordinary subscriber mechanism: the composite node, like any node, can be
subscribed to a parent event; which interior subnodes then fire is the subgraph's
internal wiring — interior nodes subscribing to the def's `SubgraphInput` node (its
"trigger"), not an exposed port. Flatten resolves both directions across the boundary
(§A.5).

### §4.6 Copying, localizing, lineage

`SubgraphDef::fresh_copy()` produces an independent copy with a fresh def id and fresh
interior node ids (exposed-event emitters remapped to match), preserving the
interface. It's used to *localize* a `Linked` instance (turn a shared def into a
private one) or to make an instance's identity unique.

`origin: Option<SubgraphId>` is lineage metadata only — "this local def was copied
from that library def." Editors use it; the runtime ignores it.

### §4.7 Validation and the recursion guard

`Graph::validate_with(library)` recurses per composite level. A def that (directly or
transitively) instances itself is rejected — the flatten pass tracks the set of defs
on the current descent path (`seen`) and asserts a def never appears twice
(`flatten.rs`), and validation surfaces it as an error rather than looping. A hard
depth cap (`MAX_DEPTH`) backstops the output-resolution walks the `seen` guard doesn't
cover.

## A.5 Flattening (`flatten.rs`)

Compile expands every composite into its interior func nodes and writes them straight
into the execution program — **no intermediate `Graph` is materialized**. Boundary
nodes and composites dissolve; their edges are short-circuited so the result is a
flat, func-only graph the scheduler handles unchanged.

### §5.1 The descent walk

`Flattener::build` walks the root graph; `emit` iterates a level's nodes:

- a **func** or **special** node emits one flat execution node (both resolve to a
  `&Func` spec — `library.by_id` vs the hardcoded `SpecialNode::func` — so the emit
  body is shared);
- a **subgraph** node descends: push the instance id onto the descent `path`, recurse
  into the resolved def's interior, pop;
- **boundary** and **disabled** nodes emit nothing.

The `Flattener`'s only persistent state is the descent `path`; the current graph at
each level is re-derived from it on demand (`graph_at`), which keeps the scratch free
of borrowed references so it can be reused across builds.

### §5.2 Stable flattened identity (`flatten_id`)

Each emitted node gets a deterministic flat `NodeId`:

- **Top-level** nodes (empty descent path) keep their **own id verbatim** — so a
  func-only graph maps to itself and per-node caches survive edits elsewhere.
- **Nested** nodes get `hash(descent-path ids, interior id)`. This is stable across
  builds, distinct from the bare interior id, and sensitive to both the path and the
  interior id — so two instances of one composite, or two interior nodes, never
  collide on a flat id (and thus never share a cache slot).

### §5.3 Resolving data bindings across boundaries

When an interior input binds to an output, `resolve` follows the wire to a concrete
flat producer:

- a func/special producer → `Source::Producer { flatten_id(path, node), port }`;
- a **subgraph** producer → follow into its `SubgraphOutput` input `port`, one level
  **down**;
- a **`SubgraphInput`** producer → the enclosing instance's exposed input `port`, one
  level **up** (pop the instance off the path, resolve in the parent, push it back);
- a **disabled** producer → `Source::None` (the wire reads unbound downstream).

So a binding that crosses any number of boundaries lands on the real flat producer,
and the execution program contains only func-to-func edges.

### §5.4 Resolving event edges across boundaries

Event subscriptions are resolved into flat `(emitter, event_idx, subscriber)` edges
during the walk and applied after the node pass (when every flat node exists and is
addressable by key):

- `resolve_emitter` follows a composite's `SubgraphEvent` mapping **inward** to the
  concrete interior func event it ultimately fires;
- `resolve_subscriber` expands a composite subscriber **into** the interior nodes
  wired to its `SubgraphInput` trigger — those are the nodes that actually run when the
  composite is triggered.

Subscriptions emitted *by* a `SubgraphInput` (the trigger) are consumed when the
enclosing instance is resolved as a subscriber, so they aren't emitted as standalone
edges.

### §5.5 Caching survives across the boundary

Because top-level node ids are preserved (§5.2) and a nested node's flat id is a pure
function of its descent path + interior id, an edit that doesn't touch a given node
leaves its flat id — and therefore its content digest and cached output — unchanged.
Editing a shared (linked) def re-flattens every instance, and each instance's interior
nodes keep their per-instance flat ids, so unaffected instances keep their caches.
Disk and RAM caching then work across composite boundaries with no special handling
(Part B).

---

# Part B — Disk (content-addressed output) cache

How a `persist = Disk` node's outputs survive a reload — staying correct (never serve
a stale value) and lean (never hold bytes a run won't use). The **explicit-path**
cache (Part C) shares the same storage primitive but keys on a path.

## B.1 What opts in

A node opts in with `CachePersistence::Disk` (`graph.rs`), surfaced on the flat node
as `ExecutionNode.persist`. `Disk` is a **request**, honored only when:

1. The node has a **content digest** — its whole upstream cone is reproducible (§B.2).
   An impure node, or anything downstream of one, has no digest and is silently kept
   memory-only — it can never risk serving a stale value.
2. A **disk root** is configured (`ExecutionEngine::set_output_cache` with a
   `disk_root`). Until then every node is memory-only.

## B.2 The content digest is the cache key (`digest.rs`)

A node's output is a pure function of: its func (`func_id` + `func_version`), its
resolved input values, the outputs of its upstream producers, and the content of any
external files it reads. `DigestEngine` folds exactly that into a 256-bit BLAKE3
`Digest`, memoized per node over the flattened program.

- **Equal digest ⇒ identical computation**, on any machine — so the digest is at once
  the cache *key* and the *invalidation* signal. Change any const, binding, func
  version, or upstream output and every downstream digest changes.
- **Reproducibility taint.** Only `Pure` nodes are hashed; an `Impure` node (or any
  node with a non-reproducible producer) has digest `None` = "always recompute, never
  cache", for RAM and disk alike. The `NodeDigest` memo distinguishes
  `Pending`/`NotCacheable`/`Digest(d)`.
- **File inputs.** An `FsPath` const folds the path string *and* the file's identity
  (`FileId`, default `(len, mtime)`; opt-in content hash) so the same path holding
  different bytes invalidates.
- **Output ports** of a multi-output node are disambiguated (`port_digest`); a `DOMAIN`
  separator versions the hashing scheme itself.

Digests are **compile-stable**, so the engine recomputes them once per `update`
(`Cache::recompute_digests`) and the planner/run read the stored value.

## B.3 Storage (`blob.rs`)

A node's blob lives at `<disk_root>/<hex(digest)>` — so identical computations
**dedup** across nodes, documents, and machines, and any change re-keys to a new path
(auto-invalidation; the stale blob is simply never read again). Writes are **atomic**
(temp file + rename), so a reader never sees a half-written blob. A content-addressed
blob already on disk is the same bytes, so the store skips re-serializing it.

## B.4 Values ↔ bytes (`value_codec.rs`)

`DynamicValue` isn't `Serialize`: `Unbound`/`Static` are trivial, but
`Custom(Arc<dyn CustomValue>)` is an opaque runtime payload. Each custom *type*
registers a `CustomValueCodec` on its `Library` type-table entry; that one entry drives
`encode` (async + context-aware, so a GPU-resident value can read back) and `decode`
(bytes + type id → value). The envelope (`CachedValue`) carries each custom value's
`type_id` so the loader picks the right codec. **Caching is all-or-nothing per node:** a
custom output whose type has no codec makes the whole node uncacheable, so a reload
never yields a half-real output set.

## B.5 The RAM tier and the availability predicate (`cache.rs`)

Each node has a `RuntimeSlot` (index-aligned to `e_nodes`):

```
output_values:  Option<Vec<DynamicValue>>   // resident value, if any
current_digest: Option<Digest>              // recomputed at update
output_digest:  Option<Digest>              // digest output_values were produced under
disk_available: bool                        // a decodable blob exists for current_digest
```

Two predicates, deliberately separated:

- **`is_resident_hit`** — value in RAM *and* produced under the current digest. The
  executor's input read and the disk store rely on this being the true "bytes are here"
  test.
- **`is_available`** = `is_resident_hit || disk_available` — what the **planner** prunes
  on: a node servable from *either* tier need not execute.

Separating them is the point: the planner prunes knowing only that a blob *exists* (a
cheap `stat`, no bytes), so a disk-cached value behind another disk-cached value is
pruned without ever entering RAM.

## B.6 Lazy lifecycle (`output_cache.rs`)

The engine drives a fixed sequence; the ordering *is* the contract.

1. **`update` → `mark_available`.** For each cacheable node, flag `disk_available` iff a
   blob exists for the current digest **and** the outputs are decodable (every custom
   output type has a codec — predicted without reading). A `stat` + a codec check, **no
   bytes**. The planner can now prune disk-cached cones.
2. **`execute` → plan, then `hydrate_frontier` (looped).** Read into RAM only the
   **frontier** — the cached producers an executing node's inputs `Bind` to. Producers
   *behind* a pruned producer are never referenced, so a chain loads only its frontier.
   A frontier blob that fails to load (corrupt/deleted/undecodable) clears its own
   `disk_available` and makes `hydrate_frontier` return `false`; the engine **re-plans**,
   rescheduling that node to recompute instead of pruning it behind an absent value
   (which would trip the executor's "value present" invariant). Each failure clears one
   flag, so the plan/hydrate loop converges.
3. **run → `store`.** Write executed nodes' outputs (blobs already present are skipped).
4. **after store → `evict_unused`.** Demote resident values the run neither executed nor
   read as a frontier — prior-run leftovers — back to disk-only, **iff** a blob can serve
   them again. Lossless (a later run or inspection reloads); a value with no blob
   (Memory-only, impure) is kept, so eviction never forces a recompute. Runs after
   `store` so a value computed this run is on disk before its RAM copy is reclaimed.

**Off-run path — inspection.** `get_argument_values_with_previews` loads on demand via
`hydrate_for_inspection`, so a disk-cached node no run touched still shows its value when
the editor selects it.

### Worked example

`A → B → terminal`, `A` and `B` both `persist = Disk`, after a cold run that stored
both: `update` flags both available; the plan finds `B` available → prunes its cone
(`A`); the terminal executes and reads `B`, so `B` is the frontier and `A` is behind it;
`hydrate_frontier` reads **only** `B`. `A`'s bytes never enter RAM — the win. If a later
edit invalidates `B`, the next plan schedules `B` to recompute, `A` becomes the frontier,
and `A` hydrates from disk then.

### Invalidation summary

- Any upstream change re-keys the digest → miss → recompute → store under the new key.
- `func_version` bump invalidates a func's cached outputs across binaries.
- Impure cone → no digest → never cached.
- Missing codec / corrupt / deleted blob → treated as a miss, not a failure
  (`mark_available`'s codec check + `hydrate_frontier`'s re-plan keep it off the hot and
  the panic paths).

---

# Part C — File (explicit-path passthrough) cache

The `file cache` node lets a user cache an arbitrary value to a **named file** and skip
recomputing its upstream while that file exists — a manual, path-keyed cache distinct
from the automatic content-addressed one (Part B).

## C.1 The node (`elements/cache_passthrough.rs`, `special.rs`)

It's a **special node** — identified by *kind* (`SpecialNode::CachePassthrough { bypass }`),
not a registered `FuncId`. Its hardcoded interface + lambda come from
`cache_passthrough_func()`:

- `input[0]` **value** — required, the value to cache; the output is a **wildcard**
  mirroring it, so any type passes through and the editor shows the concrete wired type.
- `input[1]` **path** — a **const-only** `FsPath` (a wired binding would silently disable
  caching). Its index is `CACHE_PATH_INPUT`.
- The lambda is a plain passthrough: `output[0] = input[0]`. The **engine** does the
  path-keyed file I/O around it.
- The func is `uncacheable()` — it owns its caching, so the editor's generic
  `persist = Disk` toggle doesn't apply.

## C.2 The path is the key (`file_cache_digest`)

A cache node's content digest is a hash of its `Const` `FsPath` **alone** (domain
`scenarium-filecache-v1`), deliberately ignoring `input[0]`'s cone. So:

- `input[0]` may be impure or expensive and the node still presents a digest — the path
  *is* the reproducibility boundary.
- **Presence, not content, decides the hit:** the file's `(len, mtime)` is *not* folded
  in. While the file exists, the value is served from it and `input[0]`'s upstream is
  pruned. A non-const or empty path ⇒ no digest ⇒ never a hit, never stored.

## C.3 Load / store and `bypass`

Load/store go through the same `OutputCache` as Part B, via `Target::Explicit(path)`
(vs. `Target::Addressed(<root>/<hex(digest)>)`). The two policies differ only in:

- **how a node maps to a file** — `OutputCache::target`: explicit `FsPath` vs.
  content-addressed digest path;
- **whether a present blob is rewritten** — explicit path is always (over)written;
  a content-addressed blob already on disk is skipped (same digest ⇒ same bytes).

`bypass` (a per-instance flag riding in the `SpecialNode` variant, toggled in the UI)
forces a recompute + overwrite every run, ignoring any existing file — so it's never
flagged available and never hydrated.

Invalidation is **user-managed**: delete the file, or flip `bypass`.
