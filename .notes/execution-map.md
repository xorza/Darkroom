# `scenarium/src/execution` — ownership map, call graph, redesign notes

Snapshot of commit `71b3bfe3`. Sizes are `wc -l` (doc comments and gated
`internals`/`tests` mods included).

| | files | lines |
| --- | --- | --- |
| production files | 27 | 6 650 |
| dedicated `tests.rs` | 13 | 11 531 |
| total | 40 | 18 181 |

Biggest production files: `disk_store/format` 616, `flatten` 599, `executor`
573, `resource` 533, `cache/runtime` 497, `compile` 418, `engine` 374,
`validate` 365.

---

## 1. Ownership map

Everything below is owned by value unless marked. `Arc` marks the one shared
edge; `→` marks a borrow taken per call.

```
host thread
└── Compiler                                     [compile/mod.rs]
    └── flattener: Flattener                     [flatten/mod.rs]
        ├── path: Vec<NodeId>                    reusable descent scratch
        ├── scope_stack: Vec<u32>
        ├── seen_shared: HashSet<GraphId>
        ├── subs: Vec<PendingSubscription>
        ├── pending_binds: Vec<PendingBind>
        └── e_nodes: Vec<(ExecutionNodeId, ExecutionNode)>
   (per build only: Run<'a> — borrows all of the above + `levels: Vec<&Graph>`)

     ── Arc<CompiledGraph> crosses the worker channel ──

worker thread (WorkerTask)
├── outcome: ExecutionOutcome                    [outcome.rs]  retained buffer
└── ExecutionEngine                              [engine/mod.rs]
    ├── compiled: Arc<CompiledGraph>             [compile/mod.rs]  replaced wholesale
    │   ├── program: ExecutionProgram            [program/mod.rs]
    │   │   ├── e_nodes:      NodeColumn<ExecutionNode>
    │   │   ├── e_node_ids:   NodeColumn<ExecutionNodeId>
    │   │   ├── e_node_index: HashMap<ExecutionNodeId, NodeIdx>
    │   │   ├── inputs:  Pool<ExecutionInput>     ← ExecutionBinding{None,Const,Bind(OutputAddr)}
    │   │   ├── events:  Pool<ExecutionEvent>
    │   │   └── outputs: Pool<ExecutionOutput>
    │   ├── flatten_map: FlattenMap              [flatten/map.rs]
    │   │   ├── scopes:  Vec<Scope>              instance ancestry
    │   │   ├── leaves:  HashMap<ExecutionNodeId, Leaf>
    │   │   └── exposed: Vec<(NodeId, ExecutionNodeId)>
    │   ├── node_lists: Pool<NodeIdx>            shared backing for the 3 relations below
    │   ├── footprints: HashMap<NodeId, PoolRange<NodeIdx>>   authored → execution nodes
    │   ├── consumers:  NodeColumn<PoolRange<NodeIdx>>        reversed data edges
    │   └── exposed:    HashMap<NodeId, PoolRange<NodeIdx>>   instance → exposed producers
    │
    ├── cache: RuntimeCache                      [cache/runtime/mod.rs]  CROSS-RUN
    │   ├── slots: NodeColumn<RuntimeSlot>       [cache/slot.rs]
    │   │   └── RuntimeSlot { owner: StateOwner, state: AnyState,
    │   │                     event_state: SharedAnyState,
    │   │                     current_digest: Option<Digest>,
    │   │                     value: ValueState::{Empty, Resident{OutputSnapshot, produced_under}} }
    │   ├── e_node_ids: NodeColumn<ExecutionNodeId>   ⚠ duplicates program.e_node_ids
    │   ├── disk_store: DiskStore                [cache/disk_store/]  survives installs
    │   │   ├── codecs: Codecs                   [codec.rs] TypeId → Arc<dyn CustomValueCodec>
    │   │   └── disk_root: Option<PathBuf>
    │   ├── stamper: ResourceStamper             [cache/resource/]
    │   │   ├── fs_paths: HashMap<String, FsPathId>   per-run memo
    │   │   └── job: StampJob {requests, stamped, files, pending}  moves to blocking pool
    │   └── ram_seen: HashSet<usize>             dedup scratch for RAM accounting
    │
    ├── planner: Planner                         [plan/mod.rs]  PER-RUN scratch
    │   ├── color: NodeColumn<Color>
    │   └── stack: Vec<Visit>
    ├── plan: ExecutionPlan                      [plan/mod.rs]  PER-RUN, buffer reused
    │   ├── process_order: Vec<NodeIdx>
    │   ├── verdicts: NodeColumn<NodeVerdict>    {Execute, Disabled, MissingInputs}
    │   ├── roots: NodeSet
    │   ├── seeded: NodeSet
    │   └── event_sources: NodeSet
    ├── resolver: Resolver                       [resolve/mod.rs]  PER-RUN
    │   ├── disposition: NodeColumn<Disposition> {Cut, Reuse, MissingLambda, Run}
    │   └── outputs: ResolvedOutputs { demand: OutputColumn<OutputDemand>,
    │                                  readers: OutputColumn<u32> }
    └── executor: Executor                       [executor/mod.rs]  PER-RUN
        ├── ctx_manager: ContextManager          [runtime/context.rs] cancel, logs, contexts
        ├── inputs: Vec<DynamicValue>            per-invoke scratch
        ├── remaining_reads: RemainingOutputReads(OutputColumn<u32>)
        └── outcomes: NodeColumn<NodeOutcome>    {Pending, Reused, Cut, Ran, Failed, Skipped}

per-run transients (never stored):
  RunRequest<'a,'r>   6 borrows   engine → Executor::run
  ExecutionFrame<'a,'r> 11 borrows  Executor::run → per-node methods
```

### Lifetime tiers

| tier | lives for | members |
| --- | --- | --- |
| immutable artifact | one install | `CompiledGraph` (program, flatten map, 3 relations) |
| cross-run | many installs | `RuntimeCache.disk_store`; slot `state`/`event_state`/`value` (re-paired by id at `reconcile`) |
| per-run | one `execute` | `ExecutionPlan`, `Resolver`, `Executor.outcomes`, `ResourceStamper.fs_paths`, `remaining_reads` |
| per-invoke | one lambda | `Executor.inputs`, `InvokeSlot` |

### Per-node state is spread across 8 parallel structures

For one `NodeIdx` a run touches: `program.e_nodes[i]`, `program.e_node_ids[i]`,
`cache.slots[i]`, `cache.e_node_ids[i]`, `plan.verdicts[i]`, `plan.roots/seeded/
event_sources` (3 bitsets), `resolver.disposition[i]`, `executor.outcomes[i]` —
plus two output-indexed columns (`demand`, `readers`) and one more
(`remaining_reads`). Eleven index spaces aligned by convention and checked by
`validate.rs`.

---

## 2. Call graph

### Install path — host thread, synchronous, infallible after `compile`

```
Compiler::compile(graph, library)
├── Graph::validate_for_execution(library)                        [graph/validate.rs]
├── Flattener::build(program, root, library, flatten_map)
│   └── Run::emit(ancestor_disabled)                              ◀── recursive
│       ├── Run::emit_instance → push_level → FlattenMap::push_scope → emit
│       ├── Run::execution_node_id → ExecutionNodeId::from_authoring   (BLAKE3)
│       ├── Pool::append × {outputs, events, inputs}
│       ├── FlattenMap::set_leaf
│       ├── Run::typed_binding → resolve_binding → Run::resolve   ◀── recursive
│       │      └── resolve_exposed_output → typed_boundary_binding
│       ├── Run::record_exposed_outputs → FlattenMap::push_exposed
│       └── Run::collect_subscriptions → resolve_emitter / resolve_subscriber
│   └── ExecutionProgram::adopt_flattened
│       ├── adopt_nodes (sort by id) → push
│       ├── intern_bindings   ExecutionOutputPort → OutputAddr    ◀── only id hashing
│       └── apply_subscriptions
├── ExecutionProgram::resolve_output_types(library) → OutputTypeResolver::resolve
├── CompiledGraph::indexed(program, flatten_map)
│   ├── FlattenMap::attribution × N  → pack_groups → footprints
│   ├── FlattenMap::exposed_producers → pack_groups → exposed
│   └── binding sweep → consumers
└── CompiledGraph::validate_debug(library)                        [validate.rs]

ExecutionEngine::install(Arc<CompiledGraph>)
├── RuntimeCache::reconcile(program)
│   ├── RuntimeSlot::reown(StateOwner)          drops state on impl change
│   └── RuntimeCache::release_dead_outputs
└── CompiledGraph::validate_installed_debug(cache)                [validate.rs]
```

### Run path — worker thread, `async`

```
ExecutionEngine::execute(seeds, reporter, cancel, outcome)
├── outcome.clear()                                              ⚠ done again in Executor::run
│
├─ PHASE 2 ── Planner::plan(compiled, seeds, plan)
│   ├── ExecutionPlan::reset_for_program
│   ├── collect_roots(compiled, seeds, plan)      → roots, seeded, event_sources
│   │     └── SpecialNode::RunSinks promotes a fired event to a full sinks run
│   ├── Planner::walk_backward_collect_order      → process_order, verdicts
│   │     └── plan::input_missing(input, verdicts)
│   └── ExecutionPlan::validate_debug(program)
│
├─ PHASE 2a ── RuntimeCache::prepare(program, plan, cancel)
│   └── ResourceStamper::identify
│       ├── request_node_paths × executing → request_fs_paths
│       └── ResourceStamper::prepare → spawn_blocking(StampJob::run)
│           └── StampJob::stamp → stamp_directory → collect_files
│
├─ PHASE 2b ── Resolver::resolve(program, plan, cache)
│   ├── RuntimeCache::stamp_digests → stamp_digest       producer-first
│   │     └── ResourceStamper::node_digest
│   │           ├── digest::hash_data_type / hash_static
│   │           ├── ResourceStamper::hash_fs_paths
│   │           └── ResourceStamper::hash_bound_fs_path   (None ⇒ late restamp)
│   └── reverse sweep over process_order → disposition, demand, readers
│         └── RuntimeCache::probe_reuse
│             ├── is_resident_hit → current_snapshot → OutputSnapshot::covers_demand
│             └── DiskStore::covers_demand → format::covers_demand
│                   └── read_header → scan_header → read_prefix / read_descriptor
│
├─ PHASE 3 ── Executor::run(RunRequest, outcome)
│   ├── RemainingOutputReads::seed(resolver)
│   ├── for node_idx in plan.process_order:
│   │   ├── task::yield_now
│   │   ├── cancel? → ExecutionFrame::retire_cancelled_tail → abandon_input_reads
│   │   └── ExecutionFrame::run_node(node_idx)      ◀── dispatch on Disposition
│   │       ├── Cut           → RuntimeCache::is_resident_current
│   │       ├── MissingLambda → outcomes::mark_skipped
│   │       ├── Reuse         → ExecutionFrame::serve_reuse
│   │       │                     └── RuntimeCache::hydrate_reuse
│   │       │                           └── DiskStore::read → format::read → codec.decode
│   │       └── Run           → ExecutionFrame::needs_invoke
│   │           ├── RuntimeCache::restamp_and_hydrate     late 2nd chance at reuse
│   │           │     └── identify → stamp_digest → hydrate_reuse
│   │           └── ExecutionFrame::invoke_node
│   │               ├── outcomes::has_errored_dependency → mark_skipped
│   │               ├── ExecutionFrame::collect_inputs
│   │               │     ├── RuntimeCache::read_output_port(take?)
│   │               │     └── complete_planned_read → clear_output_port
│   │               ├── RuntimeSlot::invoke_slot → FuncLambda::invoke   ◀── user code
│   │               ├── RunReporter::progress ×2 (Started / Finished)
│   │               ├── RuntimeSlot::unbound_demanded_outputs
│   │               ├── RuntimeSlot::stamp_produced | clear_output
│   │               ├── collect_event_triggers → EventTrigger
│   │               ├── RuntimeCache::store_node → DiskStore::store → format::write
│   │               └── release_drained_outputs
│   └── outcomes::collect_execution_outcome(program, plan, outcomes, start, outcome)
│         └── plan::input_missing  (shared with the planner)
│
├── RuntimeCache::release_dead_outputs(program)
├── RuntimeCache::resident_ram_stats(&mut outcome.node_ram)
└── outcome.triggered_events ← seeds.events
```

### Module dependency graph (production `use` edges only)

```
LEAF      identity   pool   codec   event   seeds   report   digest
  │          ▲        ▲       ▲       ▲       ▲       ▲        ▲
index ──────────────────────────────────────────────────────── slot
  ▲ └──▶ program (⇄ index)                                      ▲
program ──▶ identity, index, pool                     resource ─┘──▶ program, index
  ▲                                                   disk_store ──▶ digest, slot, codec, identity, program
  │                                                     │  └─ format ──▶ digest, codec
flatten::map ──▶ identity                             cache::runtime ──▶ digest, disk_store, resource,
flatten ──▶ map, identity, program                        slot, identity, program, index, plan⚠, outcome⚠
compile ──▶ flatten, map, identity, program, index, pool        ▲
plan ──▶ compile⚠, error, program, index, seeds                 │
resolve ──▶ cache::runtime, plan, program, index ───────────────┘
executor::outcomes ──▶ runtime, error, identity, outcome, plan, program, index
executor ──▶ + disk_store, event, report, resolve            (12 edges)
engine ──▶ 14 modules
validate ──▶ runtime, slot, compile, flatten::map, identity, plan, index, program   ⚠ cross-layer hub
```

≈ 92 intra-module edges over 21 modules. `⚠` marks the four edges that make the
graph non-layered; see §3.

---

## 3. Redesign findings

Ordered by (value ÷ risk). Line estimates are net deletions.

### Tier 1 — mechanical, no behavior change (≈ −120 lines, 4 edges + 1 cycle removed)

**T1.1 `plan` → `compile` is spurious.** `Planner::plan` takes
`&CompiledGraph` and immediately does `let program = &compiled.program`;
`collect_roots` takes `&CompiledGraph` and does the same. Neither touches
`flatten_map`, `footprints`, `consumers`, or `exposed`. Change both signatures
to `&ExecutionProgram` and the whole `plan → compile` edge disappears — `plan`
then sits directly on `program`, below `compile` instead of above it. This is
the single edge that forces `compile` (a host-side concern) into the run-side
layering.

**T1.2 `cache::runtime` → `plan` is spurious.** `stamp_digests(program, plan)`
and `prepare(program, plan, cancel)` both use `plan` only to compute
`process_order.iter().filter(|i| verdicts[i].wants_execute())`. Take
`impl Iterator<Item = NodeIdx>` instead. The caller (`Resolver`/`engine`)
already holds the plan.

**T1.3 `cache::runtime` → `outcome` is spurious.** The only use is
`resident_ram_stats(&mut Vec<NodeRamUsage>)`. Have it yield
`(ExecutionNodeId, RamUsage)` (or fill the vec in `engine`, which owns the
outcome). With T1.2 this leaves `cache` depending on nothing above
`program`/`identity`/`codec` — a genuine leaf subsystem, which is what the
module doc already claims it is ("Per-run results are *not* here").

**T1.4 `program::index` ⇄ `program` cycle.** `index/mod.rs` imports
`program::OutputRange` for exactly two methods (`OutputColumn::slice`,
`slice_mut`). Make them generic over the pool marker
(`fn slice<M>(&self, r: PoolRange<M>) -> &[T]`) and the import — and the only
import cycle in the subsystem — goes away.

**T1.5 Split `validate.rs` (365 lines) into `compile/validate.rs` and
`plan/validate.rs`.** It is a leaf module that reaches *up* into four layers
(`cache`, `compile`, `flatten::map`, `plan`) purely because three unrelated
validators share a file. Each half only needs what it validates. Removes the
one cross-layer hub node from the graph at zero behavioral cost.

**T1.6 `node_digest` lives in the wrong module.** `cache/digest/mod.rs` (231
lines) holds only hasher primitives; the actual `node_digest` /
`hash_bound_fs_path` / `hash_fs_paths` are methods on `ResourceStamper` in
`cache/resource/mod.rs`, because they read its `fs_paths` memo. The
consequence is that `digest`'s own module doc has a **broken intra-doc link**
to `[node_digest]`, as does `slot.rs:76`. Moving `node_digest` back into
`digest` and passing `&ResourceStamper` (or a narrow `fn(&str) -> Option<&FsPathId>`)
restores "digest owns the digest, resource owns filesystem identity" and cuts
`resource` by ~110 lines.

**T1.7 Double `outcome.clear()`** — `engine::execute:97` and
`executor::run:126`. Delete one.

**T1.8 `Error::EventLambdaPanic` is never constructed by `execution`.** Its
only producer is `worker/task.rs:316`. It sits in `execution::error::Error`,
which the doc describes as "the error type of the engine's `Result`-returning
entry points" — the engine cannot return it.

### Tier 2 — small structural changes (≈ −220 lines, 2 types removed)

**T2.1 Delete `RuntimeCache::e_node_ids`.** It is a verbatim copy of
`program.e_node_ids`, rebuilt every install. Three readers: `reconcile` (needs
the *previous* ids), `validate_installed` (compares the two columns
element-wise), `resident_ram_stats` (zips to recover ids). The engine still
holds the previous `CompiledGraph` at install time — it just overwrites it
before calling `reconcile`. Reorder to
`self.cache.reconcile(&old.program, &new.program)` and:

- the column and its per-install `NodeColumn` rebuild go,
- `InstalledGraphValidationError::NodeMismatch` goes (after `reconcile` it
  compares a column against the column it was just built from — a tautology at
  its only production call site),
- `resident_ram_stats` takes `&ExecutionProgram`, which it needs for T1.3 anyway.

**T2.2 Collapse `is_sink` + `is_impure` into one lookup.** Both are
`footprint(node_id)` + `.iter().any(..)` + the same `Option` contract ("`None`
where the node covers no compiled work"). `darkroom/src/gui/scene/mod.rs:526`
and `:531` call them back to back per node per frame — two hash lookups and
two footprint walks for one answer. One `fn node_facts(&self, NodeId) ->
Option<NodeFacts { sink: bool, impure: bool }>` halves that and leaves one
`None` contract to document instead of two. `run_targets` and
`data_consumer_closure` stay as they are.

**T2.3 Unify `NodeColumn<T>` and `OutputColumn<T>`.** Identical bodies,
differing only in the index newtype. One `Column<I: ColumnIdx, T>` removes
~60 lines and one type from the vocabulary; `NodeColumn`/`OutputColumn` stay as
type aliases so no call site changes.

**T2.4 Drop `RunRequest`.** It exists to avoid a six-argument `Executor::run`,
but `run` destructures it on line 1 and re-packs all six fields into
`ExecutionFrame`'s eleven. Since the engine owns every one of them, build the
`ExecutionFrame` in `engine::execute` and let `Executor` be what it already
mostly is — a bag of reusable scratch. One struct, one destructure, and one
lifetime-pair comment disappear.

### Tier 3 — real design change (≈ −250 lines; needs care)

**T3.1 Merge `NodeVerdict` and `Disposition` into one per-node state column.**

Today a run carries two columns whose states are disjoint by construction:
`NodeVerdict {Execute, Disabled, MissingInputs}` written by the planner, and
`Disposition {Cut, Reuse, MissingLambda, Run}` written by the resolver — and
the resolver's first act is to copy one into the other
(`if !verdicts[i].wants_execute() { disposition[i] = Cut }`). The executor then
reads *both* on every node (`run_node` checks the verdict, then matches the
disposition).

The stated reason for the split is that the plan is "structural and reusable
across runs". It is not reused: `engine::execute` calls `planner.plan(..)`
unconditionally on every run — only the *buffer* survives. So the split buys
conceptual separation, not work avoided.

One `NodeState {Disabled, MissingInputs, Cut, Reuse, MissingLambda, Run}`
column would remove a full `NodeColumn` per run, one enum, the `wants_execute`
/`missing_required_inputs` predicate pair, and a good deal of prose explaining
how the two columns interact.

**The hazard, precisely:** the resolver promotes a *producer* to `Run` before
that producer is itself swept, so a later consumer reading the producer's slot
would see `Run` where it currently sees the planner's verdict. That is still
correct — `Run` implies runnable — but two lines must change: the
`disposition[i] = Cut` write for a non-runnable node has to become a plain
`continue` (or it erases the `Disabled`/`MissingInputs` value that
`collect_execution_outcome` later reads to report missing input ports), and the
default must be `Cut` only for nodes the planner marked runnable. Worth doing
only with the `resolve` and `executor` test suites as the gate.

**Cheaper variant, same file:** keep both columns but delete the redundant
`Cut` write and let `run_node`'s existing verdict check carry it. Two lines,
no risk, removes the double encoding without the merge.

**T3.2 Consolidate the plan's three `NodeSet`s.** `roots`, `seeded`, and
`event_sources` are three bitsets over the same index space where `seeded ⊆
roots` and `event_sources ⊆ roots` (both invariants are asserted in
`validate.rs:333-352`). One `NodeColumn<RootKind>` — or one bitset plus two
small `Vec<NodeIdx>` lists, since seeds and event sources are sparse — removes
two allocations per install and the two validation branches that police the
subset relation.

**T3.3 Flatten's `Run<'a>` duplicates `Flattener`.** Nine of `Run`'s eleven
fields are `&'a mut` re-borrows of `Flattener`'s six, spelled out twice with
the same doc comments on both. Making `Run` hold `&'a mut Flattener` plus its
two genuinely per-build fields (`levels`, `program`) removes ~35 lines of
declaration and the drift risk between the two copies.

### Deliberately not proposed

- **The dense-index design** (`NodeIdx`/`OutputIdx`/`OutputAddr`, columns,
  pools) earns its complexity: it is what makes per-run state a memset and every
  edge walk hash-free. Keep it.
- **`disk_store/format`** (616 lines) is a wire format with an explicit version
  and thorough framing validation. Large but irreducible.
- **Splitting `engine::tests.rs`** (6 566 lines, 40 % of the subsystem) is a
  separate exercise from anything above.

### Net effect

Tiers 1+2 are ≈ −340 lines with no behavior change, and turn the module graph
into a clean DAG:

```
identity  pool  codec ─┐
                       ├─▶ program ⇄ index ─┬─▶ digest ─▶ slot ─▶ resource ─▶ disk_store ─▶ cache
flatten ─▶ compile ────┘                    ├─▶ plan ─▶ resolve ─▶ executor ─▶ engine
                                            └─▶ (compile arm stays host-side)
```

with `cache` no longer reaching into `plan`/`outcome`, `plan` no longer
reaching into `compile`, and no import cycle. Tier 3 adds ≈ −250 more and one
fewer per-node column, at real review cost.
