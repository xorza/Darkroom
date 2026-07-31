# Scenarium audit — subgraph leftovers, duplication, unnecessary complexity

Scope: `scenarium/src` (82 files, 26 539 lines). Cross-referenced against
`darkroom`, `lens` and the rest of the workspace for callers.

Baseline: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
is clean; `cargo test -p scenarium --lib --tests --all-features` is 305 passing
in 0.26 s. Nothing below is a warning the toolchain already reports.

The delete-only findings (a dead `GraphId`, a garbled comment, a stale "graph
interface", eight wrong doc references, the test-only public methods), the
duplicate-node-id gap, and the two real duplications (the const tables and the
run-seed shape) have been fixed and removed from this file; what remains is
open.

---

## B. One shape that only looks duplicated

B1 (three literal-fits-port tables) and B3 (`BatchIntent` re-spelling
`RunSeeds`) are resolved. What is left is the pair I flagged as "two identical
lambda enums", and on a closer look it should **not** be merged.

`FuncLambda` and `EventLambda` share a shape — `None | Lambda(Arc<dyn …>)`,
`new`, `is_none`, a hand-written `Debug` — but not a concept:

- an empty `FuncLambda` is a func `Func::validate` **rejects**, and the run loop
  reports it as `RunError::MissingLambda`;
- an empty `EventLambda` is an ordinary event nothing is wired to, filtered out
  by `collect_event_triggers` without comment.

So the emptiness the two types spell identically means "broken" in one and
"fine" in the other. Three unifications were considered and each costs more
than the ~22 duplicated lines it removes:

- **A generic `Lambda<F: ?Sized>`.** `new` cannot be shared without `Unsize`
  (unstable), and `invoke` differs in both argument and return type, so the
  generic carries only the enum, `is_none` and `Debug` — while the project's
  impl-locality rule pulls both `invoke`s into the shared file, putting the two
  concerns back in one place.
- **A `lambda_slot!` macro** in the `id_type!` mould. ~28 lines of macro to
  delete 22 duplicated ones, net +6, and `FuncLambda::is_none` stops having a
  definition to jump to.
- **A trait tying `F` to its call signature.** Two implementors, one method
  each, and the call sites gain a bound to name.

Merging coincidental shape is what would make this worse, so the two stay
apart. The finding stands corrected rather than acted on.

---

## C. Public API with one test-only member left

The rest of this section is resolved. `Graph::serialize`/`deserialize` (with
`GraphDeserializeError`), `Graph::subscribers`, `Graph::validate_debug` and
`Graph::validate_with_debug` are gone; `Graph::find_by_name` moved into the
crate's `#[cfg(test)]` internals, where its fifteen fixture callers live.

Two of the original rows were **wrong**, and both for the same reason — a grep
that assumed the receiver sat on the same line as the call, plus one truncated
result list:

- `RunSeeds::sinks()` is called by `darkroom/src/core/worker.rs:89`.
- `Library::remove` is called by `lens/src/astro/nodes/ml.rs:25` and `:28`,
  written across two lines.

What is left is `RunSeeds::events()`, whose only callers are in
`worker/tests.rs`. It is **kept deliberately**: `RunSeeds::sinks()` and
`RunSeeds::nodes()` are both production-used, so this is the third member of a
three-constructor set rather than stray weight, and deleting one of three
symmetric constructors buys nothing but an asymmetric API and eight struct
literals in the tests.

Verification note for anything else in this file that claims "no production
caller": text search was not reliable here. The authoritative check is deleting
the item and building `--workspace --all-targets --all-features`, which is what
caught both mistakes above.

---

## D. Complexity worth questioning

### D1. Resolved — `NodeRef` is not half-adopted

On a closer look the shape is right and the docs were the defect. Iteration is
the one lookup whose caller does not already hold the id, so it is the one that
has to hand one back — a node stores no id of its own. `find(id)` and
`find_mut(id)` are *given* the id, so returning it would be pure redundancy.

What made it read as inconsistent was `graph/node/mod.rs` claiming "what comes
back from a lookup is a `NodeRef`", which is false for both `find`s. Both that
line and `NodeRef`'s own doc now state the actual rule, and `find` says why it
returns a bare `&Node`.

### D2. Resolved — narrowed past `pub(crate)`, and one row was wrong

`Graph::input_type`, `Graph::input_spec` and the `subscriptions` field have no
caller outside `graph`, so they are now **private** rather than the `pub(crate)`
this row proposed. The struct doc claiming `subscriptions` is visible to "the
passes beside this file, and no wider" was false for the same reason the row
was raised, and is corrected.

`DetachedNode::assert_valid` was a **wrong row**: `graph::detached`'s `super` is
`graph`, not the crate root, so its `pub(super)` is genuinely narrower than
`pub(crate)`. Left alone. The row conflated a file's location with its module
depth — the same class of mistake as the two wrong rows in section C.

### D3. Resolved — four rebindings removed, one was not a defect

`compile/validate.rs`, `engine/mod.rs` (×2) and `compile/tests.rs` are fixed;
the two in `validate.rs` and `tests.rs` existed only to rename a parameter, so
the parameter's own name is used instead. `testing/program.rs:308` was a **wrong
row**: `owner.program` is an owned `CompiledGraph`, so `&owner.program` is an
ordinary borrow.

### D4. Resolved — the engine and worker test files were split

`engine/tests.rs` (5 231 → 3 715 after the harness migration) is now
`engine/tests/`: 18 files, largest 446 lines, with `cache_persistence` split
again into `{frontier, blob_recovery, cache_modes}` over shared fixtures.
`worker/tests.rs` is 2 258 → 1 059.

The relocation proposed above was **not** carried out as written. Every test in
`compile_regressions`, `cycle_detection`, `topology`, `const_bindings`,
`graph_structure` and `cache_persistence` drives the whole install → plan → run
pipeline through `TestEngine`; filing them under `compile/tests.rs` or
`schedule/tests.rs`, whose own tests are unit-level, would misdescribe what they
cover. Splitting by subject *at the engine layer* is the fix that is true.

Three groups did move, because they never touched the engine at all: the five
`batch_intent_*` tests → `worker/batch.rs`, the five `ActiveEventLoop` /
`PauseGate` tests → `worker/event_loop.rs`, and the publisher test →
`worker/status.rs`.

### D5. Removed — the phase handles are gone

`Scheduled<'a>` and `Resolved<'a>`, their `assume` test hatches, and the
`plan → Scheduled → resolve → Resolved → run` chain are deleted. `Planner::plan`
returns `Result<()>`, `resolve` is a method on `RunSchedule` taking the program
it is aligned to, and `RunRequest` carries `program` and `schedule` as two
fields.

What replaces the type-level guarantee: `RunSchedule::validate_debug` is now
asserted after **both** passes rather than only after planning. It already
checked every column's span against the program, so a schedule read against the
wrong program is still caught — at run time in debug rather than at compile
time. Net −79 lines.

### D6. `SpecialNode` costs six mechanisms for one variant

`RunSinks`'s entire behavior is one condition in `collect_roots`
(`schedule/mod.rs:367`). Supporting it takes: the `SpecialNode` enum
(`graph/node/special.rs`), the `SPECIAL_NODES` const slice, a `NodeKind::Special`
variant, a `special: Option<SpecialNode>` field on **every** `ExecutionNode`
(`compiled_graph.rs:84`), a hardcoded `Func` with a no-op lambda
(`elements/run_sinks.rs`), and a matching arm in `compile/validate.rs:31`.

This is plainly built as an extension point, and adding a second variant costs
almost nothing — which is the argument for keeping it. Noted so the ratio is
visible if a second one never arrives.

---

## E. Id hygiene

Three shipped `FuncId` literals are not valid UUIDs and were clearly hand-typed
rather than drawn from `uuidgen`:

| Id | Where | Problem |
|---|---|---|
| `01896910-0790-AD1B-AA12-3F1437196789` | `elements/system_library.rs:14` (Print) | uppercase; version nibble `A` (10) |
| `01896a88-bf15-dead-4a15-5969da5a9e65` | `elements/system_library.rs:30` (To String) | version nibble `d` (13); `dead` is a typed word |
| `01897c92-d605-5f5a-7a21-627ed74824ff` | `elements/worker_events_library/mod.rs:14` (`FRAME_EVENT_FUNC_ID`) | variant bits `0111`, not RFC 4122 |

They have shipped in saved documents, so per project convention they must not
change. Recorded so the pattern isn't copied — every other id in the crate
(`run_sinks`, `Concat`, the `testing.rs` fixtures) is a well-formed v4.

---

## Summary

Nothing here is a correctness bug in the running pipeline; the flat-graph
rewrite landed cleanly and the execution side is coherent. What is left is
sediment:

- **No duplication left worth removing** (B): the two lambda enums share a
  shape and not a meaning; merging them would cost more than it saves.
- **Structural** (D4): resolved.
- D1, D2, D3 and D5 are resolved. Three of their rows turned out to be wrong on
  inspection (`assert_valid`'s visibility, `program.rs`'s borrow, and D1's whole
  premise) — the same over-reading that produced the two wrong rows in C.

What is left is D6 and E, both recorded rather than actionable.
