# Scenarium execution review

The move to `NodeId`-keyed execution is a net simplification: bindings, cache slots,
outcomes, and resolver state now share one stable identity and no translation layer is
needed in the run loop. The remaining problems are mostly places where the old dense-node
shape still leaks through as duplicated identity/order state, or where structural planning
is carrying information that only the cache-aware resolver can determine exactly.

The highest-risk issue is stale disk availability surviving a store replacement. The
largest algorithmic opportunity is to make the resolver's backward cut produce exact
output demand and reader counts at the same time; the current plan over-approximates both
and knowingly executes some unreachable producer work.

Current flow: compile validates and flattens into an ID-keyed program plus SoA pools;
planning builds a producer-first structural schedule and missing-input verdicts; resolution
stamps digests, then derives exact cache-aware liveness, output demand, and readers in one
reverse sweep; execution hydrates or runs nodes and counts down live readers; eviction and
RAM accounting finish the run.

## Batch 1 — Critical: invalidate stale disk availability

- [x] **Invalidate `OnDisk` claims when the disk store is replaced.** Store replacement now clears only `OnDisk` slots through `RuntimeCache::set_disk_store`, preserves resident values, and is covered by switching a populated cache to an empty root before forcing the previously hidden producer to run.

## Batch 2 — High: let cache-aware liveness own demand and reader counts

- [x] **Separate digest stamping and cache-coverage probing from the final reuse verdict.** `stamp_digests` now performs only the producer-first digest fold; `resolve_run` probes resident/disk coverage later, after consumers have accumulated exact demand for each live node.

- [x] **Fuse disposition, exact output demand, and exact reader counts into one reverse resolver sweep.** `ResolvedRun` owns all three columns; the consumer-first sweep seeds graph/node pins, probes each needed node once, and propagates demand/readers only from `Run` nodes. Coverage includes a two-output producer reusing a one-port snapshot while the other port's consumer reuses its own cache.

- [x] **Stop the backward cut at `MissingInputs` and delete structural reader accounting from the plan.** Missing nodes remain cut and propagate no upstream demand; `ExecutionPlan` now owns only topology/verdicts/roots, and `RemainingOutputReads` seeds from `ResolvedRun`. Tests cover a blocked-only producer staying cut and a transient shared producer being reclaimed after its sole running consumer reads it.

## Batch 3 — High: keep resource stamping off the async worker

- [x] **Move filesystem identity collection onto the blocking pool and keep `node_digest` a pure fold over prepared stamps.** `RunResourceStamps` collects filesystem metadata and custom stamps through `spawn_blocking`, passes prepared identities into the resolver's pure producer-first fold, and serves the executor's late bound-resource restamps through the same path. Custom stampers receive the cooperative cancel token. A current-thread Tokio regression proves a blocking stamper does not stall runtime progress and observes cancellation; a fixed digest regression proves cache-key bytes did not change.

- [x] **Memoize each resolved resource identity for the duration of one run.** Paths are keyed by their string and custom resources by stamper plus runtime-value identity, so all consumers fold one coherent snapshot. Tests cover two identical path consumers remaining coherent across a mid-run file edit, refresh on the next run, and one custom stamper call per shared resource per run.

## Batch 4 — Medium: finish the ID-keyed representation cleanup

- [x] **Delete `ExecutionNode.id` and `ExecutionNode.name`, then construct each node once.** `ExecutionNode` now stores only executable data and is inserted fully populated under its map key. Tests resolve names through `FlattenMap` and the authoring graph instead of retaining production metadata.

- [x] **Remove `node_order` unless a real independent-node ordering contract is introduced.** Full-program sweeps now use `e_nodes` keys and the planner alone derives dependency order. Independent roots explicitly have no ordering contract.

- [x] **Use `NodeSet` for `ExecutionPlan::roots` and `pinned`.** Root collection deduplicates at insertion, pinned membership is constant-time, and repeated node seeds are covered.

- [x] **Replace the one-use generic `Column<I, T>` abstraction with a concrete `OutputColumn<T>`.** The concrete wrapper retains typed `OutputIdx` indexing and span slicing without `ColumnIndex` or `PhantomData`.

- [x] **Delete `Flattener::inputs_scratch` or redesign compilation to actually recycle artifacts.** Flattening now builds directly into the fresh program input pool; only traversal scratch remains reusable.

## Batch 5 — Medium: remove duplicated cache state and release-only invariant work

- [x] **Store coverage only where it is not derivable: the disk metadata states.** Resident snapshots now store only values and test demand directly against boundness. Coverage remains in `OnDisk` and blob headers, is derived when storing or reclaiming, and is checked against decoded values before hydration. Sparse reuse, per-port clearing and last-read moves remain covered, while disk tests verify hydration, corrupt metadata rejection, and broader-coverage overwrite.

- [ ] **Compute total and per-node RAM usage in one cache traversal.** The engine walks every resident value twice after every run and calls each custom value's `ram_bytes` twice (`scenarium/src/execution/cache/mod.rs:287`, `scenarium/src/execution/cache/mod.rs:313`, `scenarium/src/execution/mod.rs:348`). Return a named `CacheRamStats { total, by_node }` from one method; accumulate per-node usage once while applying pointer dedup only to the global total. Verify shared `Arc` values remain counted once globally and once per owning node.

- [ ] **Move hot-path logic assertions to debug builds.** Output-index conversion, DFS coloring, reader consumption, outcome transitions, cache coverage checks, and per-port reads/releases all run per node/edge/output yet use release `assert!`/`assert_eq!` (`scenarium/src/execution/program.rs:159`, `scenarium/src/execution/plan/mod.rs:226`, `scenarium/src/execution/executor/mod.rs:99`, `scenarium/src/execution/executor/mod.rs:373`, `scenarium/src/execution/cache/mod.rs:64`, `scenarium/src/execution/cache/mod.rs:229`, `scenarium/src/execution/cache/mod.rs:398`). Convert internal invariants to `debug_assert!` and rely on the existing debug validation pass for structural guarantees; keep explicit fallible checks at disk/blob and authored-input trust boundaries. Verify debug tests still trip deliberately malformed internal fixtures and release execution no longer pays these checks.

## Batch 6 — Medium: consolidate compile-time resolution and correct its design contract

- [x] **Use one exact wildcard-output type algorithm and memoize it by `OutputIdx`.** Authoring and compiled resolution now share `OutputTypeResolver`; compiled constant mirrors read their declared type from the function spec, outputs memoize by `OutputIdx`, cycles resolve to `Any`, and there is no depth cap. Parity coverage includes fixed, bound, typed-const, scalar and ambiguous `Any` constants, unbound, cycle, and a 70-hop chain; output-signature digest invalidation remains covered independently.

- [ ] **Maintain the current graph on the flatten descent instead of re-resolving the full path.** Every `Run::current` starts from the root and replays all instance lookups (`scenarium/src/execution/flatten/mod.rs:166`, `scenarium/src/execution/flatten/mod.rs:238`), and binding/event resolution calls it repeatedly. Keep a local `Vec<&Graph>` or equivalent current-graph stack parallel to the ID path; push the already resolved child graph on descent and pop on ascent. This changes nested flattening from repeated root-to-leaf walks toward one lookup per descent without adding persistent borrowed state to `Flattener`. Verify identical flat IDs, bindings, subscriptions, and attribution over deeply nested local/shared composites.

- [ ] **Refresh the execution design docs after the preceding changes.** The module overview says the executor resolves reuse and computes each digest inline even though the resolver stamps almost all digests before the run (`scenarium/src/execution/mod.rs:11`); `program.rs` says all mutable execution state lives in the executor despite the cross-run runtime cache (`scenarium/src/execution/program.rs:3`); and the README both claims false `node_order` determinism and gives contradictory/duplicated digest timing statements (`scenarium/src/execution/README.md:132`, `scenarium/src/execution/README.md:184`, `scenarium/src/execution/README.md:188`, `scenarium/src/execution/README.md:217`). Update the flow only after the resolver and representation batches settle, then verify every phase description names the actual owner of schedule, resolved liveness, cache state, and digest work.

## Open questions

- [x] **Should independent impure roots have a stable execution order?** No current contract requires one. The false ordering claim and duplicate state were removed; a future ordering requirement must begin with an explicitly ordered authoring source.

Recommended execution order: Batch 1 first; Batch 2 before the resident-coverage cleanup because it changes the cache probe API; Batch 3 can proceed independently after the resolver API shape is chosen; then Batches 4–6.
