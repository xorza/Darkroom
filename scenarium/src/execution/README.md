# `execution` — design notes

The compile→plan→execute pipeline and its caches. This README is the long-form
design home that the module's `//!` docs point at. Two parts:

- **A. Composite graphs** — how graphs used as nodes are authored and flattened
  away (`flatten/`; authoring types in `graph/`).
- **B. Disk cache** — the node-keyed, digest-stamped output cache for disk-backed
  (`Disk`/`Both`) nodes (`digest.rs`, `cache.rs`, `disk_store.rs`, `codec.rs`).
  `RuntimeCache` owns the RAM slots and `DiskStore` and applies the caching policy
  across both.

---

# Part A — Composite graphs

## A.1 Authoring and execution representations

`Graph` is the only authoring graph entity. It owns its nodes and wiring, its
exposed interface (`inputs`, `outputs`, and `events`), its editor metadata, and a
map of nested local graphs:

```text
Graph {
    name, category,
    inputs: Vec<FuncInput>,
    outputs: Vec<FuncOutput>,
    events: Vec<GraphEvent>,
    origin: Option<GraphId>,
    nodes, bindings, subscriptions,
    graphs: HashMap<GraphId, Graph>,
}
```

The root document value is a `Graph` with no exposed interface or boundary nodes.
A reusable graph is the same type stored under an external `GraphId`. There is no
definition wrapper and no duplicate id inside the mapped value.

Compilation lowers the authoring graph into an immutable `ExecutionProgram`.
Composite instances and boundaries disappear, leaving only flat func nodes for
scheduling, caching, pruning, and cycle detection.

## A.2 Interfaces and boundary nodes

A graph interface reuses `FuncInput` and `FuncOutput`, so graph and func instances
have the same port semantics. Behavior and sink-ness are derived while flattening
instead of being stored independently.

Two node kinds route values across a reusable graph's boundary:

- `NodeKind::GraphInput` has one output per `Graph.inputs` entry.
- `NodeKind::GraphOutput` has one input per `Graph.outputs` entry.

Either boundary may be absent when that side of the interface is empty. Boundaries
are routing only and emit no execution node. Validation reads their arity directly
from the containing `Graph`; it does not require an enclosing definition object.

## A.3 Graph instances and lookup

A composite instance is a node with `NodeKind::Graph(GraphLink)`.
`Node::graph_instance(graph, link)` shapes its visible interface from the referenced
graph. `GraphLink` identifies both the key and its owning registry:

```text
enum GraphLink {
    Local(GraphId),
    Shared(GraphId),
}
```

- `Local(id)` resolves through the containing `Graph.graphs`.
- `Shared(id)` resolves through `Library.graphs`.

Both registries are `HashMap<GraphId, Graph>`. `Graph::resolve_graph` returns the
`&Graph` value directly.

## A.4 Events, copying, and lineage

`GraphEvent` re-exports an interior emitter:

```text
GraphEvent {
    name,
    emitter: NodeId,
    emitter_event_idx: usize,
}
```

An instance has one outgoing event port per entry. Incoming events use ordinary
subscriptions; flattening expands a composite subscriber into the interior nodes
subscribed to its `GraphInput` trigger.

`Graph::fresh_copy()` preserves metadata and interface while assigning fresh node
ids throughout the copied graph tree and remapping event emitters. The caller
chooses a fresh `GraphId` when inserting that value. `origin: Option<GraphId>` is
editor-only lineage pointing from a local graph to its shared source.

## A.5 Validation and recursion

`Graph::check()` validates any graph, including its interface, boundaries, nested
graphs, and exposed events. `Graph::check_for_execution()` additionally resolves
the complete local/shared graph tree against the library and requires the entry
graph to have no interface or boundary nodes. The document boundary applies the
same entry constraints without requiring a runtime library.

Compilation validates links against the library and rejects recursive composition.
Flattening also tracks graph ids on the active descent path, while a hard depth cap
backs up boundary-resolution walks.

## A.6 Flattening

`Flattener::build` walks each graph level:

- func and special nodes emit flat execution nodes;
- graph nodes push the instance id onto the path and recurse into the resolved
  `Graph`;
- boundary and disabled nodes emit nothing.

Top-level nodes keep their authoring `NodeId`. Nested ids are deterministic hashes
of the descent path and interior id, so stable instances retain cache identity.

Data bindings cross boundaries as follows:

- a func or special producer resolves to its flat producer id;
- a graph producer descends through the matching `GraphOutput` input;
- a `GraphInput` producer ascends through the enclosing instance input;
- a disabled producer resolves to no source.

Event resolution similarly maps `GraphEvent` inward to a concrete emitter and
expands composite subscribers through `GraphInput`. The resulting execution program
contains only flat func-to-func data and event edges. Nodes, binding targets, and
event subscribers are all addressed directly by flat `NodeId`; `node_order` preserves
deterministic scheduling order independently from the node map. Disk and RAM caching
therefore need no composite-specific behavior.

---

# Part B — Disk (node-keyed output) cache

How a disk-backed node's outputs survive a reload — staying correct (never serve
a stale value) and lean (never hold bytes a run won't use).

## B.0 The cache mode is two storage bits

A node's `CacheMode` (`graph.rs`) is one four-state enum over two orthogonal bits — *keep
in RAM* (`caches_in_ram`, the RAM tier §B.5) and *persist to disk* (`persists_to_disk`,
this Part) — copied onto the flat node as `ExecutionNode.cache`:

| Mode | RAM | Disk | After a run |
|------|:-:|:-:|---|
| `None` | ✗ | ✗ | value **dropped** — recomputes whenever next needed |
| `Ram`  | ✓ | ✗ | value **kept resident**, reused across runs; lost on reopen |
| `Disk` | ✗ | ✓ | RAM copy **demoted to disk-only** (`OnDisk`); reloads lazily |
| `Both` | ✓ | ✓ | value **kept resident** *and* on disk — hot reuse + survives reopen |

This is **storage only** — the mode never feeds the content digest (§B.2). Purity alone
decides reproducibility; the mode decides where a reproducible value is stored. So a `None`
node still has a digest: a downstream `Disk`/`Both` consumer caches normally and, when that
consumer is a hit, the `None` node's cone is simply cut (§B.6) — never recomputed to feed a
value nothing reads. RAM reuse (`RuntimeCache::is_resident_hit`) trusts residency itself —
a content digest attests the value produced under it, however it came to be resident; the
RAM bit acts earlier, deciding what *stays* resident (the mid-run release and
`evict_unused`, §B.6). Disk reuse
(`mark_on_disk_if_present`) is gated on `persists_to_disk`. `evict_unused` reclaims RAM the
mode doesn't call to hold — demoting to a disk blob when one exists (lossless), else
dropping only a non-RAM mode's value.

## B.1 What opts into disk

The disk store serves a node whose mode is `persists_to_disk` (`Disk`/`Both`, `graph.rs`),
surfaced on the flat node as `ExecutionNode.cache`. Disk is a **request**, honored only when:

1. The node has a **content digest** — its whole upstream cone is reproducible (§B.2).
   An impure node, or anything downstream of one, has no digest and is silently kept
   memory-only — it can never risk serving a stale value.
2. A **disk root** is configured (`ExecutionEngine::set_disk_store` with a
   `disk_root`). Until then `Disk`/`Both` degrade to memory-only.

## B.2 The content digest is the cache key (`digest.rs`)

A node's output is a pure function of its `func_id`, resolved input values, upstream
outputs, and the content of any external files it reads. `node_digest` (`digest.rs`)
folds exactly that into a 256-bit BLAKE3 `Digest`. The **executor** computes it for each
node as it reaches the node (producer-first order), reading each `Bind` producer's
*already-stamped* `current_digest` — so no recursion, no memoization, and it's the
single place any digest is computed.

- **Equal digest ⇒ identical computation** only while the implementation behind each
  `FuncId` remains stable for the cache's lifetime.
- **Reproducibility taint.** Only `Pure` nodes are hashed; an `Impure` node, or any node
  with a non-reproducible producer, has digest `None` = "always recompute, never cache",
  for RAM and disk alike (a `None` producer folds to a `None` consumer).
- **File inputs.** An `FsPath` const folds the path string *and* the file's identity
  (`FileId`, default `(len, mtime)`; opt-in content hash), or — for a **directory** — a
  fingerprint of its entries (§`hash_dir_entries`), so the same path holding different
  bytes, or a folder gaining/losing/editing a file, invalidates. This is what re-keys
  `build_masters` when its calibration folders change.
- **Wired resource inputs.** A **resource reference** arriving over a `Bind` edge — an
  `FsPath` value, or a value of a custom type with a registered
  `ResourceStamper` (`TypeEntry::with_stamper`) — folds the *referent's* live identity,
  read off the **delivered value** (`hash_bound_resource`). The fold is gated on the
  consumer's *declared* input type (the contract "this node dereferences the reference"),
  resolved onto the input at flatten (`InputStamper`). The value must exist first: pre-run
  the fold taints the digest `None` ("uncacheable, must run", keeping the cone alive), and
  the run loop **re-stamps** such a node at reach time — its producers settled by then —
  serving the cache on a hit. So a loader fed by any path- or handle-producing node re-keys
  when the referent changes and still caches when it doesn't.
- **Output ports** of a multi-output node are disambiguated (`port_digest_of`); a
  `DOMAIN` separator versions the hashing scheme itself.
- **Output signature.** Each node's resolved output types (arity + each type, wildcards
  followed) are folded in, so redefining a func's outputs (`Int → Float`, an added port)
  re-keys it. This is what makes "a blob exists for this digest but is the wrong type"
  impossible by construction: a type change is a key change, so the stale blob is never
  looked up.
A node's digest is computed at **execution** (once per run, in the pre-run resolve sweep —
the run loop reads the resolver's verdicts rather than re-deriving, since a digest folds
live filesystem state and could drift mid-run), not `update`: only then are its producers'
`current_digest`s stamped. The planner is purely structural and never touches digests.

## B.3 Storage (`disk_store.rs`)

A node's blob lives at `<disk_root>/<hex(node id)>` — **one file per node**. Its outer
header is the 32-byte content digest, a format version, the output count, and one
materialization byte per output; the codec body follows. Presence probes read only this
header and require both a matching digest and a mask that covers the current run's
demanded outputs. Invalidation is an **overwrite**. A same-digest frame is skipped only
when its coverage already contains the new result; if a later run covers more outputs,
the frame is replaced. Writes are atomic (temp file + rename), so readers never see a
half-written frame.

## B.4 Values ↔ bytes (`codec.rs`)

`DynamicValue` isn't `Serialize`: `Unbound`/`Static` are trivial, but
`Custom(Arc<dyn CustomValue>)` is an opaque runtime payload. Each custom *type*
registers a `CustomValueCodec` on its `Library` type-table entry; that one entry drives
`encode` (async + context-aware, so a GPU-resident value can read back) and `decode`
(bytes + type id → value). The envelope (`CachedValue`) carries each custom value's
`type_id` so the loader picks the right codec. **Caching is all-or-nothing per node:** a
custom output whose type has no codec makes the whole node uncacheable, so a reload
never yields a half-real output set.

## B.5 The RAM tier (`cache.rs`)

Each flat `NodeId` has a `RuntimeSlot` in the ID-keyed runtime cache:

```
current_digest: Option<Digest>   // this run's content digest, stamped by the executor
value:          ValueState        // Empty | Resident { snapshot, produced_under }
                                  //       | OnDisk { coverage }
```

`ValueState` is one enum, not the old three loosely-coupled fields — so the previously
representable bad combos ("resident *and* flagged on disk", a stale RAM value masking a
fresh blob) can't be built. The reuse test is **`is_resident_hit`** — the slot is
`Resident`, its `produced_under` equals the current digest, and its snapshot coverage
covers the current `OutputDemand`. A `None` `current_digest` (impure cone) never hits.
`OnDisk` means a decodable frame exists for this digest and demand but is not yet
loaded: a cheap header probe, so a disk-cached value behind another reused node is served
without entering RAM (B.6).

## B.6 Execution-time lifecycle (`cache.rs` policy over `disk_store.rs` I/O)

All of it happens *during the run*, not at plan time — the planner is structural and does no
cache work. The executor first does a **pre-run cut** (`resolve.rs`): fold every node's digest
producer-first (from upstream digests — no lambda, no value load), decide reuse, then prune
any cone that feeds *only* reuse hits (a backward walk from the plan's roots). So a disk-cached
node's now-unneeded upstream — a memory-only source, say — isn't recomputed on reopen. Nearly
every digest is structural, so nearly the whole graph resolves here; a digest folding a Bind-delivered resource value it can't read yet (§B.2 wired
resource inputs) stamps `None` — resolved `Run`, cone kept alive — and the run loop re-stamps
it at reach time, serving the cache on a hit. Then, per surviving node, once its digest is
computed:

1. **resident hit?** — reuse only when the digest matches and the resident
   coverage contains every demanded output.
2. **else `mark_on_disk_if_present`.** If the node's blob carries the digest and a
   coverage containing current demand **and** the outputs are decodable (every custom output type has a codec —
   predicted without reading), flag the slot `OnDisk` and reuse it — a 32-byte header
   read + codec check, **no body bytes**. The value loads only if a running consumer
   actually reads it.
3. **else run.** The output buffer is cleared before invocation, and the successful
   result's coverage is derived from its non-`Unbound` values. Returning `Unbound` for a
   demanded output fails the node; skipped outputs may remain `Unbound`. `store_node` skips
   an existing frame only when both digest and coverage match; a broader result overwrites it.
4. **Lazy load — `hydrate_slot`.** When a *running* node reads a bound input whose
   producer is `OnDisk`, `collect_inputs` pulls that one blob into RAM. Producers behind
   a *reused* consumer are never read, so a disk-cached chain loads only its frontier — a
   blob can't be the wrong type (the signature is in the digest, §B.2), but if it fails
   to *load* (corrupt/deleted) the bad file is deleted and the demanding consumer is
   dropped for this run; the next reopen recomputes it.
5. **mid-run release — `reclaim_slot`.** The executor copies the plan's binding-reader
   counts into `RemainingOutputReads` and counts them *down* as each running consumer reads a bound producer
   (`ExecutionFrame::collect_inputs`). When an output reaches zero its value is cleared one output
   at a time, and once *every* output of a **non-RAM** node is spent its whole slot is reclaimed
   the instant its last consumer reads it — `reclaim_slot` demotes it to `OnDisk` if a blob
   serves it (a `Disk` value), else drops it (`None`). A `Ram`/`Both` node is left resident
   (kept hot for reuse). This bounds a chain's peak RAM to its active frontier instead of every
   intermediate at once. A node no consumer reads (a sink, or all its consumers cut) is
   reclaimed the moment it finishes.
6. **after the run → `evict_unused`.** The same `reclaim_slot` decision, swept over the
   leftovers step 5 didn't reach — a prior run's untouched value, or a non-RAM value a consumer
   never read (so its outputs never all went spent). A `caches_in_ram` node (`Ram`/`Both`) the
   run executed or read stays resident — as does a node-seeded preview root (`plan.pinned`),
   whose retained value is the point of its run; every other resident value goes through
   `reclaim_slot`:
   demoted to `OnDisk` **iff** a blob can serve it again (lossless — a later run
   reloads), else a **non-RAM** value (`None`, or a `Disk` value whose blob is missing) is
   dropped outright and a `Ram`/`Both` leftover with no blob is kept — so eviction never forces
   a recompute of a value a mode promised to retain.

### Worked example

`A → B → sink`, `A` and `B` both disk-backed (`Disk`/`Both`), after a cold run that
stored both. On reopen the pre-run cut resolves `A` and `B` as disk hits; the sink runs
and reads `B`, so `B`'s blob is pulled into RAM by `hydrate_slot` while `A` — read only by
the reused `B` — is pruned (flagged `OnDisk`, still reported cached, its bytes never enter
RAM). A `Ram` (memory-only) node feeding *only* `A` is cut too: it has no cross-session
cache and the reused `A` ignores it, so it is **not** recomputed on reopen (the win the
removed plan-time pass used to give). If a later edit invalidates `B`, `B` misses and
recomputes, reading `A` from disk (`A` becomes the frontier) and re-needing that memory-only
source, which then runs.

### Invalidation summary

- Any upstream change re-keys the digest → miss → recompute → store under the new key.
- **Output-type change** — a redefined output signature (type or arity) re-keys the
  digest (§B.2), so a stale blob of the wrong type is never even looked up. No runtime
  type check is needed: wrong-type-for-key is impossible by construction.
- Impure cone → no digest → never cached.
- Missing codec / corrupt / deleted blob → treated as a miss, not a failure
  (`mark_on_disk_if_present`'s codec check keeps a codec-less blob off the hot path; a
  failed load deletes the bad blob and self-heals on the next reopen).

---
