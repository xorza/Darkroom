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
- [ ] `runtime/mod.rs` — `blob_target` re-derives `(e_node_id, e_node, digest)`
      from `(program, node_idx)` on every call, at two sites per node per run
      (`reuse_source`, `store_node`).

## The cache's own state is also a public field, so callers bypass its methods

- [ ] `runtime/mod.rs` — `read_output_port` and `clear_output_port` destructure
      `ValueState::Resident` and mutate `snapshot.values` from the cache, while
      the neighbouring mutators (`clear_output`, `invoke_slot`, `stamp_produced`)
      are methods on `RuntimeSlot`. The same state is mutated from two layers.
- [ ] `runtime/mod.rs:43` — `pub(crate) disk_store` is public and the worker
      replaces it wholesale (`worker/task.rs:174`), so the cache cannot observe
      the swap that invalidates every reuse verdict it has already given.

## One reuse function serves two contracts, told apart only by the caller

`hydrate_reuse` has two callers with opposite preconditions: from `serve_reuse`
a probe already promised the blob loads and the producer cone is **cut**, so
failure is unrecoverable (`RunError::CacheLoadFailed`); from
`restamp_and_hydrate` no probe happened and the cone is **alive**, so failure
just runs the node. Nothing in the signature distinguishes them. See
`cache-reuse-path.md` for the full call-graph analysis.

- [ ] `resolve/mod.rs:126` — the resolver cuts a producer cone on `probe_reuse`'s
      bare `bool`, discarding the evidence behind it (which blob, which digest,
      which descriptors). `NodeState::Reuse` therefore records a promise without
      recording what justified it, and the hydrate that must keep the promise
      re-derives everything.
- [ ] `runtime/mod.rs:520` — `restamp_and_hydrate` is a 5-argument orchestration of
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
- [ ] `runtime/mod.rs` — `prepare` and `identify` differ only in whether the
      run's memo is reset first, and nothing in either name says so; `prepare`
      also repeats two of `clear`'s three field-clears verbatim.
- [ ] `runtime/mod.rs` — `stamp_digests` is a three-line loop over `stamp_digest`
      with one call site.

## RAM accounting mixes two aggregations and parks its scratch on the cache

- [ ] `runtime/mod.rs:52` — `ram_seen: HashSet<usize>` is a cross-run field used by
      exactly one method, which clears it on entry. It is per-call scratch stored
      on a struct whose other fields all survive runs.
- [ ] `runtime/mod.rs:96` — `resident_ram_stats` computes two different things in
      one loop (a pointer-deduplicated global total, an undeduplicated per-node
      total) and reports them two different ways (return value, `&mut Vec`
      out-parameter).

## Module boundary leaks

- [ ] `disk_store/mod.rs:68` — `DiskStore::new` takes `&Library` to call
      `library.codecs()` once, making the cache subtree depend on the library
      registry for a single extraction.
- [ ] `runtime/mod.rs:19` — `runtime` imports `DOMAIN`, `DigestHasher`, and
      `InputTag` from `digest` to perform the fold, while `digest` holds only the
      encoding primitives. The module named for the digest contains no code that
      computes one.

## Deliberate, recorded so they are not "fixed" by mistake

Not findings — correct answers to real constraints, listed because each looks
like an accident at the call site. Leave them; the doc comment beside each is
the fix.

- `store_node` returns `impl Future + 'a` from a manual `async move` block so the
  cache borrow ends before the await.
- `probe_reuse` takes `&mut self` while mutating nothing: a shared borrow held
  across its await would make the worker future non-`Send`.
- `InvokeSlot` exists to hand out two disjoint `&mut` borrows of one slot.
