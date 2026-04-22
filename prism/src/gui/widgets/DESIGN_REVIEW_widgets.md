# Design review: prism/src/gui/widgets/  (2026-04-22)

## Current design

21 files / 2,570 LOC (of which `text_edit.rs` is a 1,308-line vendored fork
of `egui::TextEdit` — out of scope). The remaining ~1,260 LOC is a thin
shadow layer over egui primitives: container widgets (`Frame`, `Area`,
`ScrollArea`, `PositionedUi`, `Layout`, `ColumnFlow`, `Expander`,
`PopupMenu`), interactive widgets (`Button`, `DragValue`, `FilePicker`,
`ComboBox`), and display primitives (`Label`, `Space`, `Separator`,
`Image`, `Texture`, `HitRegion`, `StatusPanel`).

The load-bearing design decision is **why** these wrappers exist: the
`no_raw_ui_outside_widgets` and `no_egui_chrome_outside_widgets`
tripwires (`common/id_salt.rs`) forbid app code from calling
`gui.ui_raw()` or importing chrome types directly. Widgets are the
sanctioned site where raw egui lives. Everything takes `&mut Gui<'_>` +
optionally a `StableId`, and returns `Response` or `InnerResponse<R>`.
Widget id discipline is backed by `StableId` (typed newtype over
`egui::Id`) which is what lets chrome live at the same rect across
frames without tripping egui's drift warnings.

Persistence uses egui's `Memory::data` (both `get_temp` and
`get_persisted`) — with the recently-added `Gui::load_persistent<T>` /
`store_temp<T>` / etc. generic helpers most widgets don't use yet
because they pre-date those helpers.

## Overall take

The encapsulation model is right. The wrapping boundary successfully
isolates egui's weirdness and the tripwires make violations loud. The
findings below are about **consistency** and a couple of specific
contract bugs — not about whether this module should exist. Nothing
here justifies a ground-up rewrite.

## Findings

### [F1] `PopupMenu::new` mutates egui memory as a construction side-effect

- **Category**: Contract
- **Impact**: 4/5 — silent state corruption path, confidence-eroding when debugging open/close bugs
- **Effort**: 2/5 — move one block from `new` to the top of `show`
- **Current**: `popup_menu.rs:27–35` toggles the popup's stored open/closed bool *during construction*, before `.show()` is even called:

  ```rust
  pub fn new(anchor_response: &Response, id_salt: impl Hash) -> Self {
      let id = anchor_response.id.with(id_salt);
      if anchor_response.clicked() {
          let ctx = &anchor_response.ctx;
          let is_open = ctx.memory(|mem| mem.data.get_temp::<bool>(id).unwrap_or(false));
          ctx.memory_mut(|mem| mem.data.insert_temp(id, !is_open));
      }
      Self { id, anchor_response: anchor_response.clone(), ... }
  }
  ```
- **Problem**: Builder construction is supposed to be inert — only `.show()` commits work. Today's call sites (e.g. `combo_box.rs:116`) always chain `.show()` immediately, so the bug is latent, but the contract permits a caller to construct a `PopupMenu` under a conditional and never show it; the toggle fires anyway and leaves stored state out of sync with what's on screen. The `#[must_use]` attribute we use on `ScopedGui` isn't here either.
- **Alternative**: Store the anchor response verbatim, read `.clicked()` only inside `.show()` before the open-check. Add `#[must_use = "PopupMenu does nothing until .show() is called"]`. Both cheap.
- **Recommendation**: Do it.

### [F2] Inconsistent `StableId` placement across widget APIs

- **Category**: Abstraction (API shape)
- **Impact**: 3/5 — predictability, not correctness. Every widget type forces the caller to re-learn the id contract.
- **Effort**: 3/5 — surface-level signature churn across ~8 widgets, touching ~30 call sites
- **Current**: Four distinct id-passing conventions across the cluster:
  - **`id` in `show(gui, id, closure)`**: `Button::show`, `ComboBox::show`, `DragValue::show`, `FilePicker::show`, `ListItem::show`, `Frame::show`, `ColumnFlow::show` — builder-chain-neutral id arg.
  - **`id` in constructor (`new(id, ...)`)**: `HitRegion::new`, `StatusPanel::new`, `PositionedUi::new`, `Area::new`.
  - **`id` as optional builder method (`.id(id)`)**: `ScrollArea::id`, `Expander::id`, `ColumnFlow::id` (also accepts via show).
  - **No explicit id**: `Label`, `Space`, `Separator`, `Image`, `Texture` (none need persistence).
- **Problem**: Frame's comment at `frame.rs:75–81` explains why it takes id in `show`; the same rationale applies to `Area`/`PositionedUi` but they took the id in `new`. `PopupMenu::new` derives id from the anchor response rather than accepting a `StableId` at all. There's no documented rule, so the reader has to consult each widget's signature. Worse: widgets that currently don't need an id (`Label`) might later, and callers can't predict which form the new arg will take.
- **Alternative**: Pick one rule and enforce it.
  - **Rule A**: interactive & stateful widgets always take `StableId` as the **second arg** of `show` (matches egui's `ui.add_widget_with_id(id, ...)`). Constructor stays id-free.
  - **Rule B**: widgets that need id at construction time (to key memory, e.g. `PopupMenu`, `PositionedUi`) take it in `new`; pure-rendering widgets take it in `show`. The rationale is "does this widget read memory before `.show()` can run?".
  - Rule A is more uniform; Rule B captures a real distinction.
- **Recommendation**: Adopt Rule A and migrate. The migration is mechanical; the payoff is that every caller looks the same.

### [F3] `DragValue` edit-state machine lives in four memory keys, not a type

- **Category**: State
- **Impact**: 3/5 — fragile, hard to trace, duplicated unpacking logic
- **Effort**: 3/5 — internal refactor, no public API change
- **Current**: `drag_value.rs:124–289` — four memory keys (`drag_temp_id`, `edit_id`, `edit_text_id`, `edit_original_id`), each independently loaded (lines 125, 148, 160, 162), mutated across click/drag/confirm/cancel paths (lines 199–207, 221–224, 232–234, 239–241), and cleared in either of two places (lines 203–206, 239–241). An implicit state machine with three states — idle / dragging / editing — is smeared across five `if` branches.
- **Problem**: The state is implicit. There's no single place that says "when transitioning idle→editing, set these four keys; when confirming, clear them." A missed clear leaves stale keys that corrupt the next interaction on a different `DragValue` with the same id space. The bug class isn't theoretical: four keys × three transitions = twelve coordination points to get right.
- **Alternative**: One struct:
  ```rust
  #[derive(Clone, Serialize, Deserialize)]
  enum DragValueState<T> {
      Idle,
      Dragging { start: T, current: T },
      Editing { text: String, original: T },
  }
  ```
  Load once at the top of `show` via `gui.load_temp::<DragValueState<T>>(id)`, match on the variant, store once at the end. Five transitions compile-check themselves. The encoding is the same size; it's just the type system doing the bookkeeping.
- **Recommendation**: Do it. This is where the new `Gui::load_temp<T>` generic helper earns its keep.

### [F4] `FilePicker` couples its outer hit-test to an inner `Button`'s id

- **Category**: Leaky abstraction
- **Impact**: 3/5 — silent correctness hazard if `Button`'s id scheme ever changes
- **Effort**: 1/5 — allocate the outer id explicitly
- **Current**: `file_picker.rs:124` creates the browse button with `StableId::from_id(id.id().with("browse"))`, then `file_picker.rs:145` registers the outer-rect interact with `browse_response.id` — reusing the inner button's widget id for the wrapper rect:
  ```rust
  let widget_response = gui.ui_raw().interact(rect, browse_response.id, Sense::hover());
  widget_response | browse_response
  ```
- **Problem**: The outer widget's id is load-bearing on an internal `Button` id. If Button's id derivation ever changes (it already has a comment about a drift bug — `button.rs:131–146`), the outer interact silently aliases something else. It also bakes a non-local invariant into the file.
- **Alternative**: Allocate an outer id explicitly: `let outer_id = id.with("file_picker_outer");` and use that. Compose the two responses the same way.
- **Recommendation**: Do it. Ten-minute fix.

## Considered and rejected

- **`Image` is dead code (Agent claim).** Not true — used at `node_details_ui.rs:312` as the preview image widget in the selected-node panel, aliased as `ImageWidget` to avoid clashing with `palantir::Image`.
- **`PositionedUi`'s measured-size cache is dead (Agent claim).** Only partly true. The cache path is skipped when `.rect()` is set (`node_details_ui.rs:44`), but both `graph_ui/overlays.rs:33` and `overlays.rs:81` use the cache to pivot-align auto-sized button bars — so the cache is load-bearing for 2 of 3 call sites. The one-frame lag concern is real but minor for fixed-content bars.
- **`StatusPanel` should fold back into `log_ui.rs` (Agent claim).** Rejected — the whole point of the recent `EGUI_ENCAPSULATION_PLAN` follow-up was to pull chrome out of the app layer. Reinlining would reintroduce raw `egui::ScrollArea`/`CollapsingState` into the tripwire-guarded files.
- **`HitRegion` is too thin to earn its keep.** Keeper. Yes it's one line, but it's the sanctioned replacement for `ui.interact()` in app code — deleting it would punch a hole in the tripwire contract.
- **`ScrollArea`'s `Option<T>` boilerplate.** Noted but not worth a finding on its own; Impact 1/5.
- **`Layout` vs `ScopedGui` duplication.** Real but low priority: 3 call sites across the app. Fold it in opportunistically when `ScopedGui` grows the relevant setters (`min_width`, `fill_width`), not as a standalone task.
- **Migrating widgets off raw `data_mut(|d| d.get_temp(...))` onto the new `Gui::load_temp<T>` helpers.** Not a design-review finding — purely a code-polish task, belongs in an `/improve-module` pass.
