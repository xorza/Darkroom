# `scenarium/src/execution/cache` — open items

Findings only. **Delete each item once it is addressed** — this file lists open
items, nothing else. Grouped by change area (each group is one edit, landable
alone), groups ordered by impact.

Scope: `cache/{mod,slot}.rs`, `cache/{digest,disk_store,resource,runtime}/`.
Production code only; test structure and the APIs tests use are out of scope.

---

## 1. `RuntimeCache` ↔ `Program`: the alignment is a parameter, never a field

Every slot is `NodeIdx`-aligned to one `Program`, but the cache holds no
reference to it, so each method re-receives it to learn what its own indices
mean. The single largest source of argument count, and of invariants policed
rather than represented.

The cache now holds `aligned_to: Arc<Program>`, shared with the artifact it came
from, and `reconcile` reads the program it is leaving off that field. The
remaining methods still receive one per call.

- [ ] `runtime/mod.rs` — **16 of 26** production `RuntimeCache` methods take
      `program: &Program`: `evict`, `resident_ram_stats`, `read_output_port`,
      `stamp_digests`, `stamp_digest`, `node_digest`, `prepare`, `identify`,
      `request_node_paths`, `restamp_and_hydrate`, `blob_target`, `probe_reuse`,
      `reuse_source`, `hydrate_reuse`, `store_node`, `release_dead_outputs`.
      Each is free to be handed a program that is not `aligned_to` — the
      indices then mean something the slots do not.
- [ ] `runtime/mod.rs:236` — `read_output_port` exists only to take `&Program` and
      check the resident arity against it before delegating to
      `RuntimeSlot::read_output`. The one slice of this group that lands standalone.
- [ ] `runtime/mod.rs:154` — `resident_ram_stats` takes a whole `&Program` for
      `e_nodes.len()`, and asserts the alignment it depends on.
- [ ] `runtime/mod.rs:521` — `blob_target` re-derives `(e_node_id, e_node, digest)`
      from `(program, node_idx)` at two sites per node per run (`reuse_source`,
      `store_node`).

**Cost to weigh before starting.** The field exists; what is left is the borrow.
A `&mut self` method cannot also hold `&self.aligned_to`, which is plausibly why
the parameter was threaded to begin with. Either the hot methods clone the `Arc`
into a local (an atomic bump per node per run), or the slots move behind an inner
struct the program field can be borrowed alongside. Pick that before touching
signatures.

## 2. `RuntimeCache` ↔ `StampJob`: buffers owned by one type, driven by another

`StampJob` was split off so the walk can cross to the blocking pool, but its
queue and result buffers are `pub(super)` and manipulated from the cache, so
neither type owns the pass end to end.

- [ ] `resource/mod.rs:113,116` — `requests` and `stamped` are `pub(super)`; the
      cache inserts into `requests` (`runtime/mod.rs:462`), clears it (`:122,418`),
      tests emptiness (`:477`), and drains `stamped` (`:488,683`). Three methods
      on `StampJob` — request, is-queued, drain-stamped — close it, and the fields
      go private.

## 3. `RuntimeCache` ↔ `DiskStore`: boundaries

- [ ] `runtime/mod.rs:55` — `pub(crate) disk_store` is public and the worker
      replaces it wholesale (`worker/task.rs:174`). The swap happens in
      `apply_intent`, between runs, so no verdict is actually in flight — but the
      cache cannot see it happen, and a setter costs nothing. Tests assign the
      field directly in ~10 places and would need a gated helper.
- [ ] `disk_store/mod.rs:68` — `DiskStore::new` takes `&Library` to call
      `library.codecs()` once, making the cache subtree depend on the library
      registry for a single extraction. Take the `Codecs`.

## 4. `OutputSnapshot`: the buffer is public to everyone holding one

- [ ] `slot.rs:10` — `OutputSnapshot::values` is `pub(crate)`, read directly at
      `disk_store/mod.rs:204,233` (the whole slice, into `covers` and
      `format::write`) and `runtime/mod.rs:367` (one indexed port, in
      `hash_bound_fs_path`). One level below the slot field that just went
      private, and the same shape of leak — but read-only: nothing outside
      `slot.rs` writes through it, so an accessor closes it and no invariant is
      at risk meanwhile. Lowest-value item here; worth folding into whichever
      change next touches the disk store.

---

## Settled — do not re-open

Each of these was a finding once and is now answered, either by a deliberate
design decision or by a change that landed. Listed so they are not "fixed" by
mistake; the doc comment beside each in the source is the real record.

- `store_node` returns `impl Future + 'a` from a manual `async move` block so the
  cache borrow ends before the await.
- `probe_reuse` takes `&mut self` while mutating nothing: a shared borrow held
  across its await would make the worker future non-`Send`.
- `InvokeSlot` exists to hand out two disjoint `&mut` borrows of one slot.
- `DiskStore`'s `#[cfg(test)] store_io` field and the two inline counter bumps in
  `store` are exactly the two mid-file gates the project's gating rule sanctions
  (a struct field; an inline statement in a production fn).
- `StorePolicy`'s two variants are two store *behaviours* — `PreserveCovering`
  probes coverage first, `KnownMiss` does not — not caller knowledge leaking in.
  Splitting into two methods is a wash, and `covers` existing only for the first
  arm is a consequence, not a separate finding.
- `restamp_and_hydrate` has one call site by design: it is the named late second
  chance at reuse, and its point is that every failure inside it is attributable
  to exactly one node.
- `prepare` vs `identify` (reset-then-walk vs walk) and `stamp_digests` vs
  `stamp_digest` are each documented at both ends; the ordering guarantee is what
  `stamp_digests` is for.
- `resident_ram_stats` computing a pointer-deduplicated total and an
  undeduplicated per-node column in one pass — one walk, two outputs, and the
  `&mut NodeColumn` out-parameter is dense-alignment plus allocation reuse.
  `ram_seen` is that pass's reused scratch, like `stamp_job`'s buffers.
- `node_digest` living on the cache while `digest/` holds only the encoding: the
  fold reads the slots, the producer digests, and the `fs_paths` memo — all three
  are the cache's. Recorded at `runtime/mod.rs:309-311`.
- Carrying the probe's evidence into `NodeState::Reuse` — see below.

### The reuse path, closed

`hydrate_reuse` has two callers with opposite preconditions, and only the caller
knows which one it is:

| caller | reached via | was there a probe? | producer cone | failure means |
| --- | --- | --- | --- | --- |
| `executor::serve_reuse` | `NodeState::Reuse` | **yes**, in the resolve sweep | **cut** | unrecoverable → `RunError::CacheLoadFailed` |
| `executor::needs_invoke` → `restamp_and_hydrate` | `NodeState::Run`, digest was `None` | **no** | **alive** | fine → returns `false`, node runs |

So `hydrate_reuse` is not "the second half of `probe_reuse`". It is a standalone
*serve-if-you-can* used in one recoverable and one unrecoverable context, and
`schedule/mod.rs:609` cuts a producer cone on a verdict a different function, at
a different time, has to make good on.

The two mechanical dedups landed: `DiskStore::open_blob` is the one
open-and-measure preamble behind `covers`/`covers_demand`/`read`, and
`RuntimeCache::reuse_source` → `ReuseSource::{Resident, Blob}` is the one
preamble behind `probe_reuse`/`hydrate_reuse`.

**Carrying the approved `BlobTarget` on `NodeState::Reuse` is rejected.** The
claim was that it removes every way the two derivations can disagree. After
`reuse_source` they cannot disagree at all:

- `blob_target` is pure in `(e_node_ids[node_idx], program[node_idx],
  current_digest)`; the first two are fixed for the install.
- `current_digest` is stamped once, in `stamp_digests` before the sweep. The only
  other writer is `restamp_and_hydrate`, reached exclusively from `needs_invoke`
  on a `NodeState::Run` node — never a `Reuse` one.
- `release_dead_outputs` runs at install and *after* the run (`engine/mod.rs:138`),
  not between resolve and execution, so residency cannot move under a pending
  `Reuse` either.

What is left to carry is a `PathBuf::join` per reused node, against a wider
schedule payload — and carrying the *Resident* verdict is a small regression,
turning hydrate's re-check into an assumption, so a slot that did empty would
surface as a downstream panic instead of a clean `CacheLoadFailed`.

**Collapsing the two phases is also rejected**, for a different reason:
`runtime/mod.rs:585-588` — decoding at probe time would pull every frontier blob
into RAM before the first lambda runs, instead of interleaving decodes with
execution. The two-phase split and the header-only probe are both load-bearing.

The durable part: the problem was never that the reuse path is written three
times, it is that **one function serves two contracts and the caller's context is
what distinguishes them** — now documented at both call sites.
