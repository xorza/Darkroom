# Code Review: prism/src/gui (Round 3)

## Summary

The GUI module is well-structured with clean separation between layout, rendering, and interaction. The 3-phase render pipeline in `GraphUi::render()` is clear, and the action coalescing in `GraphUiInteraction` is solid. The previous review's key suggestions (deferred deletions, explicit `ConstBindFrame::finish()`, `ButtonResult` extraction, assertion cleanup) have been implemented.

The main remaining issues are: dead code, duplicated connection deletion logic, hardcoded style values bypassing the style system, and several simplification opportunities.

## Findings

### Priority 1 — High Impact, Low Invasiveness

#### [F1] Duplicated connection deletion logic
- **Location**: `graph_ui.rs:355-421` and `connection_ui.rs:335-377`
- **Category**: Generalization
- **Impact**: 4/5 — Two independent code paths do the exact same graph mutation (disconnect Input bindings, remove Event subscribers) with the same action emission. A bug fix in one must be mirrored in the other.
- **Meaningfulness**: 5/5 — Real duplication with real maintenance risk.
- **Invasiveness**: 2/5 — Extract a shared `disconnect_connection(ctx, key, interaction)` function. Both call sites become one-liners.
- **Description**: `apply_breaker_results` (breaker tool) and `apply_connection_deletions` (double-click) both iterate `ConnectionKey` variants and perform identical disconnect + action-emit logic. Extract to a shared helper:
  ```rust
  fn disconnect(ctx: &mut GraphContext, key: ConnectionKey, interaction: &mut GraphUiInteraction) {
      match key {
          ConnectionKey::Input { input_node_id, input_idx } => { /* shared logic */ }
          ConnectionKey::Event { .. } => { /* shared logic */ }
      }
  }
  ```

#### [F2] Dead code: `brighten` function
- **Location**: `style.rs:10-17`
- **Category**: Dead code
- **Impact**: 3/5 — Dead code adds noise when reading the style module.
- **Meaningfulness**: 5/5 — Grep confirms zero callers in the entire codebase.
- **Invasiveness**: 1/5 — Delete 8 lines.
- **Description**: `pub fn brighten(color: Color32, amount: f32) -> Color32` is defined but never called anywhere. Remove it.

#### [F3] Dead code: `_scaled_u8` closure in `Style::new`
- **Location**: `style.rs:182-189`
- **Category**: Dead code
- **Impact**: 3/5 — Prefixed with `_` to suppress the warning, suggesting it was once used and is now stale.
- **Meaningfulness**: 5/5 — Unused code with assert logic that will never execute.
- **Invasiveness**: 1/5 — Delete 7 lines.
- **Description**: The `_scaled_u8` closure is defined inside `Style::new()` but never called. The `Shadow` fields that likely used it now use inline expressions like `(10.0 * scale).ceil() as u8`. Remove the dead closure.

#### [F4] Hardcoded colors in `node_details_ui.rs` bypass the style system
- **Location**: `node_details_ui.rs:150,160`
- **Category**: Consistency
- **Impact**: 4/5 — These colors won't change when the user edits `StyleSettings`, breaking theming.
- **Meaningfulness**: 4/5 — The style system already has matching colors for these exact states.
- **Invasiveness**: 1/5 — Two one-line changes.
- **Description**: `show_execution_info` uses hardcoded `Color32::from_rgb(255, 100, 100)` for errors and `Color32::from_rgb(255, 180, 70)` for missing inputs. The style already defines `node.errored_shadow.color` and `node.missing_inputs_shadow.color` for these states. Use them instead:
  ```rust
  // line 150: replace Color32::from_rgb(255, 100, 100) with:
  gui.style.node.errored_shadow.color
  // line 160: replace Color32::from_rgb(255, 180, 70) with:
  gui.style.node.missing_inputs_shadow.color
  ```

#### [F5] `PointerButtonState` enum and `get_primary_button_state` are over-abstracted
- **Location**: `graph_ui.rs:54-58, 202-214`
- **Category**: Simplification
- **Impact**: 3/5 — 20 lines of code for what should be a single boolean check.
- **Meaningfulness**: 4/5 — The `Released` variant is never matched; `Pressed` and `Down` are always grouped together.
- **Invasiveness**: 1/5 — Inline at the single call site.
- **Description**: The only usage of `get_primary_button_state` is:
  ```rust
  let primary_down = matches!(
      Self::get_primary_button_state(gui),
      Some(PointerButtonState::Pressed | PointerButtonState::Down)
  );
  ```
  Replace the entire enum + function with:
  ```rust
  let primary_down = gui.ui().input(|i| i.pointer.primary_pressed() || i.pointer.primary_down());
  ```
  Delete `PointerButtonState` and `get_primary_button_state`.

#### [F6] Unused parameter `_ctx` in `rebuild_texture`
- **Location**: `graph_background.rs:58`
- **Category**: Dead code
- **Impact**: 2/5 — Minor but misleading — suggests the texture depends on graph context.
- **Meaningfulness**: 4/5 — The parameter is explicitly prefixed with `_`.
- **Invasiveness**: 1/5 — Remove parameter from signature and call site.
- **Description**: `GraphBackgroundRenderer::rebuild_texture` takes `_ctx: &GraphContext<'_>` but never uses it. The texture only depends on `gui.style`. Remove the parameter.

### Priority 2 — High Impact, Moderate Invasiveness

#### [F7] Duplicated breaker/hover/double-click pattern in `ConnectionUi::render`
- **Location**: `connection_ui.rs:155-197` (data connections) and `connection_ui.rs:200-242` (event connections)
- **Category**: Generalization
- **Impact**: 4/5 — The same 10-line interaction pattern (update curve, check breaker, handle hover/double-click) is copy-pasted for data and event connections.
- **Meaningfulness**: 4/5 — Real duplication of non-trivial interaction logic.
- **Invasiveness**: 3/5 — Requires extracting a helper that takes `key`, `start_pos`, `end_pos`, `port_kind`, and returns whether a deletion was requested.
- **Description**: Extract the shared pattern into a helper:
  ```rust
  fn update_curve_interaction(
      gui: &mut Gui, curves: &mut CompactInsert<..>, deletions: &mut Vec<ConnectionKey>,
      key: ConnectionKey, start_pos: Pos2, end_pos: Pos2, port_kind: PortKind,
      breaker: Option<&ConnectionBreaker>,
  ) { ... }
  ```

#### [F8] `path_length()` recomputed from scratch on every `add_point` call
- **Location**: `connection_breaker.rs:126-133`
- **Category**: Simplification / Data flow
- **Impact**: 3/5 — O(n) recompute per point addition; becomes quadratic over the full breaker stroke.
- **Meaningfulness**: 3/5 — Breaker lines are short (max 900px / 4px = 225 points), so not a perf issue in practice, but the fix is trivial.
- **Invasiveness**: 2/5 — Add a `cached_length: f32` field, update in `add_point`, reset in `reset`.
- **Description**: `add_point` calls `self.path_length()` which sums all segment lengths from scratch. Instead, maintain a running `cached_length` field:
  ```rust
  // In add_point, after pushing the new point:
  self.cached_length += segment_len;
  ```

#### [F9] Four identical shadow constructions differ only by color
- **Location**: `style.rs:258-281`
- **Category**: Simplification
- **Impact**: 3/5 — Four 5-line blocks with identical structure; changing blur/spread requires editing all four.
- **Meaningfulness**: 3/5 — Real duplication with change-amplification risk.
- **Invasiveness**: 2/5 — Extract a `make_status_shadow(color, scale, settings)` helper.
- **Description**: `executed_shadow`, `cached_shadow`, `missing_inputs_shadow`, and `errored_shadow` all have `offset: [0, 0]`, same `blur`, same `spread`, different `color`. Extract:
  ```rust
  let status_shadow = |color| Shadow {
      color,
      offset: [0, 0],
      blur: scaled(style_settings.shadow_blur).ceil() as u8,
      spread: scaled(style_settings.shadow_spread).ceil() as u8,
  };
  ```

#### [F10] Three near-identical port label rendering loops
- **Location**: `node_ui.rs:531-553`
- **Category**: Generalization
- **Impact**: 3/5 — Three loops with identical structure, differing only in (a) galley list and (b) x-offset direction (left-aligned for inputs, right-aligned for outputs/events).
- **Meaningfulness**: 3/5 — Duplication that will need triple-editing if label positioning logic changes.
- **Invasiveness**: 2/5 — Extract a `render_label_column(gui, galleys, center_fn, align_left, padding)` helper.
- **Description**: The input loop uses `+padding` offset (left-aligned), while output and event loops use `-padding - width` (right-aligned). A shared helper could take an alignment enum or sign parameter.

### Priority 3 — Moderate Impact

#### [F11] `log_ui.rs` bypasses the `Gui` wrapper entirely
- **Location**: `log_ui.rs:30`
- **Category**: Consistency
- **Impact**: 2/5 — Works correctly, but `LogUi::render` takes `&mut Gui` then immediately accesses the private `gui.ui` field, never using any `Gui` methods.
- **Meaningfulness**: 3/5 — The function signature claims a `Gui` dependency it doesn't actually use.
- **Invasiveness**: 2/5 — Change signature to take `(ui: &mut Ui, style: &Style, status: &str)`, or refactor to use `gui.ui()`.
- **Description**: `LogUi::render` extracts `gui.style` then passes `gui.ui` (private field) to `frame.show()`. The rest of the function works on raw `egui::Ui`. Either change the signature to honestly reflect dependencies, or use `gui.ui()` properly.

#### [F12] `new_node_ui.rs` accesses `gui.ui` field directly
- **Location**: `new_node_ui.rs:225-227`
- **Category**: Consistency
- **Impact**: 2/5 — Works due to submodule visibility, but inconsistent with the rest of the file which uses `gui.ui()`.
- **Meaningfulness**: 3/5 — One direct field access among ~10 method calls in the same file.
- **Invasiveness**: 1/5 — Change `gui.ui.make_persistent_id(...)` to `gui.ui().make_persistent_id(...)`.
- **Description**: `show_category_functions` accesses `gui.ui` (private field) on line 226 while every other access in the file uses the `gui.ui()` method. Change for consistency.

#### [F13] Redundant `PhantomData<&'a mut Ui>` in `Gui` struct
- **Location**: `mod.rs:31`
- **Category**: Dead code
- **Impact**: 2/5 — Confusing to readers — the lifetime is already captured by `ui: &'a mut Ui`.
- **Meaningfulness**: 3/5 — PhantomData is typically used when a lifetime isn't otherwise constrained. Here it's redundant.
- **Invasiveness**: 1/5 — Remove the field and its initialization in constructors.
- **Description**: `Gui<'a>` has `ui: &'a mut Ui` which already constrains `'a`. The `_marker: PhantomData<&'a mut Ui>` adds nothing. Remove it.

#### [F14] Hardcoded spacing values in `node_details_ui.rs`
- **Location**: `node_details_ui.rs:205,293,303-306`
- **Category**: Consistency
- **Impact**: 2/5 — Magic numbers `4.0`, `8.0` for spacing when `style.padding` and `style.small_padding` exist.
- **Meaningfulness**: 3/5 — These won't scale with the style system.
- **Invasiveness**: 1/5 — Replace literals with style references.
- **Description**: `add_space(4.0)` and `add_space(8.0)` should use `gui.style.small_padding` and `gui.style.padding` (or multiples thereof) for consistency with the rest of the UI.

#### [F15] `Gui::new` and `Gui::new_with_scale` near-duplication
- **Location**: `mod.rs:35-56`
- **Category**: Simplification
- **Impact**: 2/5 — Two constructors with identical bodies except one hardcodes `scale: 1.0`.
- **Meaningfulness**: 2/5 — Minor DRY violation.
- **Invasiveness**: 1/5 — Have `new` call `new_with_scale(ui, style, 1.0)`.
- **Description**: `Gui::new` is identical to `Gui::new_with_scale` except for the default scale. Simplify:
  ```rust
  pub fn new(ui: &'a mut Ui, style: &Rc<Style>) -> Self {
      Self::new_with_scale(ui, style, 1.0)
  }
  ```

### Priority 4 — Low Priority

#### [F16] Commented-out `#[derive(Debug)]` on `Gui` struct
- **Location**: `mod.rs:25`
- **Category**: Dead code
- **Impact**: 1/5 — Stale comment adds minor noise.
- **Meaningfulness**: 2/5 — Cosmetic.
- **Invasiveness**: 1/5 — Delete the comment line.
- **Description**: `// #[derive(Debug)]` is commented out (because `egui::Ui` doesn't implement `Debug`). Either add a manual `Debug` impl (like `GraphBackgroundRenderer` does) or just remove the comment.

#### [F17] `GraphUiInteraction` has inconsistent field visibility
- **Location**: `graph_ui_interaction.rs:13-21`
- **Category**: API cleanliness
- **Impact**: 1/5 — Works fine; the public fields are simple values, the private ones have invariants.
- **Meaningfulness**: 2/5 — The mixed visibility is intentional (actions need careful coalescing) but could be cleaner.
- **Invasiveness**: 2/5 — Add accessors for `errors`, `run_cmd`, `request_argument_values` and make them private.
- **Description**: `errors: Vec<Error>`, `run_cmd: Option<RunCommand>`, and `request_argument_values: Option<NodeId>` are `pub` while `coalesced_actions`, `immediate_actions`, and `pending_action` are private. Adding simple accessors/setters would make the encapsulation consistent.

#### [F18] `NonNull` import unused in `mod.rs`
- **Location**: `mod.rs:1`
- **Category**: Dead code
- **Impact**: 1/5 — Unused import.
- **Meaningfulness**: 2/5 — Compiler may already warn about this.
- **Invasiveness**: 1/5 — Remove `NonNull` from the import line.
- **Description**: `use std::ptr::NonNull` is imported but never used in `mod.rs`. Remove it.

## Cross-Cutting Patterns

### Connection manipulation duplication
The most impactful cross-cutting issue is the duplicated connection disconnect logic. Three places touch connections: breaker tool (`graph_ui.rs`), double-click deletion (`connection_ui.rs`), and const-bind clearing (`const_bind_ui.rs`). The first two share near-identical code for Input and Event disconnection. A single `disconnect_connection` helper would unify them and reduce the surface area for bugs.

### Inconsistent `gui.ui` vs `gui.ui()` access
Two files (`log_ui.rs`, `new_node_ui.rs`) access the private `gui.ui` field directly, while the rest of the codebase consistently uses the `gui.ui()` accessor. This is possible because submodules can see parent module's private fields, but it breaks the abstraction boundary that `Gui` establishes.

### Style bypass in details panel
`node_details_ui.rs` has both hardcoded colors and hardcoded spacing values, making it the only rendering file that partially ignores the style system. Bringing it in line would make the theming story complete.
