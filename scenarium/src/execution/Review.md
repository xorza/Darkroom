Issues

1. intern_bindings converts a compile error into a panic, and leaves a dead validation variant.
program/mod.rs:161 does self.e_node_index[&port.e_node_id] — bare HashMap index, panics on a bind naming an unemitted producer. It runs at compile.rs:163, before validate_debug at compile.rs:178. And because every surviving NodeIdx came from a successful lookup, validate.rs:145's .get() can never return None — CompiledGraphValidationError::MissingBindingTarget is now unreachable.

Pick one: delete the variant and make the panic an .expect() that names the invariant, or have intern_bindings leave ExecutionBinding::None on a miss so validate still reports it. Note the inconsistency with apply_subscriptions twelve lines below, which silently skips the same class of drift.

2. Install-side validation got weaker. InstalledGraphValidationError::MissingNode is gone because slots no longer carry ids, so the only alignment check is slots.len() == e_nodes.len() (validate.rs:179). A misaligned column is now caught only incidentally by the StateOwner compare — which won't fire between neighbours sharing func id + version, i.e. two instances of the same func, which is the common case in this model. resident_ram_stats (cache/runtime/mod.rs:89) zips ids against slots and would silently mis-attribute RAM under the same failure.

3. validate_plan can panic on the corruption it exists to report. validate.rs:250 uses the Option-safe verdicts.get(addr.node_idx), but validate.rs:253 uses seen_in_order.contains(addr.node_idx), which indexes Vec<bool> directly and panics out of range. Debug-only, but it's a diagnostic path — make it consistent.

Design

4. reconcile(previous, program) — the cache can't interpret its own state. The previous parameter plus assert_eq!(slots.len(), previous.e_nodes.len()) encodes an invariant spanning two ExecutionEngine fields. Give RuntimeCache its own NodeColumn<ExecutionNodeId> of the ids its slots are aligned to and: previous disappears, the assert disappears, resident_ram_stats drops its program param, and validate_installed can do a real element-wise id comparison — which fixes #2 properly. Cost is 16 bytes/node written once per install. This is my main suggestion.

5. Two container types for three parallel structures. e_nodes: Vec<ExecutionNode> next to e_node_ids: NodeColumn<ExecutionNodeId>. Making e_nodes a NodeColumn lets validate.rs:145/:236 use the typed NodeColumn::get(NodeIdx) instead of raw .get(idx.idx()). Then add NodeColumn::iter_indexed() -> (NodeIdx, &T) and(i as u32) reconstructions (resolve_output_types,release_dead_outputs, collect_roots, compile.rs:72, engine.rs:172) all vanish. The Index<NodeIdx> for ExecutionProgram impl exists mostly to paper over this split.

6. Flattener::subs and pending_binds went pub(crate) so Compiler can reach into its scratch buffers. That makes internal scratch part of the API. build already holds &mut program.inputs/outputs/events — either let it populate the program directly, or return a FlattenOutput { e_nodes, binds, subs }. Separately, pending_binds: Vec<(u32, ExecutionOutputPort)> is a bare tuple sitting directly beside ExecutionSubscription, a named struct for the exact same "resolved edge fixup" role.

7. Index order is UUID order, not topological. You removed the hashing, but process_order still walks NodeIdx values scattered
uniformly across e_nodes, slots, verdicts, dispositorded its emission order — or you sorted bytopological rank with id as tiebreak — the hot walk would stride forward through every column instead of jumping. Both are
deterministic. Only worth doing if you have a large the hash removal is the dominant win either way.

Simplifications

- compile.rs:108 — closure.sort_unstable() is now a no-op. in_closure.iter() yields ascending NodeIdx, and index order is id order after adopt_nodes. Drop it or make it debug_assert!(closure.is_sorted()).
- compile.rs:147 allocates a fresh HashMap per compit, plus a Vec for the sort. Flattener::build still
calls e_nodes.clear() — that buffer was designed toner and drain() it.
- NodeSet as Vec<bool> × 3 (roots, pinned, event_sources) is 3 bytes/node and three memsets per plan, and iter() is a full O(n) scan even for a single-node preview seed. A bitset is 8× smaller per set. Low priority.

Nits

- ~11 files carry two adjacent use crate::execution::program::index::… lines (program/mod.rs, resolve/mod.rs, digest/mod.rs,
validate.rs, value_flow.rs, …). Merge them.
- executor/tests.rs:477 — nx() reverse-engineers the index arithmetically (as_u128() as u32 - 1) when program.e_node_index[&id] is right there. Silently breaks if the fixture ever stops using from_u128(idx + 1).
- output_idx and apply_subscriptions use self.e_nodof the crate uses the program[node_idx] impl theyadded.
- RunSeeds.e_node_ids names the type rather than the role — nodes read better — and the constructor is still RunSeeds::nodes(…), so the two now disagree.
