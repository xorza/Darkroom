# `scenarium/src/execution/cache` — review

Findings only. **Delete each item once it is addressed** — this file lists open
items, nothing else. Groups are named after the shared root cause and ordered by
severity.

Scope: `cache/{mod,slot}.rs`, `cache/{digest,disk_store,resource,runtime}/`.
Production code only (~1 480 lines); test structure and the APIs tests use are
out of scope.

---

## The program the cache is aligned to is a parameter, never a field

Every slot in the cache is `NodeIdx`-aligned to one `Program`, but the cache
holds no reference to it, so each method re-receives it to learn what its own
indices mean. This is the single largest source of argument count and of
invariants that must be policed rather than represented.

- [ ] `runtime/mod.rs` — **16 of 24** production `RuntimeCache` methods take
      `program: &Program`: `evict`, `resident_ram_stats`, `reconcile` (as two
      parameters), `read_output_port`, `stamp_digests`, `stamp_digest`,
      `node_digest`, `prepare`, `identify`, `request_node_paths`,
      `restamp_and_hydrate`, `blob_target`, `probe_reuse`, `hydrate_reuse`,
      `store_node`, `release_dead_outputs`.
- [ ] `runtime/mod.rs:144` — `reconcile(previous, installed)` takes two programs
      and relies on a `debug_assert_eq!` plus a doc paragraph to state that
      `previous` must be the one the slots currently belong to. Passing the wrong
      one is expressible and only caught in debug.
- [ ] `runtime/mod.rs:96` — `resident_ram_stats` takes a whole `&Program` to read
      `e_node_ids` alone, and asserts the alignment it depends on.
- [ ] `runtime/mod.rs:207` — `read_output_port` takes `&Program` solely to compute
      `arity` for a `debug_assert_eq!`; nothing else in the body uses it.
- [ ] `runtime/mod.rs:507` — `blob_target` re-derives `(e_node_id, e_node, digest)`
      from `(program, node_idx)` on every call, and is called three times per node
      per run (`probe_reuse`, `hydrate_reuse`, `store_node`).

## The cache's own state is also a public field, so callers bypass its methods

- [ ] `runtime/mod.rs` — `read_output_port` and `clear_output_port` destructure
      `ValueState::Resident` and mutate `snapshot.values` from the cache, while
      the neighbouring mutators (`clear_output`, `invoke_slot`, `stamp_produced`)
      are methods on `RuntimeSlot`. The same state is mutated from two layers.
- [ ] `runtime/mod.rs:43` — `pub(crate) disk_store` is public and the worker
      replaces it wholesale (`worker/task.rs:174`), so the cache cannot observe
      the swap that invalidates every reuse verdict it has already given.

## "Is this value current?" has three implementations

The predicate `current_digest.is_some() && produced_under == current_digest` is
written twice, and wrapped in a three-deep accessor chain on one side.

- [ ] `runtime/mod.rs:178` `current_snapshot` and `slot.rs:124`
      `current_output_values` encode the same condition independently — one on the
      cache, one on the slot.
- [ ] `slot.rs:116,124` — `output_values` returns `Option<&Vec<DynamicValue>>` and
      `current_output_values` returns `Option<&[DynamicValue]>`: the same concept
      at two return types.
- [ ] `runtime/mod.rs:191` — `is_resident_current` is a one-line proxy for
      `current_snapshot(..).is_some()`.
- [ ] `runtime/mod.rs:196` — `is_resident_hit` is `current_snapshot` plus one
      `covers_demand` call, and is the only caller of
      `OutputSnapshot::covers_demand`.

## The reuse path is written three times over

Probe, hydrate, and store each repeat the same open-and-classify sequence, and
the disk layer repeats it again with different error handling per copy.

- [ ] `runtime/mod.rs:529,554` — `probe_reuse` and `hydrate_reuse` share an
      identical three-step preamble (`is_resident_hit` → `blob_target` → disk
      call) and diverge only at the last line. A probe and the hydrate that
      follows it can disagree about what is reusable; `RunError::CacheLoadFailed`
      exists to absorb that disagreement.
- [ ] `disk_store/mod.rs:111,129,141` — `covers`, `covers_demand`, and `read` each
      open the file, fetch `metadata().len()`, and dispatch into `format`. The
      three copies handle failure differently: two return `false` silently, one
      warns and returns `None`.
- [ ] `disk_store/format/mod.rs:124,148` — `covers_outputs` and `covers_demand` are
      two wrappers over `scan_header` differing only in their `accept` closure.
- [ ] `runtime/mod.rs:490` — `restamp_and_hydrate` is a 5-argument orchestration of
      `identify` + `stamp_digest` + `hydrate_reuse` with one call site.

## `StorePolicy` exists to carry one caller's knowledge into the store

Two variants, two call sites, and one of them drags a whole read path plus a
test counter through the disk layer.

- [ ] `disk_store/mod.rs:31` — `StorePolicy::KnownMiss` is passed only from
      `executor/mod.rs:444`, `PreserveCovering` only from `engine/mod.rs:169`.
      The enum encodes what the caller already established, not a behaviour of
      the store.
- [ ] `disk_store/mod.rs:111` — `covers` exists solely for the `PreserveCovering`
      arm, and is the only caller of `format::covers_outputs`.
- [ ] `disk_store/mod.rs:192-203` — `store` carries two `#[cfg(test)]` counter
      increments inline in the production path, and `DiskStore` carries a
      `#[cfg(test)] store_io` field (`:27`) to hold them.

## The stamp job's buffers are owned by one type and driven by another

`StampJob` was split off so the walk can cross to the blocking pool, but its
queue and result buffers are `pub(super)` and manipulated from the cache, so
neither type owns the pass end to end.

- [ ] `resource/mod.rs:114,117` — `requests` and `stamped` are `pub(super)`;
      the cache inserts into `requests` (`runtime/mod.rs:448`), clears it
      (`:65,404`), tests emptiness (`:463`), and drains `stamped` (`:474`).
- [ ] `runtime/mod.rs:396,411` — `prepare` is `clear` + `identify` and has one
      call site; `identify` has two. The two-step split is not visible to either
      caller.
- [ ] `runtime/mod.rs:248` — `stamp_digests` is a three-line loop over
      `stamp_digest` with one call site.

## RAM accounting mixes two aggregations and parks its scratch on the cache

- [ ] `runtime/mod.rs:52` — `ram_seen: HashSet<usize>` is a cross-run field used by
      exactly one method, which clears it on entry. It is per-call scratch stored
      on a struct whose other fields all survive runs.
- [ ] `runtime/mod.rs:96` — `resident_ram_stats` computes two different things in
      one loop (a pointer-deduplicated global total, an undeduplicated per-node
      total) and reports them two different ways (return value, `&mut Vec`
      out-parameter).

## Borrow workarounds surfaced in signatures

- [ ] `runtime/mod.rs:590` — `store_node` returns `impl Future + 'a` with a manual
      `async move` block instead of being an `async fn`, to end the cache borrow
      before the await. The reason is load-bearing but invisible at the call site.
- [ ] `runtime/mod.rs:529` — `probe_reuse` takes `&mut self` while mutating
      nothing, because a shared borrow held across its await would make the worker
      future non-`Send`. Documented, but the signature states the opposite of what
      the method does.
- [ ] `slot.rs:87` — `InvokeSlot` is a two-field struct whose only purpose is to
      return two disjoint `&mut` borrows of one slot.

## Module boundary leaks

- [ ] `disk_store/mod.rs:68` — `DiskStore::new` takes `&Library` to call
      `library.codecs()` once, making the cache subtree depend on the library
      registry for a single extraction.
- [ ] `runtime/mod.rs:19` — `runtime` imports `DOMAIN`, `DigestHasher`, and
      `InputTag` from `digest` to perform the fold, while `digest` holds only the
      encoding primitives. The module named for the digest contains no code that
      computes one.
- [ ] `resource/mod.rs:1-6` — the module doc opens with a duplicated line
      ("One\n//! One job serves…") left by an earlier edit.
