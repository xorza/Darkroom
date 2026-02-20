# execution_graph.rs - Research & Analysis (AI)

Comprehensive comparison of current implementation against industry standards from Nuke, Houdini, Bazel/Skyframe, Salsa, Turbopack, Taskflow, and academic research.

## Architecture Overview

Three-phase execution pipeline:
1. **Graph Update** (`update`): Sync execution nodes with graph structure via CompactInsert
2. **Preparation** (`prepare_execution`): Two backward passes + one forward pass to determine what to execute
3. **Execution** (`run_execution`): Sequential async invocation in dependency order

State machine: `Unvisited -> Visiting -> DependenciesResolved -> Ready`

## What We Do Well

| Feature | Assessment |
|---------|-----------|
| DFS cycle detection via ProcessState | Correct, standard approach |
| Pure/Impure/Once caching modes | Covers the three key behaviors |
| Binding change tracking | Thorough old-vs-new comparison, including const value equality |
| Error propagation to dependents | Clean "skip due to upstream error" pattern |
| Output usage counting (Skip/Needed) | Lets nodes skip unused output computation |
| Debug-only validation assertions | Catches logic errors without runtime cost |
| Test coverage | 20+ tests covering caching, errors, bindings, serialization |
| CompactInsert for structural updates | Efficient add/remove during graph changes |
| Missing-input transitivity | Correctly propagates through dependency chains |

## Critical Gap: No Change Pruning / Backdating

**Severity: High. This is the single most important optimization across ALL production dataflow systems.**

When a node re-executes and produces output **identical to its previous output**, the system should NOT propagate dirtiness downstream. Currently:

```
propagate_input_state_forward():
  inputs_updated |= dep_wants_execute   // cascades regardless of actual output change
```

If `get_b` (Impure) re-executes but returns the same value, `sum`, `mult`, and `print` all re-execute unnecessarily. In production systems:

- **Salsa** calls this "backdating" -- if re-execution produces same result, revision stays old
- **Bazel/Skyframe** calls it "change pruning" -- same output = no cascade
- **Turbopack** uses equality-based cutoff on value cells
- **Nuke** uses content hashing -- same hash = same data = no recompute

**Impact**: For graphs with impure sources (file readers, sensors, timers) that frequently produce unchanged data, this causes O(N) unnecessary re-execution per frame where N = total downstream nodes.

**Fix approach**: After execution, compare new output with previous. If identical, mark node as "output unchanged" so downstream Pure nodes remain cached. Requires splitting the current single-pass prepare + execute into an interleaved approach, or a post-execution pruning step.

## Second Backward Pass is Unnecessary

`walk_backward_collect_execute_order()` (lines 708-766) duplicates `walk_backward_collect_order()` structure. After the forward pass computes `wants_execute` for every node, the execute order is simply:

```rust
e_node_execute_order = e_node_process_order
    .iter()
    .copied()
    .filter(|&idx| self.e_nodes[idx].wants_execute)
    .collect();
```

The second backward walk re-traverses edges and re-does cycle checking (via unreachable panics). The only theoretical difference would be if a node `wants_execute` but isn't reachable from a terminal through `wants_execute` edges -- but this can't happen because `wants_execute` is only set on nodes in `e_node_process_order` (which are terminal-reachable), and if a node wants to execute, its needed dependencies also want to execute (ensured by forward propagation).

**Recommendation**: Replace pass 2 with a filter over `e_node_process_order`.

Also: `e_node_terminal_idx.drain()` in pass 2 (line 712) already empties the set, making `e_node_terminal_idx.clear()` in `execute()` (line 526) redundant.

## Sequential Execution

```rust
// run_execution():
for e_node_idx in self.e_node_execute_order.iter().copied() {
    // ... execute one by one
}
```

Industry standard (Bazel, Taskflow, Blender Geometry Nodes) processes independent nodes **in parallel**. Nodes at the same topological level with no mutual dependencies can execute concurrently.

For this project's likely graph sizes (tens of nodes), the overhead of parallelism probably exceeds the benefit. But if any node performs expensive computation (image processing, ML inference), parallel execution becomes important.

**If needed**: Kahn's algorithm naturally produces "wavefronts" of independent nodes. A work-stealing scheduler (Rayon) could process each wavefront in parallel.

## Missing: Demand-Driven (Pull-Based) Evaluation

The current system is **push-based**: all terminal-reachable nodes are processed, even if their outputs aren't needed by any executing path.

Production systems (Houdini, Salsa, Adapton) use **demand-driven evaluation**: start from needed outputs, walk backward, only evaluate what's actually required.

The current system partially addresses this via `OutputUsage::Skip/Needed`, but this is just advisory -- it doesn't prune the execution graph.

**Impact**: Low for small graphs. Could matter for large graphs with many dormant branches.

## Stale Comment in walk_backward_collect_order

Line 619:
```rust
ProcessState::Unvisited => unreachable!("should be Forward"),
```

There is no "Forward" state. This comment is from before `ProcessState::Ready` was renamed. Should say `"should be Ready"`.

## update_input_binding Control Flow is Confusing

The binding change flag is set in two separate locations with different logic:

1. Initial match (lines 445-461): `binding_changed |= match (binding, &e_input.binding) { ... }`
   - For `Bind -> Bind`: returns `false` (defers to later check)
   - For `Bind -> non-Bind`: sets `Undefined`, returns `true`

2. Later refinement (lines 480-497): checks actual target/port equality
   - Same target+port: silently updates `target_idx`
   - Different target or port: sets `binding_changed = true`

This is correct but hard to follow. The split across two locations means you need to read both to understand the full change-detection logic.

## Incremental Graph Updates

`update()` re-processes ALL nodes on every call:

```rust
fn build_execution_nodes(&mut self, graph: &Graph, func_lib: &FuncLib) {
    for node in graph.nodes.iter() {  // iterates ALL nodes
        // ...
    }
}
```

The `invalidate_recursively()` method exists for targeted invalidation but isn't used in the normal update path. For graphs with hundreds of nodes where only one changes, this is wasteful.

**Mitigation**: `CompactInsert` and the `binding_changed` tracking mean the overhead per unchanged node is small (refresh metadata, compare bindings). So this is more of a design cleanliness issue than a performance problem at current scale.

## ProcessState Reuse Across Phases

`ProcessState` is reused for three different purposes:
1. **Graph update**: Set to `Ready` after building
2. **Backward pass**: `Ready -> Visiting -> DependenciesResolved` (cycle detection)
3. **Forward pass**: `DependenciesResolved -> Ready`
4. **Execute pass**: `Ready -> Visiting -> DependenciesResolved` (reused again)

The state has no formal "phase" concept, relying on caller discipline. Asserting expected states in each phase catches bugs but the design is fragile.

## Missing: Output Value Lifetime Management

Output values (`output_values: Option<Vec<DynamicValue>>`) live indefinitely once computed. There's no mechanism to free outputs that are no longer needed.

Production systems use:
- **Reference counting**: Free output when all consumers have read it
- **LRU eviction**: Drop cold entries under memory pressure
- **Execution-order-based lifetime**: Process depth-first to release intermediates sooner

For DynamicValue::Custom (which holds `Arc<dyn Any>`), this could hold large allocations (images, buffers) long after they're consumed.

## Industry-Standard Feature Comparison

| Feature | Scenarium | Nuke | Houdini | Salsa | Bazel |
|---------|-----------|------|---------|-------|-------|
| Topological ordering | DFS | N/A (per-request) | Per-channel | On-demand | Kahn's |
| Cycle detection | At prepare time | At cook | At cook | At query | At analysis |
| Change pruning | No | Hash-based | Hash-based | Backdating | Change pruning |
| Demand-driven eval | No (push) | Yes (pull) | Yes (pull) | Yes (pull) | Yes (pull) |
| Parallel execution | No | Yes | Yes | No | Yes |
| Cache eviction | No | LRU | LRU | Revision GC | In-memory |
| Error propagation | Skip downstream | Error values | Error values | Panic/cycle | Fail fast |
| Incremental update | Full rebuild | Incremental | Incremental | Incremental | Incremental |

## Prioritized Recommendations

1. **Change pruning** (high impact): Compare output values after execution; suppress downstream cascade when unchanged. Single biggest improvement for interactive use.

2. **Simplify: remove second backward pass** (low risk, reduces ~60 lines): Replace `walk_backward_collect_execute_order()` with a filter over `e_node_process_order`.

3. **Fix stale comment** (trivial): Line 619 "should be Forward" -> "should be Ready".

4. **Output value eviction** (medium impact for memory): Track consumer count, drop outputs when all consumers have read them. Important when nodes produce large data (images).

5. **Parallel execution** (low priority at current scale): Use Rayon wavefronts for CPU-bound nodes. Only matters when individual nodes are expensive.

## References

- Salsa algorithm: https://salsa-rs.github.io/salsa/reference/algorithm.html
- Nuke hashing: https://learn.foundry.com/nuke/developers/80/ndkdevguide/advanced/hashing.html
- Houdini cooking: https://www.sidefx.com/docs/hdk/_h_d_k__op_basics__overview__cooking.html
- Bazel Skyframe: https://bazel.build/reference/skyframe
- Turbopack incremental: https://nextjs.org/blog/turbopack-incremental-computation
- Taskflow: https://github.com/taskflow/taskflow
- Adapton (demand-driven change propagation): https://dl.acm.org/doi/10.1145/2666356.2594324
