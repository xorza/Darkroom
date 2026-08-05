# scenarium — simplification review

Scope: every production file under `scenarium/src`, plus the `darkroom`/`lens`
call sites that constrain the public surface. Baseline: clippy clean
(`--all-targets --all-features -D warnings`), 294 tests green in 0.43s.

Batches are ordered so each is independently shippable and later batches read
against a smaller file set.

---

## Batch 1 — Dead code and the crate's own conventions

Investigated in depth. Every claim below is compiler-verified: delete the item,
`cargo check --workspace --all-targets --all-features`, read the call sites out
of the errors, restore.

### 1.1 Dead and test-only trait impls

Trait impls are invisible to `dead_code` — an impl always counts as used — so
this is the one category the compiler will not sweep for you.

**Delete outright** — no caller in production, tests, or either downstream crate:

| item | where |
|---|---|
| `impl Add for RamUsage` | `data/dynamic_value.rs:19` |
| `impl FromStr for DataType` | `data/type_system.rs:221` |

`FromStr for DataType` is public API, covers only 5 of the 8 variants, and
fails with a bare `Err(())`. Decide which it is: a config-parsing entry point
that needs finishing, or an export that has never had a caller.

**Move into a `#[cfg(test)] mod internals`** — used only by scenarium's own
tests, and the project's rules already put gated `impl` blocks there:

| item | callers |
|---|---|
| `impl From<&str> for DynamicValue` (`dynamic_value.rs:181`) | `elements/system_library.rs` tests |
| `impl From<OutputPort> for Binding` (`graph/mod.rs:539`) | `graph/tests.rs` |
| `impl<I, T> From<Vec<T>> for Column<I, T>` (`common/column/mod.rs:159`) | `executor/tests.rs`, `testing/program.rs` |

**Gate on the feature, not `cfg(test)`** — used only by a *downstream* crate's
tests:

| item | caller |
|---|---|
| `impl From<ConstValue> for Binding` (`graph/mod.rs:545`) | `darkroom/src/core/worker.rs`, inside its `#[cfg(test)] mod tests` |

A plain `#[cfg(test)]` here breaks darkroom's test build — darkroom links
scenarium's non-test build. It needs `#[cfg(any(test, feature = "internals"))]`,
which darkroom already enables.

### 1.2 Nine files broke the project's inline-test split rule — **DONE**

(`tests` past 40% or 150 lines → `foo/{mod.rs, tests.rs}`)

Each was a `foo.rs` → `foo/{mod.rs, tests.rs}` move via `git mv`, so history
follows. Production-file sizes after:

| file | before | after |
|---|---:|---:|
| `testing/graph.rs` | 845 | 552 |
| `worker/task.rs` | 654 | 438 |
| `library.rs` | 638 | 339 |
| `data/type_system.rs` | 391 | 237 |
| `worker/event_loop.rs` | 378 | 180 |
| `worker/status.rs` | 251 | 142 |
| `graph/output_types.rs` | 240 | 131 |
| `worker/batch.rs` | 311 | 95 |
| `graph/serde.rs` | 188 | 84 |

`testing/graph.rs` also gave its 120-line `#[cfg(test)] pub(crate) mod
compiled` its own file — it is the compile-bridge view, a distinct concern from
the fixture builder. `worker/event_loop.rs`'s gated `pub(crate) mod internals`
stayed inline in `mod.rs`: the rule splits out `mod tests`, and
internals-gated code sits at the end of the production file it reaches into.

Verified pure: the identifier-token multiset of each original file equals that
of its replacements, so nothing was lost or added — only braces and
indentation changed. `cargo fmt` + `clippy --all-targets --all-features
-D warnings` clean, 294 tests pass (unchanged), and
`cargo check --workspace --all-targets --all-features` is clean.

### 1.3 Two files use `std::collections::HashMap` — verified safe to switch

`library.rs:3` and `data/codec/mod.rs:5`, where the other seven map users take
`hashbrown`. `Library.funcs` / `Library.types` and `Codecs.by_type` are all
pure lookup maps.

`Library.types` is a **public field**, so this looks like a downstream break.
It isn't: the three external readers (`lens/src/config_node.rs:162,270`,
`lens/src/image/codec/tests.rs:120`) only call `.get()` and `.contains_key()`,
which hashbrown provides with identical signatures. Applying the switch and
running `cargo check --workspace --all-targets --all-features` compiles clean.

Iteration order changes, and that is unobservable: the only order-sensitive
consumer is darkroom's node-add menu (`gesture/new_node/mod.rs:317`), which
sorts by name explicitly. Everything else that walks `funcs()` uses `find`,
`len`, or collects into a map.

---

## Batch 2 — Data-structure consolidation

Four families of near-identical types, one of which pulls in a whole workspace
dependency.

**2.1 Three byte-identical port-address structs.** `graph/identity.rs` hand-
writes `OutputPort` (32), `EventPort` (53), `InputPort` (71): same `NodeId` +
`usize`, same ten derives in the same order, same one-line `new`. The module
doc already names the failure mode this invites — *"a set that drifted between
them would leave a port type missing a bound its siblings have"* — and then
relies on discipline to prevent it.

The crate already has this idiom twice: `::common::id_type!` for the uuid
identities and `crate::common::column::idx_type!` for the dense spaces. A
`port_type!` sibling makes the derive list unforgeable, keeps `pub struct
InputPort` greppable (the doc's stated reason for writing them out —
`idx_type!` already proves a macro-declared type stays greppable), and drops
~45 lines.

**2.2 `FuncLambda` and `EventLambda` are the same type twice.**
`func/lambda.rs:61` and `func/event.rs:14`. Both are
`enum { None, Lambda(Arc<dyn …>) }` with the identical `new` / `is_none` /
`invoke` (panicking on `None`) / hand-written `Debug`, each preceded by the
identical blanket-impl trait-alias dance (`AsyncLambdaFn` / `AsyncEventFn`).
Only the closure signature differs. One `lambda_type!` macro taking the
signature, or a generic `Lambda<Sig>`, removes ~50 lines and one place for the
two to drift.

**2.3 Two containers for "unique, insertion-ordered ids" in the same struct.**
`BatchIntent` (`worker/batch.rs:51-52`) keys `evict_cache`/`flush_cache` on
`IndexSet<NodeId>`, while `RunSeeds` — a field of that same struct — uses
`Vec<NodeId>` + `extend_unique` (`execution/seeds.rs:96`), whose doc explicitly
argues a linear scan beats a hash set at these sizes. Both are right about
their own case only if the sizes differ, and they don't: all three lists are
one burst of worker messages.

Pick one. Choosing `Vec` + `extend_unique` also **removes `indexmap` from the
workspace entirely** — `rg indexmap` across every crate returns exactly
`worker/batch.rs:3`.

**2.4 `DynamicValue` re-declares `ConstValue`'s accessor surface.**
`dynamic_value.rs:80-106` — seven methods (`as_f64`, `as_i64`, `as_bool`,
`as_string`, `as_enum`, `as_fs_path`, `as_fs_paths`) that are each verbatim
`self.as_static().and_then(ConstValue::as_x)`. Adding an accessor to
`ConstValue` silently leaves `DynamicValue` behind. A `forward_accessors!`
macro, or leaving callers to write `.as_static()?.as_f64()`, collapses it.

---

## Batch 3 — One implementation per algorithm

**3.1 Reversed-edge walking exists twice, with very different quality.**
`ConsumerCone` (`compile/consumer_cone/mod.rs`) reverses the data edges with a
counting sort into four flat buffers its owner keeps across calls.
`Graph::produces_cycle` (`graph/mod.rs:247`) does the same job over the
authoring graph by building a fresh `HashMap<NodeId, Vec<NodeId>>` — one
allocation for the map plus one `Vec` per producer node — **on every call**.

It is called from darkroom's `accepts_wire`
(`gesture/connection/mod.rs:415`), i.e. once per frame for the whole duration
of a wire drag. It is the crate's only per-frame allocation storm, and the
cheap fix already exists in-tree: give the reachability walk the `ConsumerCone`
shape, or at minimum hang reusable scratch off the caller so the map is
refilled rather than rebuilt.

**3.2 `validate` / `validate_debug` is written out three times.**
`schedule/mod.rs:519`, `compile/validate.rs:109`, `engine/mod.rs:247` — each is
the same four lines (`if !is_debug() { return; } self.validate().expect(…)`).
A single `debug_validate(result, "…")` helper in `common` removes the
copy-paste and the chance of a fourth forgetting the gate.

---

## Batch 4 — Element libraries and the tag tables

**4.1 `math_library.rs` invented a parameter protocol it didn't need.**
`FloatInputSpec` (10) and `FloatOutputSpec` (17) exist only to be unpacked by
`declared_input` (28) and `declared_output` (34) into the
`FuncInput::required(…).description(…).default(…)` chain every other element
module writes directly. The result is 6-argument `unary_float_func` (38) /
`binary_float_func` (62) and call sites where a one-line port declaration takes
six lines of struct literal.

Take `FuncInput`/`FuncOutput` directly. Two structs, two mapper functions and
roughly 80 lines go, and `math_library` reads like `system_library` and
`worker_events_library` — the point being that the crate currently has two
idioms for declaring a func and no reason for the split.

**4.2 The `ConstValue` tag byte is written in three places with two unrelated
meanings.** `digest/mod.rs:184` (`write_static`) and
`disk_store/format/mod.rs:394/446` (`write_static` / `read_static`) each map the
eight variants onto `0..=7` — the *same numbers*, in the same order, for two
formats that have nothing to do with each other (a digest domain vs. an on-disk
frame). They are exhaustive matches, so the compiler catches a new variant;
what it cannot catch is a reader assuming the two tables are one. Either give
`ConstValue` a single `tag()` both domains fold, or make the numbering visibly
independent.

**4.3 The `DataType` ↔ `ConstValue` correspondence is restated four times** —
`or_const_type` (`type_system.rs:88`), `default_value` (109), `accepts_const`
(174), `write_data_type` (`digest/mod.rs:220`). Each answers a different
question, so this is not straight duplication, but a new type pair means four
edits in two modules and nothing links them. Worth a cross-reference now, a
shared table if a fifth appears.

---

## Batch 5 — File layout: one major struct, one file

The rule is `struct FooBar` → `foo_bar.rs`, with only small satellites riding
along. Outliers, ordered by what they cost a reader:

- **`execution/report.rs`** — eight top-level types (`LogLevel`, `LogEntry`,
  `EventTrigger`, `NodeExecutionStatus`, `NodeStatus`, `ExecutionOutcome`,
  `RunPhase`, `RunProgress`) plus the `RunReporter` trait, and no type named
  `Report`. `ExecutionOutcome` is the major one; `RunReporter` + `RunProgress`
  + `RunPhase` are a separate concern sharing the file.
- **`worker/status.rs`** — five types (`WorkerActivity`, `WorkerStatusKind`,
  `WorkerStatus`, `WorkerStatusPublisher`, `WorkerStatusPatch`).
- **`worker/task.rs` → `WorkerTask`**, **`worker/event_loop.rs` →
  `ActiveEventLoop`**, **`cache/slot.rs` → `RuntimeSlot`** — file name and type
  name disagree.
- **Module-name collision:** `crate::runtime` (`AnyState`, `ContextManager`)
  versus `crate::execution::cache::runtime` (`RuntimeCache`). Two unrelated
  things called "runtime"; imports from both appear in the same files.

Bundle with 1.2 — both are file-splitting work over an overlapping set.

---

## Batch 6 — Delete the counterfactual doc comments

Roughly fifteen comments justify the present design by describing code that no
longer exists. A reader cannot verify them and a future edit will not update
them. Delete the counterfactual, keep the invariant:

- `library.rs:152` "as the old codec registry did"
- `schedule/mod.rs:220` "which the three `pub(crate)` sets this replaced could not manage"
- `schedule/mod.rs:508` "the two … checks here used to establish"
- `schedule/mod.rs:644,649` "the old `seeded` set spelled out" / "the old `event_sources` set"
- `cache/runtime/mod.rs:51` "The cache holding a second handle to the artifact instead made that…"
- `cache/slot.rs:143` "the alternative was every caller stamping the two by hand"
- `engine/mod.rs:292` "the pre-split `update` shape"
- `seeds.rs:68` "a batch that spelled its own four fields had to restate it"
- `testing/program.rs:6` "each place that did grew its own copy of the same two tricks"

"Every root flag carries the root bit" stands on its own; "which the three sets
this replaced could not manage" is a commit message living in a header.

---

## Suggested order

| # | Batch | Risk | Payoff |
|---|---|---|---|
| 1 | Dead impls, gating test-only impls, ~~9 test-file splits~~ (done), hashbrown | none — compiler-verified | 2 dead items gone, 4 impls correctly gated |
| 2 | Port types, lambda types, unique-id containers, accessor forwarding | low | −1 workspace dep, −150 lines, 4 drift surfaces removed |
| 3 | Reversed-edge walk, `validate_debug` helper | low | removes the only per-frame allocation storm |
| 4 | `math_library` specs, tag tables | low | −80 lines, one func-declaration idiom |
| 5 | File-per-struct layout | none | navigability |
| 6 | Counterfactual doc comments | none | ~15 comments |
