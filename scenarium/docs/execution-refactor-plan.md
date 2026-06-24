# `execution/` refactor plan

A staged plan to improve isolation, testability, and dependency hygiene of the
`scenarium/src/execution` module. The pipeline architecture
(`compile → plan → execute`) is sound; this plan tightens ownership boundaries
and cuts cross-phase coupling without changing observable behavior.

## Goals

- **One owner per piece of state.** Today the per-node runtime/cache state is
  read and written by three modules (engine, planner, executor).
- **No phase depends on a sibling phase.** Each phase should depend only on the
  shared data contracts (`program`, `plan`, and a new cache type).
- **`execution` self-contained below `worker`.** Remove the
  `execution ↔ worker` dependency cycle.
- **Each phase unit-testable in isolation**, the way `digest.rs` and
  `disk_cache.rs` already are.

## Current state (baseline)

```
mod.rs        ExecutionEngine: orchestration + disk-cache choreography
              + value queries + event-trigger extraction + debug validation
program.rs    immutable IR (e_nodes + SoA pools)   — clean
plan.rs       per-run schedule (orders + flag columns) — clean
planner.rs    scheduling; depends on Executor (✗)
executor.rs   run loop + RuntimeSlot (cache + per-run results)
flatten.rs    authoring Graph → ExecutionProgram     — clean deps
digest.rs     content digests        — isolated, unit-tested (the model)
disk_cache.rs on-disk blob store      — isolated, unit-tested (the model)
```

Known issues, in priority order:

- **A.** `RuntimeSlot` fields are written by engine (`recompute_digests`,
  `load_disk_cache`), executor (`run`), and read by planner (`resolve_node_flags`).
  No single owner; cache-hit predicate duplicated. Slot also mixes cross-run
  cache with per-run results (`error`, `run_time`).
- **B.** `Planner::plan` takes `&Executor` only to read cache state → phase→phase
  dependency.
- **C.** `EventRef` / `EventTrigger` are defined in `worker` but used by
  `execution` (mod.rs, planner.rs, execution_stats.rs) → `execution ↔ worker`
  cycle.
- **D.** `mod.rs` (`ExecutionEngine`) carries ~4 responsibilities.
- **E.** Duplicated semantics: cache-hit predicate (planner + engine), missing-
  input check (planner + executor).
- **F.** `execute(terminals: bool, event_triggers: bool, …)` is boolean-blind.
- **G.** `flatten` / `planner` / `executor` have no colocated unit tests; all
  coverage rides on the 2975-line `execution/tests.rs`.
- **H.** `OutputUsage` lives in `mod.rs` (a plan/executor concept); stale
  `#[cfg(test)]` doc comment; `execution_stats` boundary.

## Stages

Each stage compiles, passes the full suite, and is independently reviewable.

### Stage 1 — move `EventRef` / `EventTrigger` into `execution` (issue C) ✅ in progress

Mechanical, no behavior change. Removes the `execution ↔ worker` cycle.

- New `execution/event.rs` (`pub(crate) mod event;`) holding both types.
- `worker` imports them from `execution::event`; drop the now-unused
  `serde`, `SharedAnyState`, `EventLambda` imports in `worker`.
- `planner`, `execution_stats`, `execution/tests.rs` import from
  `execution::event`.
- `lib.rs` prelude re-exports them (they appear in the public `ExecutionStats`
  and `active_event_triggers` surface).

### Stage 2 — introduce a `Cache` type owning the slots (issues A, B, E) ✅ done

The central change.

- New `execution/cache.rs` owns `Cache { slots: KeyIndexVec<NodeId, RuntimeSlot> }`.
  `RuntimeSlot` moved here from `executor.rs` and slimmed to cross-run state only:
  `state`, `event_state`, `output_values`, `current_digest`, `output_digest`.
- Intent-revealing API: `reconcile`, `reset_states`, `set_current_digest`,
  `current_digest`, `output_values`, `is_hit(idx) -> bool` (the single cache-hit
  definition), `hydrate(idx, values, digest)`.
- `Planner::plan` now takes `&Cache` instead of `&Executor` — issue B resolved.
  `planner` no longer imports `executor`.
- Per-run results (`error`, `run_time`) moved out of the slot into local columns
  owned by `Executor::run` (reused as scratch across runs). `Executor` now owns
  only `ctx_manager` + invoke/result scratch.
- The cache-hit predicate (was duplicated in planner + engine `load_disk_cache`)
  is now `Cache::is_hit` alone.
- `node_error_propagates_to_dependents` rewritten to read errors from
  `stats.node_errors` (the per-run channel) rather than the slot.
- New colocated unit tests in `cache.rs` for `is_hit` (full truth table) and
  `hydrate`. Full suite: 174 passing.

### Stage 3 — slim `ExecutionEngine` (issue D) ✅ done

- `recompute_digests` moved onto `Cache` (it's RAM-cache digest refresh, runs
  every `update`).
- Disk choreography moved behind a `DiskCacheLayer` coordinator in
  `disk_cache.rs`: `load_into` (disk→RAM at `update`) and
  `pending_persists` + `store_pending` (RAM→disk after a run). The
  `disk_cache.is_none()` branch lives inside it; the engine just calls the
  coordinator. The store is split sync-collect / async-write so the non-`Sync`
  `Cache` borrow never crosses an await (the run future stays `Send`).
- Value queries + event-trigger extraction (`get_argument_values`,
  `…_with_previews`, `active_event_triggers`) moved to `execution/query.rs` as an
  `impl ExecutionEngine` block (child module → reads the private fields).
- Both debug validators moved to `execution/validate.rs` as free fns
  `validate::built` (was `validate_built`) and `validate::schedule` (was the
  planner's `validate_for_execution`).
- Trivial `set_current_digest` setter dropped (per no-trivial-accessors).
- `mod.rs` shrank from ~454 to 291 lines; `planner.rs` from 410 to 327. The
  engine is now orchestration only: error/value types, struct, accessors, state,
  `update`, `execute`. Full suite: 174 passing.

### Stage 4 — unit tests for the core phases (issue G) ✅ done

Colocated `#[cfg(test)]` modules added where there were none (+10 tests, suite
174 → 184):

- `flatten.rs`: `flatten_id` pure-function tests — identity at top level (the
  cache-survival invariant), plus determinism and path/interior sensitivity.
  Fuller composite flattening stays covered by the integration suite.
- `planner.rs`: hand-built program + `Cache` snapshot (no executor needed) →
  post-order ordering, the cached-consumer-prunes-producer case (the one a plain
  `filter(wants_execute)` gets wrong), required-vs-optional missing-input
  propagation, fan-out `output_usage` counts, and cycle rejection.
- `executor.rs`: hand-built program with real `async_lambda!` lambdas + a
  hand-built plan (no planner needed) → bind resolution + output storage, and
  upstream-error-skips-dependent with output cleared. Each phase is now testable
  without the others, so a bug in one can't mask a bug in another.

### Stage 5 — ergonomics + cleanups (issues E, F, H)

- Replace `execute`'s positional bools with a `RunSeeds` struct.
- Extract shared `input_missing(...)` used by planner + executor stats.
- Move `OutputUsage` to `plan.rs`; refresh the stale `#[cfg(test)]` doc comment;
  decide `execution_stats` placement.

## Sequencing rationale

1 first (mechanical, unblocks clean layering). 2 is the keystone — it resolves A,
B, and half of E and unlocks 4. 3 depends on 2 (the cache coordinator). 4 depends
on 2. 5 is incidental cleanup done as files are touched.
