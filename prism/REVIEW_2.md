# Prism Second-Pass Review — residue

The nine-step structural refactor shipped (history in git). This doc
tracks the **open items** surfaced by a fresh three-angle review, now
that most of the noise is gone.

## Baseline invariants (held today)

- Zero writes to `view_graph.{pan,scale,selected_node_id}` or
  `view_graph.{graph,view_nodes}.{add,remove_node,by_id_mut,by_key_mut}`
  anywhere in `gui/`. Every mutation funnels through
  `GraphUiAction::apply` in `AppData::handle_actions`.
- `AppState` is pure — no async, no channels, no egui. Testable
  directly.
- `Interaction` is a variant-local state machine. Stale-breaker /
  dangling-drag classes are structurally impossible.
- 58 tests cover the action boundary — cycle detection,
  subscribe-once, drag transitions, apply idempotency.

## Open items — ordered by effort × payoff

### M1. Per-node execution-stats cache in `render_nodes`

`gui/node_ui.rs` calls `get_missing_input_ports(execution_stats, node_id)`
per port per node per frame. Each call filters
`execution_stats.missing_inputs` linearly. For 100 nodes × 5 ports =
500 filters/frame. Compute once per node at the top of the render
loop. Same kind of hot-path win as the Step 1 `path_length` fix.

### Apple laptop. Galley cache churn in `NodeLayout`

`node_layout.rs::rebuild_port_galleys` reallocates all three galley
`Vec`s on every scale change, and `make_galley` clones the font plus
calls `text.to_string()` on every call. `title_galley` already caches
on name-change; the port galleys should use the same pattern. Hot
path — every node, every frame a scale changes.

### M3. Test fixtures for the action-undo suite

`common/undo_stack/action_undo_stack.rs::undo_roundtrip_all_action_variants_with_json_snapshots`
is ~170 lines of setup for ~10 lines of assertion. Extract shared
helpers:

```rust
fn make_test_node(inputs: usize, events: usize) -> Node
fn bind_input(vg: &mut ViewGraph, from: (NodeId, usize), to: (NodeId, usize))
fn subscribe_event(vg: &mut ViewGraph, emitter: (NodeId, usize), subscriber: NodeId)
```

Trims the test by ~60 lines and unblocks M4.

### M4. Direct `apply(a); undo(a) == identity` invariant test

The existing JSON-snapshot roundtrip proves undo works via
serialization equality. A direct state-equality test per variant is
the natural shape of the invariant — faster, clearer failure
messages, no JSON fixtures to keep in sync. Pair with M3 helpers.

### L1. Drop the last `&mut` from `GraphContext`

Only `node_details_ui::show_content` writes through
`ctx.argument_values_cache` (lazy preview texture fill). Everywhere
else the `&mut` is inert. Drop it from `GraphContext` and pass it
explicitly to the one call site. Cosmetic — `GraphContext` becomes
`Copy`-like, signatures get simpler. Worth doing only when someone
wants that property.

### L2. `const_bind_ui` broke-tracking ownership

`ConstBindUi::broke_iter` inspects its own cached curves to surface
breaker hits. Works (each curve self-reports after rendering), but a
reader can't tell whether the breaker or the curves own the hit-test
result. One-line doc-comment on the method resolves it; no code
change.

### L3. Render-boundary tests via real egui `Context`

No `egui::__run_test_ui` helper exists; the minimum fixture is:

```rust
let ctx = egui::Context::default();
ctx.run(egui::RawInput::default(), |ctx| {
    let mut input = InputSnapshot::capture(ctx);
    input.primary_down = true;
    // … build Gui, call graph_ui.render(…) …
    // assert on graph_ui.ui_interaction().action_stacks()
});
```

Doable; not trivial. Only justified when someone adds render-time
behaviour worth pinning down.

### L4. Typed save/load error

`app_data.rs::load_graph` collapses every failure mode into a string
in the status bar. `FileNotFound` and `Corrupted` could drive
different UI responses (retry vs. revert-to-default). Product-polish,
not correctness.

## Items that looked like problems and aren't

(Filtered from the agent sweep — worth stating explicitly so the same
claims don't get re-raised.)

- **`graph_background.rs` `Option<Texture>` unwraps** — initialised
  at the top of `render()` before `draw_tiled()` in the same call.
  Invariant-local.
- **`polyline_mesh.rs` `Arc::get_mut().unwrap()`** — the mesh is
  exclusively owned while being built; the `Arc` exists so the
  finished mesh can be handed to the painter lock-free. Document the
  invariant; don't change the shape.
- **`GraphUi::render` "too large"** (635 lines in `graph_ui/mod.rs`)
  — it's the orchestrator. Phases already live in submodules
  (`connections.rs`, `overlays.rs`, `pan_zoom.rs`) via inherent
  `impl` blocks.
- **`text_edit.rs` (1286 lines)** — fork of egui's `TextEdit`.
- **`PortInteractCommand::priority` magic numbers** — one call site
  (`prefer`); order is the information content.
