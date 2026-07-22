# `execution` — design notes

The compile→plan→execute pipeline and its caches. This README is the long-form
design home that the module's `//!` docs point at. Two parts:

- **A. Composite graphs** — how graphs used as nodes are authored and flattened
  away (`flatten/`; authoring types in `graph/`).
- **B. Runtime cache** — the node-keyed, digest-stamped RAM and disk cache
  (`digest.rs`, `cache/`, `disk_store/`, `codec.rs`).
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

`Graph::validate()` validates any graph, including its interface, boundaries, nested
graphs, and exposed events. `Graph::validate_for_execution()` additionally resolves
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
- boundary nodes emit nothing;
- disabled function and special nodes remain in the program with an effective
  disabled bit inherited through composite ancestors.

Top-level nodes keep the UUID value of their authoring `NodeId` behind the distinct
`ExecutionNodeId` type. Nested ids are deterministic hashes of the descent path and
interior id, so stable instances retain cache identity.

Data bindings cross boundaries as follows:

- a func or special producer resolves to its flat producer id;
- a graph producer descends through the matching `GraphOutput` input;
- a `GraphInput` producer ascends through the enclosing instance input;
- a disabled producer remains connected, but planning treats it as unavailable
  unless that node is explicitly seeded for the run.

Event resolution similarly maps `GraphEvent` inward to a concrete emitter and
expands composite subscribers through `GraphInput`. The resulting execution program
contains only flat func-to-func data and event edges. Nodes, binding targets, and
event subscribers are all addressed directly by `ExecutionNodeId`. The planner derives
dependency order for each run; independent roots have no ordering contract. Disk and RAM
caching therefore need no composite-specific behavior.

---

# Part B — Runtime (node-keyed output) cache

How node outputs are reused in RAM or survive a reload on disk — staying correct
(never serve a stale value) and lean (never hold bytes a run won't use).

## B.0 The cache mode is two storage bits

A node's `CacheMode` (`graph.rs`) is one four-state enum over two orthogonal bits — *keep
in RAM* (`caches_in_ram`, the RAM tier §B.5) and *persist to disk* (`persists_to_disk`,
this Part) — copied onto the flat node as `ExecutionNode.cache`:

| Mode | RAM | Disk | After a run |
|------|:-:|:-:|---|
| `None` | ✗ | ✗ | value **dropped** — recomputes whenever next needed |
| `Ram`  | ✓ | ✗ | value **kept resident**, reused across runs; lost on reopen |
| `Disk` | ✗ | ✓ | RAM copy **demoted to disk-only** (`OnDisk`); reloads at the live frontier |
| `Both` | ✓ | ✓ | value **kept resident** *and* on disk — hot reuse + survives reopen |

This is **storage only** — the mode never feeds the content digest (§B.2). Purity alone
decides reproducibility; the mode decides where a reproducible value is stored. So a `None`
node still has a digest: a downstream `Disk`/`Both` consumer caches normally and, when that
consumer is a hit, the `None` node's cone is simply cut (§B.6) — never recomputed to feed a
value nothing reads. RAM reuse (`RuntimeCache::is_resident_hit`) trusts residency itself —
a content digest attests the value produced under it, however it came to be resident; the
RAM bit acts earlier, deciding what *stays* resident (the mid-run release and
`evict_unused`, §B.6). Disk reuse is gated on `persists_to_disk`. `evict_unused` reclaims
RAM the mode doesn't call to hold — demoting to a disk blob when one exists (lossless),
else dropping only a non-RAM mode's value.

## B.1 What opts into disk

The disk store serves a node whose mode is `persists_to_disk` (`Disk`/`Both`, `graph.rs`),
surfaced on the flat node as `ExecutionNode.cache`. Disk is a **request**, honored only when:

1. The node has a **content digest** — its whole upstream cone is reproducible (§B.2).
   An impure node, or anything downstream of one, has no digest and is silently kept
   memory-only — it can never risk serving a stale value.
2. A **disk root** is configured (`ExecutionEngine::set_disk_store` with a
   `disk_root`). Until then `Disk`/`Both` degrade to memory-only.

## B.2 The content digest is the cache key (`digest.rs`)

A node's output is a pure function of its `func_id`, function `version`, resolved input
values, upstream outputs, and the content of any external files it reads. `node_digest` (`digest.rs`)
folds exactly that into a 256-bit BLAKE3 `Digest`. Before the fold,
`RunResourceStamps` (`resource.rs`) resolves filesystem identities and custom resource
stamps on Tokio's blocking pool, memoizing each identity for the run. The resolver then
computes node digests producer-first from prepared resource identities and each `Bind`
producer's *already-stamped* `current_digest`, with no recursive digest traversal or I/O
inside `node_digest`.

- **Function implementation.** A pure `Func`'s `version` is folded beside its stable
  `FuncId`; incrementing it invalidates that function and every downstream digest without
  breaking authored nodes that reference the `FuncId`.
- **Reproducibility taint.** Only `Pure` nodes are hashed; an `Impure` node, or any node
  with a non-reproducible producer, has digest `None` = "always recompute, never cache",
  for RAM and disk alike (a `None` producer folds to a `None` consumer).
- **File inputs.** An `FsPath` const folds the path string *and* the file's identity
  (`FileId`, default `(len, mtime)`; opt-in content hash), or — for a **directory** — a
  sorted fingerprint of its entries, so the same path holding different
  bytes, or a folder gaining/losing/editing a file, invalidates. This is what re-keys
  `build_masters` when its calibration folders change.
- **Wired resource inputs.** A **resource reference** arriving over a `Bind` edge — an
  `FsPath` value, or a value of a custom type with a registered
  `ResourceStamper` (`TypeEntry::with_stamper`) — folds the *referent's* live identity,
  read off the **delivered value** (`hash_bound_resource`). The fold is gated on the
  consumer's *declared* input type (the contract "this node dereferences the reference"),
  resolved onto the input at flatten (`InputStamper`). The value must exist first: pre-run
  the fold taints the digest `None` ("uncacheable, must run", keeping the cone alive), and
  the run loop prepares the newly available identity through the same blocking-pool
  stamp set and **re-stamps** such a node at reach time — its producers settled by then —
  serving the cache on a hit. So a loader fed by any path- or handle-producing node
  re-keys when the referent changes and still caches when it doesn't.
- **One coherent resource identity per run.** Repeated references to the same path, or
  repeated consumers of the same custom resource, reuse one prepared identity. The next
  run clears and refreshes the stamps. `ResourceStamper` receives the run's cooperative
  cancel token so long-running custom identity work can stop promptly.
- **Output ports** of a multi-output node are disambiguated (`port_digest_of`); a
  `DOMAIN` separator versions the hashing scheme itself.
- **Output signature.** Each node's resolved output types (arity + each type, wildcards
  followed) are folded in, so redefining a func's outputs (`Int → Float`, an added port)
  re-keys it. This is what makes "a blob exists for this digest but is the wrong type"
  impossible by construction: a type change is a key change, so the stale blob is never
  looked up.
A node's digest is computed at **execution** (once per run, in the pre-run resolve sweep),
not `update`. The run loop reads the resolver's verdicts rather than re-deriving them;
only a node whose bound resource becomes readable after its producers settle is improved
at reach time, using the same run stamps. The planner is purely structural and never
touches digests.

## B.3 Storage (`disk_store/`)

A node's blob lives at `<disk_root>/<hex(node id)>` — **one file per node**. Its indexed
header carries a magic value, format version, 32-byte content digest, body length, and one
fixed descriptor per output. A descriptor records the value kind and payload length plus the
type identity and codec version for custom values. Presence probes read only the header,
require matching content and current codec versions, and derive exact output coverage from
the descriptors. Invalidation is an **overwrite**. A write is skipped only when the existing
blob already covers every value being stored. Writes stream into a temporary file and publish
it atomically, so readers never see a partially encoded generation.

## B.4 Values ↔ bytes (`codec.rs`)

`DynamicValue` isn't `Serialize`: `Unbound`/`Static` are trivial, but
`Custom(Arc<dyn CustomValue>)` is an opaque runtime payload. Each custom *type*
registers a `CustomValueCodec` on its `Library` type-table entry; that one entry drives
async, context-aware `encode` and async `decode` over bounded Tokio I/O streams. Every codec
also declares a `version`; each custom output descriptor records its actual type and version,
so custom values flowing through an `Any` output are covered while unrelated codec changes
leave the blob valid. The loader verifies every descriptor before decoding, then restricts
each decoder to exactly its declared payload region and requires it to consume that region.
**Caching is all-or-nothing per node:** a custom output whose type has no codec makes the
whole node uncacheable, so a reload never yields a half-real output set.

## B.5 The RAM tier (`cache/`)

Each `ExecutionNodeId` has a `RuntimeSlot` in the ID-keyed runtime cache:

```
current_digest: Option<Digest>   // this run's content digest, stamped by the resolver
value:          ValueState        // Empty | Resident { snapshot, produced_under }
                                  //       | OnDisk
```

`ValueState` is one enum, not the old three loosely-coupled fields — so the previously
representable bad combos ("resident *and* flagged on disk", a stale RAM value masking a
fresh blob) can't be built. The reuse test is **`is_resident_hit`** — the slot is
`Resident`, its `produced_under` equals the current digest, and every demanded snapshot
value is bound. A `None` `current_digest` (impure cone) never hits.
`OnDisk` means a resident slot was demoted after its current disk blob was validated. It is
not a reuse verdict: the resolver still verifies and decodes the body before cutting its
producer cone. A disk-cached value behind another reused node is never opened and therefore
never enters RAM (B.6).

## B.6 Execution-time lifecycle (`cache/` policy over `disk_store/` I/O)

All of it happens *during the run*, not at plan time — the planner is structural and does
no cache work. First the engine prepares one per-run resource stamp set on the blocking
pool. The resolver then makes two passes over the structural schedule: a producer-first
pass stamps content digests from upstream digests and prepared resource identities; a
consumer-first pass derives cache-aware liveness, exact output demand, and exact binding
reader counts together. The reverse pass starts from the plan's roots, tests reuse only
after all live downstream demand has accumulated, and propagates demand and readers only
through nodes that will run. Cache hits and missing-input nodes therefore cut their entire
upstream cone. A disk-cached node's now-unneeded upstream — a memory-only source, say —
isn't recomputed on reopen.

A digest folding a Bind-delivered resource value it can't read yet (§B.2 wired resource
inputs) stamps `None` — resolved `Run`, cone kept alive — and the run loop prepares that
identity off-thread and re-stamps it at reach time, serving the cache on a hit. Cache
resolution and execution then follow this lifecycle:

1. **resident hit?** — reuse only when the digest matches and every demanded resident
   output is non-`Unbound`.
2. **else verify disk reuse.** The loader opens the blob once, checks its digest, output
   count, codec versions, and demanded coverage while scanning the header, then continues
   through the same file to decode its body. Insufficient coverage is rejected before any
   payload is read, while a valid partial blob remains available for a narrower future run.
   Only a successful decode becomes `Reuse` and cuts the producer cone. A corrupt body is
   deleted and treated as a miss; the same reverse sweep continues through its producers, so
   the node recomputes this run.
3. **else run.** The output buffer is cleared before invocation. Returning `Unbound` for a
   demanded output fails the node; skipped outputs may remain `Unbound`. Resident snapshots
   store only those values; `store_node` derives disk coverage from them and skips an existing
   frame only when both digest and coverage match. A broader result overwrites it.
4. **mid-run release — `reclaim_slot`.** The executor seeds `RemainingOutputReads` from
   the resolver's exact, cache-aware reader counts and counts them *down* as each running
   consumer reads a bound producer (`ExecutionFrame::collect_inputs`). When an output
   reaches zero its value is cleared one output at a time, and once *every* output of a
   **non-RAM** node is spent its whole slot is reclaimed the instant its last consumer
   reads it — `reclaim_slot` demotes it to `OnDisk` if a blob serves it (a `Disk` value),
   else drops it (`None`). A `Ram`/`Both` node is left resident (kept hot for reuse).
   This bounds a chain's peak RAM to its active frontier instead of every intermediate
   at once. A node no consumer reads (a sink, or all its consumers cut) is reclaimed the
   moment it finishes.
5. **after the run → `evict_unused`.** The same `reclaim_slot` decision, swept over the
   leftovers step 4 didn't reach — a prior run's untouched value, or a non-RAM value a consumer
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
stored both. On reopen the pre-run cut verifies and hydrates `B`, then prunes `A` because its
only consumer is reused. `A`'s blob is never probed or decoded. A `Ram` (memory-only) node
feeding *only* `A` is cut too: it has no cross-session
cache and the reused `B` makes `A` unnecessary, so it is **not** recomputed on reopen (the win the
removed plan-time pass used to give). If a later edit invalidates `B`, `B` misses and
recomputes, reading `A` from disk (`A` becomes the frontier) and re-needing that memory-only
source, which then runs.

### Invalidation summary

- Any upstream change re-keys the digest → miss → recompute → store under the new key.
- **Output-type change** — a redefined output signature (type or arity) re-keys the
  digest (§B.2), so a stale blob of the wrong type is never even looked up. No runtime
  type check is needed: wrong-type-for-key is impossible by construction.
- **Used codec-version change** — a blob naming that codec is rejected before decoding and
  overwritten without invalidating RAM values, downstream nodes, or blobs using other codecs.
- Impure cone → no digest → never cached.
- Missing codec / corrupt / deleted blob → treated as a miss, not a failure
  (the header manifest keeps a codec-less blob off the hot path; a
  failed load deletes the bad blob and recomputes it in the same run).

---
