# `scenarium/src/execution` — ownership, borrows, call graph, redesign

Supersedes the first pass. Reflects the five changes already landed (plan→compile
edge removed, `ExecutionProgram`→`Program`, cache made a leaf, `RuntimeCache::e_node_ids`
deleted, `NodeVerdict`+`Disposition` merged into `NodeState`).

---

## 1. Ownership

```
host thread
└── Compiler
    └── flattener: Flattener { path, scope_stack, seen_shared, subs, pending_binds, e_nodes }
        └─(per build)─ Run<'a> { library, path:&mut, levels:Vec<&Graph>, scope_stack:&mut,
                                 flatten:&mut, seen_shared:&mut, subs:&mut,
                                 pending_binds:&mut, e_nodes:&mut, program:&mut }

     ── Arc<CompiledGraph> ──▶ worker channel ──▶

worker thread
└── ExecutionEngine
    ├── compiled: Arc<CompiledGraph>
    │     ├── program: Program { e_nodes, e_node_ids, e_node_index, inputs, events, outputs }
    │     ├── flatten_map: FlattenMap { scopes, leaves, exposed }
    │     └── node_lists / footprints / consumers / exposed        (compile-time indices)
    ├── cache: RuntimeCache          CROSS-RUN
    │     ├── slots: NodeColumn<RuntimeSlot>
    │     ├── disk_store: DiskStore { codecs, disk_root }
    │     ├── stamper: ResourceStamper { fs_paths, job: StampJob }
    │     └── ram_seen
    ├── plan: ExecutionPlan          PER-RUN  { process_order, states, roots, seeded, event_sources }
    ├── planner: Planner             PER-RUN scratch { color, stack }
    ├── resolver: Resolver           PER-RUN  { outputs: { demand, readers } }
    └── executor: Executor           PER-RUN  { ctx_manager, inputs, remaining_reads, outcomes }
```

**Five sibling owners of `NodeIdx`-aligned state** — `cache.slots`, `plan.states`,
`resolver.outputs`, `executor.outcomes`, `executor.remaining_reads` — all indexed
by a `Program` that **none of them holds**. That single fact generates most of
what follows.

---

## 2. Borrow graph

### Per-run borrow structs

```
RunRequest<'a,'r>            6 fields, all borrows, destructured on line 1 of Executor::run
    program:   &'a Program
    plan:      &'a ExecutionPlan
    resolver:  &'a Resolver
    cache:     &'a mut RuntimeCache
    reporter:  &'a mut (dyn RunReporter + 'r)
    cancel:    CancelToken

ExecutionFrame<'a,'r>       10 fields, all borrows
    program          &'a Program                     ─┐
    plan             &'a ExecutionPlan                ├─ from the engine, via RunRequest
    resolver         &'a Resolver                     │
    cache            &'a mut RuntimeCache             │
    reporter         &'a mut dyn RunReporter          │
    outcome          &'a mut ExecutionOutcome        ─┘
    remaining_reads  &'a mut RemainingOutputReads    ─┐
    inputs           &'a mut Vec<DynamicValue>        ├─ re-borrowed out of `Executor`
    node_outcomes    &'a mut NodeColumn<NodeOutcome>  │  (self's own fields!)
    ctx              &'a mut ContextManager          ─┘
```

Sixteen borrow-fields across two structs for one loop. Four of `ExecutionFrame`'s
ten are `Executor`'s **own fields**, re-borrowed individually because the loop
needs them disjointly alongside `&mut cache`; `&mut self` would lock them together.

### Self-borrow splits

Three sites destructure `self` to hand one field to a sibling field:

| site | split | why |
| --- | --- | --- |
| `RuntimeCache::prepare` | `let Self { slots, stamper, .. }` | `stamper.identify(.., slots, ..)` |
| `RuntimeCache::restamp_and_hydrate` | same | same |
| `Resolver::resolve` | `let ExecutionPlan { process_order, states, roots, seeded, event_sources }` | read schedule while writing states |

The two cache splits exist because **`ResourceStamper` owns `fs_paths` but not
`slots`**, so the cache must feed its own column back into its own field.

### Threading census (production signatures)

| threaded parameter | occurrences | files |
| --- | --- | --- |
| `program: &Program` (+ `previous`/`installed`) | **33** | runtime 16, plan 5, resource 4, validate 2, executor 2, outcomes 2, resolve 1 |
| `node_idx: NodeIdx` | 31 | everywhere |
| `demand: &[OutputDemand]` | 14 | runtime, slot, disk_store, executor |
| `slots: &NodeColumn<RuntimeSlot>` | 5 | resource only |

`&Program` is the subsystem's universal implicit parameter: 32 of ~90 production
methods take it, and **16 of `RuntimeCache`'s 18** do.

---

## 3. Call graph — signature and reach

`reach` = the state each method actually touches, beyond dispatching.

### `RuntimeCache` (18 methods)

| method | args | reaches |
| --- | --- | --- |
| `clear(&mut self)` | 0 | `slots`, `stamper` |
| `evict(&mut self, program, e_node_ids)` | 2 | `program.e_node_index`, `disk_store`, `slots` |
| `resident_ram_stats(&mut self, program, by_node)` | 2 | `program.e_node_ids`, `slots.value`, `ram_seen` |
| `reconcile(&mut self, previous, installed)` | 2 | both programs' id/node columns, `slots`, →`release_dead_outputs` |
| `current_snapshot(&self, node_idx)` | 1 | `slots[i].{value, current_digest}` |
| `is_resident_current(&self, node_idx)` | 1 | →`current_snapshot` *(proxy)* |
| `is_resident_hit(&self, node_idx, demand)` | 2 | →`current_snapshot`, `OutputSnapshot::covers_demand` |
| `read_output_port(&mut self, program, address, take)` | 3 | `program[i].outputs.len`, `slots[i].value` |
| `clear_output_port(&mut self, address)` | 1 | `slots[i].value` |
| `stamp_digests(&mut self, program, executing)` | 2 | →`stamp_digest` *(loop proxy)* |
| `stamp_digest(&mut self, program, node_idx)` | 2 | **split**: `stamper.node_digest(program, .., slots)` → `slots[i].current_digest` |
| `prepare(&mut self, program, executing, cancel)` | 3 | **split**: `stamper`, `slots` |
| `restamp_and_hydrate(&mut self, program, node_idx, demand, contexts, cancel)` | **5** | **split**: `stamper`, →`stamp_digest`, →`hydrate_reuse` |
| `blob_target(&self, program, node_idx)` | 2 | `program.e_node_ids[i]`, `program[i]`, `slots[i].current_digest`, →`disk_store.blob_target` *(proxy)* |
| `probe_reuse(&mut self, program, node_idx, demand)` | 3 | →`is_resident_hit`, →`blob_target`, `disk_store.covers_demand` |
| `hydrate_reuse(&mut self, program, node_idx, demand, ctx)` | **4** | →`is_resident_hit`, →`blob_target`, `disk_store.read`, `slots[i].value` |
| `store_node(&'a self, program, node_idx, policy, ctx)` | **4** | →`blob_target`, →`current_snapshot`, `disk_store.store` |
| `release_dead_outputs(&mut self, program)` | 1 | `program.e_nodes`, `slots` |

`probe_reuse` and `hydrate_reuse` share the same three-step preamble
(`is_resident_hit` → `blob_target` → disk), differing only in the last call.

### `ResourceStamper` (the `slots`-threading cluster)

| method | args | reaches |
| --- | --- | --- |
| `identify(&'a mut self, program, slots, nodes, cancel)` | **4** | →`request_node_paths`, →`prepare` |
| `request_node_paths(&mut self, program, slots, node_idx)` | 3 | `program[i].behavior/inputs`, `slots[..].output_values`, `job.requests` |
| `node_digest(&self, program, node_idx, slots)` | 3 | `program.{outputs,inputs}`, `slots[..].current_digest`, `fs_paths` |
| `hash_bound_fs_path(&self, hasher, slots, addr)` | 3 | `slots[..].current_output_values`, `fs_paths` |
| `hash_fs_paths(&self, hasher, paths)` | 2 | `fs_paths` |

Every one takes `slots` back from its owner. `node_digest` lives here **only**
because it needs `self.fs_paths` — its own module doc still places it in
`cache::digest`, where the intra-doc link is broken.

### `ExecutionFrame` (the run loop)

| method | args | reaches |
| --- | --- | --- |
| `run_node(node_idx)` | 1 | `program[i]`, `resolver.outputs.demand`, `plan.states` → 6-way match |
| `retire_cancelled_tail(from_process_idx)` | 1 | `plan.{process_order,states}`, →`abandon_input_reads` |
| `serve_reuse(node_idx, demand)` | 2 | `cache.hydrate_reuse`, `ctx.contexts`, `node_outcomes`, →`release_drained_outputs` |
| `needs_invoke(node_idx, demand)` | 2 | `cache.{slots,restamp_and_hydrate}`, `ctx`, `node_outcomes`, →`abandon_input_reads`, →`release_drained_outputs` |
| `invoke_node(node_idx, demand)` | 2 | `program`, `cache.slots`, `ctx`, `reporter`, `outcome`, `node_outcomes`, →`collect_inputs`, →`store_node`, →`release_drained_outputs` |
| `collect_event_triggers(node_idx, event_state)` | 2 | `program.events`, `outcome.event_triggers` |
| `collect_inputs(node_idx)` | 1 | `program.inputs`, `remaining_reads`, `cache.read_output_port`, →`complete_planned_read` |
| `producer_runs(addr)` | 1 | `plan.states` *(one-line proxy)* |
| `abandon_input_reads(consumer_idx)` | 1 | `program.inputs`, →`producer_runs`, →`complete_planned_read` |
| `release_drained_outputs(node_idx)` | 1 | `program[i].cache`, `remaining_reads`, `cache.slots` |
| `complete_planned_read(address)` | 1 | `program.output_idx`, `remaining_reads`, `cache.slots`, →`release_drained_outputs`, →`clear_output_port` |

### The read-accounting cluster — one idea, six methods, two types

```
RemainingOutputReads::is_last / consume / node_drained   (owns `counts`)
ExecutionFrame::complete_planned_read / release_drained_outputs / abandon_input_reads
```

All six implement: *each planned read is completed once; when a producer's last
read lands, release its value unless its cache mode retains it.* They are split
across two types because `counts` lives on `Executor` and the values live on
`RuntimeCache`, so no single owner can express the rule.

---

## 4. Redesign

Four proposals. **P1 and P2 are the ones with real leverage**; P4/P5 are local.

### P1 — `RuntimeCache` holds `Arc<CompiledGraph>`

Every cache method exists to act on slots that are index-aligned to one program,
and re-receives that program to find out which. Give the cache the `Arc` it is
reconciled to:

```rust
struct RuntimeCache { compiled: Arc<CompiledGraph>, slots, disk_store, stamper, ram_seen }

fn install(&mut self, compiled: Arc<CompiledGraph>)   // was reconcile(previous, installed)
```

- **Drops `program` from 16 of 18 signatures** — 16 of the 33 occurrences.
- `reconcile(previous, installed)` → `install(compiled)`: the previous program is
  `self.compiled`, so passing the wrong one stops being expressible. This is a
  strict improvement on the two-program signature introduced when
  `e_node_ids` was deleted — same invariant, one fewer thing to get right.
- The alignment invariant becomes structural instead of a `debug_assert`.
- Cost: one `Arc::clone` per install.
- **Hazard:** a caller cannot hold `&cache.compiled.program` and `&mut cache` at
  once. Callers that need both take their own `Arc` clone — the engine already
  holds one, and `ExecutionFrame` would carry a clone instead of a `&'a Program`.
  Verify this against `executor::invoke_node`, which interleaves program reads
  with `&mut cache` calls.

### P2 — move `fs_paths` up to the cache; `StampJob` stays the walker

`ResourceStamper` is a field of `RuntimeCache` that receives its owner's `slots`
back on every call. Split it by what it owns rather than by what it does:

- `fs_paths` (the per-run memo the digest fold reads) → `RuntimeCache`
- `StampJob` (queue + off-thread walk) stays as-is — it is genuinely separable

Then, with P1:

| before | after |
| --- | --- |
| `identify(&mut self, program, slots, nodes, cancel)` | `identify(&mut self, nodes, cancel)` |
| `request_node_paths(&mut self, program, slots, node_idx)` | `request_node_paths(&mut self, node_idx)` |
| `node_digest(&self, program, node_idx, slots)` | `node_digest(&self, node_idx)` |
| `hash_bound_fs_path(&self, hasher, slots, addr)` | `hash_bound_fs_path(&self, hasher, addr)` |

- Deletes all 5 `slots:` params and both `let Self { slots, stamper, .. }` splits.
- `node_digest` returns to `cache::digest`, fixing the broken doc link there and
  in `slot.rs:76`.

### P3 — one `Run` object (highest leverage, highest risk)

`plan` + `resolver` + `executor`'s per-run columns are five sibling structures
with identical lifetimes, re-planned from scratch every run. Merge them:

```rust
struct Run {
    compiled: Arc<CompiledGraph>,
    process_order, states, roots, seeded, event_sources,   // was ExecutionPlan
    demand, readers,                                        // was Resolver
    remaining_reads, outcomes,                              // was Executor
}
```

`Planner`, `Resolver`, `Executor` become passes over it.

- `RunRequest` disappears; `ExecutionFrame` drops from 10 fields to
  `{ run: &mut Run, cache: &mut RuntimeCache, ctx, reporter, outcome }` = 5.
- Removes `program` from the remaining plan/resolve/executor signatures.
- **Against:** a 10-field object, and the structural-vs-cache-aware pass split
  stops being expressed in types. That split is already convention-only —
  `execute` always runs all three back to back — but it is load-bearing
  *documentation*. Do P1+P2 first and re-measure before committing to this.

### P4 — collapse the read-accounting cluster

Give the six methods one owner. If `RemainingOutputReads` held the `counts`
*and* took `&mut RuntimeCache` per call, the rule ("complete a read; release on
the last one") becomes 2–3 methods on one type instead of 6 across two.

### P5 — share the reuse preamble

```rust
fn reuse_source(&self, node_idx, demand) -> Option<ReuseSource>  // Resident | Blob(BlobTarget)
```
`probe_reuse` = `.is_some()`; `hydrate_reuse` = match on it. Removes the
duplicated `is_resident_hit` → `blob_target` → disk sequence, and makes it
impossible for the probe and the hydrate to disagree about what is reusable —
the exact drift `CacheLoadFailed` exists to handle.

### Expected effect

| | now | P1+P2 | +P3 |
| --- | --- | --- | --- |
| `&Program` params | 33 | ~17 | ~4 |
| `slots:` params | 5 | 0 | 0 |
| self-borrow splits | 3 | 1 | 1 |
| `ExecutionFrame` fields | 10 | 10 | 5 |
| per-run borrow structs | 2 (16 fields) | 2 | 1 (5 fields) |

### Not proposed

- **Removing `node_idx` threading** (31 uses). It is the subject of nearly every
  method — that is a method taking its argument, not a design smell.
- **`demand` threading** (14 uses). It is a genuine per-run input the cache
  should not own; deriving it inside would require the cache to see the plan,
  re-creating the edge just deleted.
- **The dense-index design.** It is what makes per-run state a memset and every
  edge walk hash-free.
