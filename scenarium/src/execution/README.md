# `execution` — design notes

The compile→plan→execute pipeline and its caches. This README is the long-form
design home that the module's `//!` docs point at. Two parts:

- **A. Subgraphs** — how composite nodes are authored and flattened away
  (`flatten/`; authoring types in `graph/subgraph.rs` and `graph/`).
- **B. Disk cache** — the node-keyed, digest-stamped output cache for disk-backed (`Disk`/`Both`) nodes
  (`digest.rs`, `cache.rs`, `disk_store.rs`, `codec.rs`). The `RuntimeCache`
  (`cache.rs`) owns the RAM slots *and* the `DiskStore` (`disk_store.rs`, pure blob I/O) and runs
  the caching policy — reuse, hydration, persistence, eviction — over both.
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

- **Authoring graph** (`Graph`, `graph/`) — what the editor edits. Holds composite
  instances (`NodeKind::Subgraph`), boundary nodes, and nested definitions.
  Serializable.
- **Execution program** (`ExecutionProgram`, `program.rs`) — the compiled, flattened,
  immutable form. No composites, no boundaries: flat func nodes only.

The compile step (§A.5) lowers the first into the second. Everything below the
compile boundary is composite-agnostic.

## A.3 Authoring representation

A **`SubgraphDef`** (`graph/subgraph.rs`) is a reusable definition: an interior `Graph` plus
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

Behavior (`Pure`/`Impure`) and sink-ness are **derived from the interior at
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
- **Nested** nodes get the first 128 bits of a domain-separated BLAKE3 digest over
  the descent-path ids and interior id. This is stable across builds, distinct from
  the bare interior id, and sensitive to both the path and the interior id. Every
  insertion into `FlattenMap` asserts that the flat id and scoped authoring address
  are both unique, so a collision is a hard logic failure rather than silent aliasing.

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

A node's output is a pure function of: its func (`func_id` + `func_version`), its
resolved input values, the outputs of its upstream producers, and the content of any
external files it reads. `node_digest` (`digest.rs`) folds exactly that into a 256-bit
BLAKE3 `Digest`. The **executor** computes it for each node as it reaches the node
(producer-first order), reading each `Bind` producer's *already-stamped* `current_digest`
— so no recursion, no memoization, and it's the single place any digest is computed.

- **Equal digest ⇒ identical computation**, on any machine — so the digest is at once
  the cache *key* and the *invalidation* signal. Change any const, binding, func
  version, or upstream output and every downstream digest changes.
- **Equal digest ⇒ identical computation**, on any machine.
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
  re-keys it *without* a version bump. This is what makes "a blob exists for this digest
  but is the wrong type" impossible by construction: a type change is a key change, so
  the stale blob is never looked up.
A node's digest is computed at **execution** (once per run, in the pre-run resolve sweep —
the run loop reads the resolver's verdicts rather than re-deriving, since a digest folds
live filesystem state and could drift mid-run), not `update`: only then are its producers'
`current_digest`s stamped. The planner is purely structural and never touches digests.

## B.3 Storage (`disk_store.rs`)

A node's blob lives at `<disk_root>/<hex(node id)>` — **one file per node**. Its outer
header is the 32-byte content digest, a format version, the output count, and one
materialization byte per output; the codec frame follows. Presence probes read only this
header and require both a matching digest and a mask that covers the current run's
demanded outputs. Invalidation is an **overwrite**. A same-digest frame is skipped only
when its mask already covers the new result; if a later run materializes more outputs,
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

Each node has a `RuntimeSlot` (index-aligned to `e_nodes`):

```
current_digest: Option<Digest>   // this run's content digest, stamped by the executor
value:          ValueState        // Empty | Resident { values, produced_under, materialized }
                                  //       | OnDisk { materialized }
```

`ValueState` is one enum, not the old three loosely-coupled fields — so the previously
representable bad combos ("resident *and* flagged on disk", a stale RAM value masking a
fresh blob) can't be built. The reuse test is **`is_resident_hit`** — the slot is
`Resident`, its `produced_under` equals the current digest, and its materialization mask
covers the current `OutputUsage`. A `None` `current_digest` (impure cone) never hits.
`OnDisk` means a decodable frame exists for this digest and demand mask but is not yet
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
   materialization mask covers every demanded output.
2. **else `mark_on_disk_if_present`.** If the node's blob carries the digest and a
   materialization mask covering current demand **and** the outputs are decodable (every custom output type has a codec —
   predicted without reading), flag the slot `OnDisk` and reuse it — a 32-byte header
   read + codec check, **no body bytes**. The value loads only if a running consumer
   actually reads it.
3. **else run.** The output buffer is cleared before invocation, and the successful
   result is stamped with the run's demand mask. `store_node` skips an existing frame
   only when both digest and coverage match; a broader result overwrites it.
4. **Lazy load — `hydrate_slot`.** When a *running* node reads a bound input whose
   producer is `OnDisk`, `collect_inputs` pulls that one blob into RAM. Producers behind
   a *reused* consumer are never read, so a disk-cached chain loads only its frontier — a
   blob can't be the wrong type (the signature is in the digest, §B.2), but if it fails
   to *load* (corrupt/deleted) the bad file is deleted and the demanding consumer is
   dropped for this run; the next reopen recomputes it.
5. **mid-run release — `reclaim_slot`.** The executor keeps the plan's per-output consumer
   counts live and counts them *down* as each running consumer reads a bound producer
   (`collect_inputs`). When an output reaches `Skip` (zero left) its value is cleared one output
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
- `func_version` bump invalidates a func's cached outputs across binaries.
- **Output-type change** — a redefined output signature (type or arity) re-keys the
  digest (§B.2), so a stale blob of the wrong type is never even looked up. No version
  bump needed, no runtime type check: wrong-type-for-key is impossible by construction.
- Impure cone → no digest → never cached.
- Missing codec / corrupt / deleted blob → treated as a miss, not a failure
  (`mark_on_disk_if_present`'s codec check keeps a codec-less blob off the hot path; a
  failed load deletes the bad blob and self-heals on the next reopen).

---
