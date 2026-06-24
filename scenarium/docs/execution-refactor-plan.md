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

### Stage 2 — introduce a `Cache` type owning the slots (issues A, B, E)

The central change.

- New `execution/cache.rs` owning `Vec<RuntimeSlot-cache-part>` (cross-run only:
  `state`, `event_state`, `output_values`, `current_digest`, `output_digest`).
- Intent-revealing API: `set_current_digest`, `is_hit(idx) -> bool` (the single
  cache-hit definition), `hydrate(idx, values, digest)`, `store_output(...)`,
  `reconcile(e_nodes)`, `reset_states`.
- `Planner::plan` takes `&Cache` instead of `&Executor` (cuts issue B).
- Move per-run results (`error`, `run_time`) out of the slot into a transient
  run-state column owned by `run()`.
- Fold the duplicated cache-hit predicate (planner + engine `load_disk_cache`)
  into `Cache::is_hit`.

### Stage 3 — slim `ExecutionEngine` (issue D)

- Move disk-cache choreography (`recompute_digests`, `load_disk_cache`,
  `store_to_disk`) behind a cache/disk coordinator so the engine calls
  `load_before_run` / `store_after_run` and the `disk_cache.is_none()` branch
  lives inside.
- Move value queries (`get_argument_values`, `…_with_previews`) to
  `execution/query.rs`.
- Move `validate_built` to `execution/validate.rs` (alongside the planner's
  `validate_for_execution`).

### Stage 4 — unit tests for the core phases (issue G)

Now isolatable:

- `flatten`: hand-built `Graph` → assert `ExecutionProgram` shape.
- `planner`: hand-built `program` + `Cache` snapshot → assert orders/flags.
- `executor`: small program + plan → assert run results.

### Stage 5 — ergonomics + cleanups (issues E, F, H)

- Replace `execute`'s positional bools with a `RunSeeds` struct.
- Extract shared `input_missing(...)` used by planner + executor stats.
- Move `OutputUsage` to `plan.rs`; refresh the stale `#[cfg(test)]` doc comment;
  decide `execution_stats` placement.

## Sequencing rationale

1 first (mechanical, unblocks clean layering). 2 is the keystone — it resolves A,
B, and half of E and unlocks 4. 3 depends on 2 (the cache coordinator). 4 depends
on 2. 5 is incidental cleanup done as files are touched.
