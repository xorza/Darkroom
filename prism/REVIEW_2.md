# Prism Second-Pass Review

After all nine refactor steps landed, a fresh three-angle review
(architecture / code quality / testability + errors). This is the
filtered, opinionated version — the raw agent output had noise (items
already addressed, invariant-backed unwraps flagged as bugs, size-as-
smell complaints that don't survive reading the code). What's below is
what I'd act on.

## What's genuinely good now

- **The mutation pipeline is clean.** `grep` confirms zero writes to
  `view_graph.{pan,scale,selected_node_id}` or
  `view_graph.{graph,view_nodes}.{add,remove_node,by_id_mut,by_key_mut}`
  in `gui/`. Every mutation funnels through `GraphUiAction::apply` in
  `AppData::handle_actions`.
- **`AppState` is truly pure.** No async, no channels, no egui — 4
  tests exercise it directly. `AppData` wraps with session machinery.
- **Interaction is a real state machine.** Variant-local data means
  stale-breaker / dangling-drag classes are structurally impossible.
- **60 tests covering the action boundary** — cycle detection,
  subscribe-once invariants, drag state transitions, apply idempotency.
  Historically bug-heavy paths now have regression coverage.
- **`Rc<Style>` cost measured, left in place with justification** (see
  Step 8.2). Honest answer to a closed question.

## What's still rough — ordered by effort × payoff

### H1. Extend `Error::StaleNode` defense to three more sites

Same class of bug that Step 9 addressed, three more places where an
undo / redo during an in-flight UI operation could panic.

- **`gui/graph_ui/pan_zoom.rs:~160` — `fit_all_nodes_target`** does
  `layouts.next().unwrap()` after an `is_empty` check. Safe today, but
  if `view_nodes.is_empty() == false` yet `graph_layout.node_layouts`
  isn't populated yet (first frame after a load, any ordering glitch),
  it panics. Fix: handle `None` by returning `(1.0, Vec2::ZERO)`.
- **`gui/node_details_ui.rs:87-99`** accesses
  `ctx.view_graph.graph.by_id(&node_id).unwrap()` and `.func_lib.by_id`
  where `node_id` came from `selected_node_id`. `remove_node` clears
  selection, so the race is narrow — but the name-editor path reads
  it in three places. Wrap once, return early if missing.
- **`gui/node_details_ui.rs:141`** — `ctx.func_lib.by_id(func_id).unwrap()`
  when rendering execution stats. A node's `func_id` can outlive the
  func only if the lib is hot-swapped; unlikely but cheap to guard.

**Size:** ~20 lines, one commit. **Payoff:** closes the stale-ID panic
class completely, same pattern as Step 9, no new concepts.

### H2. Kill the misleading name: `apply_mew_graph`

`app_data.rs:~325` — literal typo (`mew` → `new`) preserved from the
original code. Rename to `replace_graph` (it's not really "apply" — it
discards the existing view graph and installs a new one, resetting the
undo stack). While there: `apply_data_connection` and
`apply_event_connection` in `graph_ui/connections.rs` don't actually
apply anything either — they *build* the action that would apply. Name
drift from before Step 4.0. Rename to `build_data_connection_action`
/ `build_event_connection_action`. One commit.

### M1. Per-node execution-stats cache in `render_nodes`

`gui/node_ui.rs` calls `get_missing_input_ports(execution_stats, node_id)`
per port per node per frame. Each call filters `execution_stats.missing_inputs`
linearly. For 100 nodes × 5 ports = 500 filters/frame. Compute per
node at the start of the render loop instead. Same hot-path cost as
the old `path_length` call Step 1 fixed.

### Apple laptop. Galley cache churn in `NodeLayout`

`node_layout.rs:~132` — `rebuild_port_galleys` reallocates all three
galley `Vec`s on every scale change. And `make_galley` clones the font
plus calls `text.to_string()` on every call, even when the text hasn't
changed. The current design caches `title_galley` separately on
name-change; the port galleys should use the same pattern. Hot-path:
called for every node, every frame a scale changes.

### M3. Test fixtures for the action-undo suite

`common/undo_stack/action_undo_stack.rs::undo_roundtrip_all_action_variants_with_json_snapshots`
is ~170 lines of setup for ~10 lines of assertion. No shared helpers
for "create a node with N inputs / M events", "install a data
binding", "install an event subscription". Extract:

```rust
fn make_test_node(inputs: usize, events: usize) -> Node
fn bind_input(vg: &mut ViewGraph, from: (NodeId, usize), to: (NodeId, usize))
fn subscribe_event(vg: &mut ViewGraph, emitter: (NodeId, usize), subscriber: NodeId)
```

Trims the test by 60 lines, makes it scannable, and any follow-up
apply/undo test can reuse them.

### M4. Add a direct `apply(a); undo(a) == identity` invariant test

The existing JSON-snapshot roundtrip proves undo works via
serialization equality. A direct test — for each action variant,
build a representative state, snapshot it, `apply(&action)`,
`undo(&action)`, assert `state == snapshot` — is the natural shape of
the invariant. Faster, clearer failure messages, no JSON fixture
maintenance. Pair with the M3 helpers.

### L1. `GraphContext` holds the only remaining `&mut`

Only `node_details_ui::show_content` actually writes through
`ctx.argument_values_cache` (lazy preview texture fill). Everywhere
else the `&mut` is inert. Two options:
- Drop `argument_values_cache` from `GraphContext` entirely, pass it
  explicitly to `NodeDetailsUi::show` only.
- Use interior mutability (`RefCell`) and make the whole `GraphContext`
  an `&GraphContext`.

Option 1 is better (explicit > interior). But it's cosmetic — only
worth doing when someone actually wants `GraphContext: Copy` or
something similar. Not today.

### L2. `const_bind_ui` ownership ambiguity

`ConstBindUi` iterates its own `const_link_bezier_cache` to surface
which links were broken by the breaker (`broke_iter()`). The breaker
itself lives in `Interaction::BreakingConnections`. This works — each
curve self-reports after rendering — but a reader can't tell whether
the breaker or the curves own the hit-test result. A one-line
doc-comment on `ConstBindUi::broke_iter` explaining the convention
resolves it. No code change.

### L3. Render-boundary tests require a real egui `Context`

Agent 3 investigated. There's no `egui::__run_test_ui` helper; the
minimum fixture is:

```rust
let ctx = egui::Context::default();
ctx.run(egui::RawInput::default(), |ctx| {
    let mut input = InputSnapshot::capture(ctx);
    input.primary_down = true;
    // … build Gui, call graph_ui.render(&mut gui, &mut app_data, &input, &arena) …
});
```

Doable; not trivial. Would add `tests/render_smoke.rs` that seeds a
graph, drives one frame with a crafted input, and asserts on the
emitted action list. **Only useful if someone adds new render-time
behaviour worth pinning down.** Skip for now; note the shape for when
it matters.

### L4. Typed save/load error

`app_data.rs::load_graph` collapses every failure mode to a string in
the status bar. `FileNotFound` and `Corrupted` should produce
different UI (retry vs. revert-to-default). Convert `anyhow::Result`
to a `GraphLoadError` enum. Lower priority — current users see a
string that works; it's a product-polish item, not a correctness one.

## Items that looked like problems and aren't

Claims from the agent sweep that don't survive a closer reading:

- **`graph_background.rs` `Option<Texture>` unwraps.** Initialized at
  the top of `render()` before `draw_tiled()` is called in the same
  function. Invariant-local to one call.
- **`polyline_mesh.rs` `Arc::get_mut().unwrap()`** inside `rebuild`/
  `append`. The `PolylineMesh` is exclusively owned while being built;
  the `Arc` exists so the finished mesh can be handed to the painter
  lock-free. Document the invariant (one line) but don't change the
  shape.
- **`GraphUi::render` "too large"** (635 lines in `graph_ui/mod.rs`).
  It's the orchestrator. The phases already live in submodules
  (`connections.rs`, `overlays.rs`, `pan_zoom.rs`) via inherent `impl`
  blocks. Further splitting would fragment one coherent pipeline into
  unrelated pieces.
- **`text_edit.rs` (1286 lines).** Fork of egui's `TextEdit` with a
  handful of customisations. Large because TextEdit is large. Don't
  touch.
- **`PortInteractCommand::priority` magic numbers (0/5/8/10/15).**
  Named variants would be nice, but this function has one call site
  (`prefer`) and the priority order is the entire information content.
  Cosmetic at best.

## Recommended next action

Ship H1 (stale-ID defense extension) and H2 (rename `apply_mew_graph`
+ the `build_*_connection` renames) as a single commit — 15 minutes,
no risk. Then sit on the rest until a specific reason comes up
(M3/M4 when the next graph-action bug lands, M1/Apple laptop when frame time
becomes interesting).

The refactor plan is effectively done. Everything here is editorial
polish on a codebase that's already past its structural issues.
