# Review: scenarium

Scope: `scenarium/src`, production code only (tests and `internals/` ignored).

## Proposal

### Root causes

1. **No single "what does this input deliver this run" rule.** The schedule
   (`RunSchedule::input_missing`), the digest (`RuntimeCache::node_digest`) and
   the run loop (`ExecutionFrame::collect_inputs`) each restate "overridden →
   unbound, unbound → unbound, bind to a producer that does not run → unbound".
   The override half lives on the compile artifact
   (`ExecutionInput::delivers`, `CompiledGraph::overridden`) and reads the
   per-run `NodeState`, which makes `compile` import `schedule`.
2. **The output cache owns lambda state.** `RuntimeSlot` holds the cached
   value and digest and also the node's `owner`, `state` and `event_state`.
   The cache module so carries lambda runtime state, its slots are not `Sync`
   (two `needless_pass_by_ref_mut` expects), and the executor reaches into the
   cache for a lambda's state.
3. **`RuntimeCache` has five jobs.** Output slots, the disk store, the
   per-run path-identity memo with its blocking walk, the eviction cone, and
   RAM measurement. The path memo (`fs_paths`, `previous_fs_paths`,
   `stamp_job`) is one cohesive unit that only the digest fold reads.
4. **The cache-aware sweep sits in the wrong module.** `RunSchedule::resolve`
   drives `RuntimeCache`, and `RuntimeCache` reads `NodeState`, so `schedule`
   and `cache` import each other.
5. **Lower layers reach up.** `runtime` (the lambda ABI) imports
   `execution::report` for `LogEntry`. The authoring model imports `builtin`
   for the `RunSinks` interface. `disk_store` imports the compile artifact's
   `ExecutionNode` to read one flag. The blob-format errors live in
   `data::codec`.
6. **A func node's signature is optional data on every node.** `Node.signature:
   Option<FuncSignature>` is `None` for specials, and for func nodes only for
   documents saved before signatures existed (a legacy path that AGENTS.md
   forbids). Every validation and compile also re-hashes the signature of
   each func node's declaration.
7. **Event edges do not use the event address.** `EventPort` exists, but
   `Subscription` and the `Graph` subscription methods still take
   `(emitter, event_idx, subscriber)`, and `EventPort` lacks the derive list
   its two siblings carry.

### Target design

- **Module graph (acyclic across layers):** `containers`, `data` → `graph`
  (authoring model, special-node specs included) → `runtime` (lambda ABI,
  `LogEntry`/`LogLevel`) → `library` → `builtin` → `execution` → `worker`.
  Inside `execution`: `index` → `compile` → `schedule` → `cache` → `resolve`
  → `executor` → `engine`. The edges that `graph` ↔ `runtime` share through
  `identity`/`func::lambda` stay: they are one ABI and one concept.
- **`execution::schedule`** is pure data plus the planner. It owns the one
  input-delivery rule as `RunSchedule::delivery(program, input) ->
  Delivery<'_>` with the cases `Unbound`, `Blocked` (bind to a
  `MissingInputs` producer), `Const(&ConstValue)` and `Bind(OutputAddr)`.
  `input_missing`, the digest fold and `collect_inputs` match on it.
  `compile` no longer knows `NodeState`.
- **`execution::resolve`** (new) holds the cache-aware sweep that is now
  `RunSchedule::resolve`. It depends on `schedule` and `cache`. Neither of
  them depends on it.
- **`execution::cache`** keeps output values, digests, the disk store, the
  eviction cone and RAM measurement. `RuntimeSlot` loses `owner`, `state`
  and `event_state`. The path memo becomes `PathIdentities` in the
  `stamp_job` module, held by the cache and read by `node_digest`.
- **Node runtime state** (`owner`, `state`, `event_state`) is a
  `Column<NodeIdx, NodeRuntime>` on the `Executor`. `ExecutionEngine::install`
  realigns it and the cache slots with one merge walk over the two sorted
  `node_ids` columns. The per-install `HashMap` in `RuntimeCache::reconcile`
  goes away.
- **`NodeKind::Func { func_id, signature }`** carries the signature, and
  `NodeKind::Special` does not. The `Library` computes each func's signature
  once at `add` and keeps it beside the func.
- **`Subscription { event: EventPort, subscriber: NodeId }`**, and
  `Graph::subscribe`/`unsubscribe`/`is_subscribed` take `(EventPort, NodeId)`.

## Plan

### 7. Event addressing

Files: `src/graph/identity.rs`, `src/graph/subscription.rs`,
`src/graph/mod.rs`, `src/execution/compile/mod.rs`, plus the subscription
sites in `darkroom` (`core/edit/graph_intent`, `core/edit/step/
set_subscription.rs`, `core/edit/validate.rs`, `gui/pane/graph_ui/gesture/
subscription_ui.rs`).

- [ ] `identity.rs:53` `EventPort` — misses `Serialize` and `Deserialize`,
  which breaks the module's own "one derive list, in one order" rule for the
  three port addresses. Target: the same derive list as `OutputPort` and
  `InputPort`.
- [ ] `subscription.rs:12` `Subscription { emitter, event_idx, subscriber }`
  and `graph/mod.rs:251`, `:361`, `:368` (`is_subscribed`, `subscribe`,
  `unsubscribe` with three loose arguments) — event edges do not use the
  event address, unlike `Binding::Bind(OutputPort)`. Target: `Subscription {
  event: EventPort, subscriber: NodeId }`, ordered by `event` then
  `subscriber`, and the three methods take `(EventPort, NodeId)`. The
  compile walk's subscriber merge compares `sub.event` with
  `EventPort::new(node_id, event_idx)`.

### 8. Blob store I/O

Files: `src/execution/cache/disk_store/blob_target/mod.rs`,
`src/execution/cache/disk_store/mod.rs`.

- [ ] `blob_target/mod.rs:169` `BlobTarget::store` — runs
  `fs::create_dir_all(parent)` before every write, which is one or more
  syscalls per stored node per run under an event loop. The parent is always
  the store root. Target: create the root once when the store is attached,
  and on a `NotFound` from `AtomicFile::new` (the root was deleted) create it
  and try once more.

### 9. Error placement

Files: `src/execution/error.rs`, `src/worker/error.rs`,
`src/worker/worker_task/mod.rs`.

- [ ] `execution/error.rs:32` `Error::EventLambdaPanic` — an event-loop
  failure that only the worker produces (`worker_task/mod.rs:411`), carried
  in the engine's run error and wrapped as `WorkerError::Execution`
  ("execution failed"). Target: `WorkerError::EventLambdaPanic { node_id,
  message }`, and remove the variant and its doc mention from `Error`.

### 10. Cleanup

Files: about 30 files that use root paths, plus the files named per item.

- [ ] About 37 `use crate::{…}` and `crate::Type` imports in production code
  (for example `data/dynamic_value.rs`, `library/mod.rs`,
  `execution/executor/mod.rs`) reach items through the `lib.rs` re-exports.
  That is a second path per item. Target: import each item from its home
  module.
- [ ] `execution/compile/mod.rs:331`, `:381` and
  `compile/consumer_cone/mod.rs:77`, `:92`, `:119` — `NodeIdx.0 as usize`
  bypasses `Idx::idx`. Target: `.idx()`.
- [ ] `executor/mod.rs:112`, `:342`, `engine/mod.rs:63` — `&mut dyn
  RunReporter` with one production implementor. Target: a generic `R:
  RunReporter` on `ExecuteRequest`, `RunRequest` and `ExecutionFrame`.
- [ ] `report/execution_outcome.rs:16` `ran_node_count` is copied into
  `RunSummary::executed_node_count` (`run_summary/mod.rs:49`). One fact has
  two names. Target: `executed_node_count` on both.
- [ ] `graph/func/error.rs:19`, `:21` `FuncValidationError::NilId` and
  `PureWithoutOutputs` — the only variants without `func_id`, and the second
  message starts with a capital letter. Target: carry `func_id` and match the
  message style of the other variants.
- [ ] `library/type_entry.rs` `TypeEntryKind` — both variants repeat
  `display_name`. Target: `TypeEntry { display_name, kind }`, with the kind
  holding only the codec or the variants.
- [ ] `graph/serde/mod.rs:26` `serialize_bindings` collects the map into a
  `Vec` to serialize it. Target: `serializer.collect_seq(map)`.
