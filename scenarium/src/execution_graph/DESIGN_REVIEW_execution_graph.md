# Design review: scenarium/src/execution_graph  (2026-04-23)

## Current design

`ExecutionGraph` is a mirror of the authored `Graph` annotated with per-run
state (what wants to execute, what was cached, what errored). A run is three
phases: `update()` syncs `e_nodes` with the graph via `CompactInsert` (so
node indices are stable across edits and bindings carry a resolved
`target_idx`), `prepare_execution()` does two backward DFS walks with a
forward pass sandwiched between them, then `run_execution()` invokes
lambdas sequentially in topological order. State cached between phases
(not just across frames): `ProcessState` on each node is repurposed as a
traversal marker in every pass; `wants_execute`/`cached`/`inputs_updated`
are computed in the forward pass and consumed by the second backward
pass and by `collect_inputs`; `usage_count` on outputs is produced in
pass 1 and consumed in `collect_output_usage`.

The load-bearing decisions are: (1) indices, not ids, are the canonical
way to refer to a node — every `ExecutionBinding::Bind` carries
`target_idx`, which means any `compact` reshuffle must rewrite
bindings (currently it doesn't — `CompactInsert` happens to preserve
indices, an invariant the code depends on silently); (2) `ExecutionNode`
owns both structural data (`inputs`, `outputs`, `lambda`) and per-run
scratch state (`wants_execute`, `cached`, `process_state`), with no type
separating the two; (3) cache validity is derived only from
"inputs changed?", never from "output changed?" — a re-execution of an
Impure source always cascades downstream.

## Overall take

The core approach — mirror the graph, resolve bindings to indices,
walk the DAG once in each direction — is sound and well-suited to the
current scale. The weaknesses are in *state shape*, not algorithm:
`ProcessState` is overloaded across four distinct phases, several
fields are per-run scratch that leaked into the persistent struct,
and one entire backward walk is redundant with the forward pass's
output. Fix the state shape and the file shrinks by ~80 lines without
changing semantics. The biggest *feature* gap (change pruning /
backdating) is real but architectural — separate from the shape
cleanups and worth its own decision.

## Findings

### [F1] The second backward pass ~~duplicates a trivial filter~~ *is load-bearing — name it accordingly* (revised 2026-04-23)

- **Category**: Abstraction / naming
- **Impact**: 2/5 — the pass does real work; it just looks redundant because it's named like a traversal and the forward-pass output looks like a superset. Clarity fix only.
- **Effort**: 1/5 — rename + comment, no behavior change
- **Current**: `walk_backward_collect_execute_order` (mod.rs:691–749) re-DFSes from terminals, pruning at `!wants_execute` (line 724). I initially believed (and `NOTES-AI.md` §"Second Backward Pass is Unnecessary" claims) that this equals `e_node_process_order.filter(wants_execute)`.
- **Problem — the refactor is unsafe**: Test `once_node_toggle_refreshes_upstream` (tests.rs:638–679) directly contradicts the claim. When `mult` is Once-cached, `sum.wants_execute = true` (test asserts `!sum.cached` at line 661) but the expected execute order is `["print"]` only. Pass 2 prunes `sum` because its sole consumer won't read it this run. The forward pass computes `wants_execute` as a local property of each node (Pure/Impure/Once × cache state × inputs_updated); it cannot know "my consumer is Once-cached and will skip me" because that fact flows downstream-to-upstream. Pass 2 is the backward step that propagates "I will be read this run" upward. Replacing it with a filter regresses this optimization.
- **Alternative**: Keep the pass. Rename to something like `prune_to_consumer_needed_order` and add a comment explaining the Once-cached-consumer case it exists to handle, pointing at the regression test. Optionally remove the redundant `e_node_terminal_idx.clear()` in `execute()` (the pass's `.drain()` already empties it) — ~1 LOC, not 60.
- **Recommendation**: Do the clarity fix. Do *not* replace with a filter. `NOTES-AI.md`'s claim about this pass being redundant is wrong and should be removed or corrected there too.

### [F2] `ProcessState` is overloaded across three distinct phases

- **Category**: State / Types
- **Impact**: 4/5 — the variant set models *all* traversal phases' states, not any single one's; the traversal code is littered with `unreachable!("should be X")` arms and a stale comment (line 602 says `"should be Forward"` referring to a renamed variant) that exist only because the type can represent states no pass should observe
- **Effort**: 2/5 — one struct field split into two locals, if F1 is taken first
- **Current**: `ProcessState { Unvisited, Visiting, DependenciesResolved, Ready }` (mod.rs:118–129) is stored on `ExecutionNode` and repurposed: `build_execution_nodes` sets `Unvisited → Ready` (line 412), pass 1 drives `Ready → Visiting → DependenciesResolved` with cycle detection, the forward pass writes back `DependenciesResolved → Ready` (line 669), and pass 2 runs the `Ready → Visiting → DependenciesResolved` sequence *again* — a different meaning of "visiting" and "resolved" than pass 1's. `validate_with` and `validate_for_execution` each assert against different subsets of the variants (lines 1003, 1061).
- **Problem**: One field, four meanings, enforced by convention. Every reader has to know which phase is currently active to interpret the state. The `unreachable!` arms are there to paper over the fact that the type models states that shouldn't be observable in a given phase. If F1 is taken, the only remaining use of `ProcessState` is cycle-detection coloring during pass 1 — a use that doesn't need cross-call persistence at all.
- **Alternative**: After F1, delete `ProcessState` from `ExecutionNode`. In `walk_backward_collect_order`, use a local `Vec<Color>` (White/Gray/Black) sized to `e_nodes.len()`, reset per call. Cycle detection is contained in one function; no other phase has to know about it. The `validate_*` asserts on process_state just disappear.
- **Recommendation**: Do it, as a follow-up to F1. The "fragile, relies on caller discipline" framing already in `NOTES-AI.md` §"ProcessState Reuse Across Phases" is right — the fix is to localize the state, not to add more asserts.

### [F3] `ExecutionBinding::Undefined` exists only as a transient within one function

- **Category**: Types / Contracts
- **Impact**: 3/5 — removes a variant that every consumer pattern-matches with `unreachable!`/`panic!` (mod.rs:646, 827, 1043) and that `#[derive(Default)]` wedges into the initial state of every new input
- **Effort**: 2/5 — restructure `update_input_binding` so no intermediate Undefined is ever written
- **Current**: `ExecutionBinding::Undefined` is the `Default` variant (mod.rs:58). It's *only* written at mod.rs:441 as a transient marker during a `Bind → Bind` transition, then overwritten three lines later by the block at 463–478, and finally asserted-away at 480. Consumers (`propagate_input_state_forward`, `collect_inputs`, `validate_*`) all treat it as a logic error.
- **Problem**: A variant that is never supposed to be observed is modelled as a first-class enum case. The three `unreachable!` arms are a contract enforced by prose ("callers must not see this") rather than by the type. The transient use inside `update_input_binding` doesn't even need a type-level marker — it's dead code between two assignments in the same function.
- **Alternative**: Inline the decision: compute the new binding value in a local, then write it once. The (`Bind → Bind` check, then `target_idx` refresh) fuses into one `match` that produces the final binding directly. Delete the `Undefined` variant, drop the `Default` impl on `ExecutionBinding`, remove the three `unreachable!`s. `ExecutionInput::default()` will need its `binding` field initialized explicitly on construction (the only caller is `init_from_func` via `resize(_, ExecutionInput::default())` — which can use `None` as the initial).
- **Recommendation**: Do it. Removes a class of "did someone forget to initialize?" defensiveness.

### [F4] `e_node_terminal_idx` and `stack` are struct fields but are per-call scratch

- **Category**: State / Responsibility
- **Impact**: 2/5 — they're not bugs, but they're serialized-looking state (`HashSet<usize>` in particular appears in `Debug` dumps and in `clear()` as if it's meaningful cross-call)
- **Effort**: 1/5 — move to locals, pass through function args or a `&mut Scratch` struct
- **Current**: `e_node_terminal_idx: HashSet<usize>` and `stack: Vec<Visit>` are fields of `ExecutionGraph` (mod.rs:299, 304). Both are cleared at the top of every use (mod.rs:541, 576, 693) and again in `execute()` (line 509). `clear()` wipes them (mod.rs:342–343). Neither has meaning between calls.
- **Problem**: Their scope is one call, but their lifetime is the struct's. Readers of the struct definition can't tell from the shape which fields are "the graph" and which are "scratch for the last traversal." The double-clear on `e_node_terminal_idx` (pass-2 drain + `execute()` clear) is a direct consequence of not being sure who owns the reset.
- **Alternative**: Either (a) locals in each function — a `HashSet<usize>` allocation per `execute()` is negligible at current scale; or (b) a single private `Scratch { terminals: HashSet<usize>, stack: Vec<Visit> }` field with an explicit "this is reused buffer capacity, not state" comment. (a) is simpler; (b) preserves the capacity-reuse if benchmarks show it matters.
- **Recommendation**: (a). The allocation cost is a few hundred bytes per `execute()`; the clarity gain is real. If F2 is taken, `stack` disappears with `ProcessState` anyway.

### [F5] `ExecutionOutput::usage_count: usize` where a `bool` would do

- **Category**: Types
- **Impact**: 2/5 — it's a small thing, but the `usize` implies the count matters when only `> 0` is read
- **Effort**: 1/5 — rename + replace one `+= 1` with `= true`
- **Current**: `ExecutionOutput { usage_count: usize }` (mod.rs:86–89). Pass 1 increments it per `OutputRequest` visit (mod.rs:589). The only read is `(o.usage_count == 0).then_else(Skip, Needed)` (mod.rs:853). Because pass 1 gates re-visits via `ProcessState`, the same `(e_node, output_idx)` can still be incremented multiple times from different consumers, but the consumer of the count doesn't care.
- **Problem**: The type carries information (a count) that nothing reads. Future readers will reasonably assume the count is load-bearing and preserve it through refactors.
- **Alternative**: `ExecutionOutput { needed: bool }`, set via `|= true`. Or, even simpler, drop the per-output field and compute `needed` from "any downstream input binds to this output and wants execute" during pass 1. The field is denormalized state that could be derived.
- **Recommendation**: Do the rename (bool). Deriving is more churn for less clarity.

### [F6] `pub` fields on `ExecutionNode` leak the prepare/run protocol

- **Category**: Abstraction
- **Impact**: 2/5 — no production caller writes to these fields (worker only reads via `active_event_triggers`); only tests do (42 sites in `tests.rs`)
- **Effort**: 3/5 — touches 42 test sites; needs a test-only accessor shim
- **Current**: `wants_execute`, `cached`, `inputs_updated`, `bindings_changed`, `missing_required_inputs`, `output_values`, `error`, `run_time` are all `pub` (mod.rs:139–153, 160). They're written only by `prepare_execution` and `run_execution`, but the visibility says anyone can mutate them. Tests exploit this (e.g., `by_name_mut("get_b").unwrap().output_values = Some(...)` at tests.rs:450) to fake execution results.
- **Problem**: The invariants these fields carry (e.g., "`wants_execute = false` implies `cached = false` or `output_values.is_some()`") are non-local. `pub` mutation bypasses those invariants. The test use case is legitimate but should be surfaced as an intentional affordance, not as "everything is public."
- **Alternative**: Make these fields `pub(crate)` for read, drop `pub` on mutation paths, and add a narrow `#[cfg(test)] pub fn set_output_values_for_test(...)` — the one poke tests actually need. Low urgency because the worker ignores these fields, but worth doing before any third caller arrives.
- **Recommendation**: Depends. If the module stays internal-only (and the 1M-context AGENTS note suggests aggressive refactoring is fine here), the current state is acceptable. If `scenarium` grows a plugin API, tighten first.

## Rethink

(Not needed — the algorithm is right. F1+F2+F3 together are a coherent
simplification of the state shape, not a redesign. The one place a
redesign might be warranted is change pruning / backdating, but that's
a feature decision — do you want Salsa-style output equality checks? —
not a design flaw in what's here.)

## Considered and rejected

- **Demand-driven (pull) evaluation.** `NOTES-AI.md` lists this as a
  gap. It is, but at current graph sizes the push-based walk is
  simpler and the `OutputUsage::Skip` advisory already buys most of
  the win. Only revisit if graphs grow to hundreds of nodes with
  dormant branches.
- **Parallel execution via Rayon wavefronts.** Same reasoning —
  current scale doesn't justify the added complexity; the sequential
  loop is easy to read and debug.
- **Incremental `update()`.** `invalidate_recursively` exists but
  isn't wired into the normal update path. `build_execution_nodes`
  re-processes all nodes, which `NOTES-AI.md` correctly calls out as
  cheap per-node. The `CompactInsert` already makes this O(n) rather
  than O(n²). Not worth restructuring until profiling shows it.
