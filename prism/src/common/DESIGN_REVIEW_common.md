# Design review: prism/src/common/  (2026-04-22)

## Current design

22 files, 3770 LOC, sitting between the workspace-level `common/` crate (egui-free utilities) and the `gui/` module (the editor's actual UI). The intent was "shared building blocks" — both pure helpers (`id_salt`, `bezier_helper`, `polyline_mesh`, `image_utils`, `primitives`, `font`, `ui_equals`, `undo_stack`) and reusable chrome widgets (`button`, `combo_box`, `expander`, `frame`, `popup_menu`, `text_edit`, `drag_value`, `value_editor`, `file_picker`, `area`, `column_flow`, `positioned_ui`, `scroll_area`).

The actual structure has drifted. Of the 22 files, **12 import `crate::gui::Gui`** (the editor's central rendering wrapper) and most also import `crate::gui::style::*`. Two — `value_editor` and `file_picker` — additionally import `scenarium::data::*` (domain types). Only 8 files are genuinely Gui-free utilities. So `common/` currently bundles three different things (pure helpers, gui-coupled chrome, domain-coupled editors) under a single name that suggests "no upstream deps."

## Overall take

The "common = pure utilities" framing has slipped. The widgets that depend on `Gui` are doing the same upward-into-`gui/` reach we've fixed one-file-at-a-time recently (`connection_bezier`, `polyline_mesh`, `action_undo_stack`) — but it's pervasive here, not isolated. Cleaning it up is structural work, not a polish pass. The right answer is splitting `common/` along the dependency boundary, not patching individual files.

## Findings

### [F1] Pervasive `common → gui` upward dependency (12 of 22 files)
- **Category**: Abstraction / Layering
- **Impact**: 4/5 — it's the same layering bug we keep hitting, but at directory scale; fixing it preempts a long tail of one-file-at-a-time corrections
- **Effort**: 4/5 — 12+ file moves, mod.rs surgery, every consumer's `use` path updates
- **Current**: Every chrome widget (`button`, `combo_box`, `expander`, `frame`, `popup_menu`, `positioned_ui`, `scroll_area`, `text_edit`, `area`, `column_flow`, `drag_value`, `value_editor`, `file_picker`) lives in `common/` but imports `crate::gui::Gui` and (often) `crate::gui::style::*`. The "lower" layer reaches up into the "higher" one.
- **Problem**: The directory name is a lie. A reader expects `common/` to be free of upstream deps; instead 60% of its files depend on `Gui` and `Style` defined in `gui/`. Refactoring is fragile because the structural protection (Rust's module hierarchy) doesn't bite — adding a cycle just compiles. Worse, every recent layering fix (`connection_bezier`, `polyline_mesh`, `action_undo_stack`) was an instance of this same pattern; the underlying organization keeps producing them.
- **Alternative**: Split `common/` along the dependency boundary.
  - **`common/`** keeps the genuinely pure helpers: `id_salt`, `ui_equals`, `bezier_helper`, `polyline_mesh`, `image_utils`, `primitives`, `font`, `undo_stack`.
  - **All 12 Gui-coupled widgets move into `gui/`** (or a `gui/widgets/` subdir to keep `gui/`'s root tidy).
  - `Gui` and `Style` stay where they are. The widgets are now neighbors of their dependency.
  - The two scenarium-coupled widgets (see F2) move further downstream.
- **Recommendation**: Do it as one bulk pass. Painful in diff size but mechanical (mostly file moves + `use` updates). Would have prevented all three recent one-file layering fixes.

### [F2] `value_editor` and `file_picker` import scenarium types — domain knowledge in "common"
- **Category**: Responsibility
- **Impact**: 3/5 — fixes a category mistake (these aren't generic widgets)
- **Effort**: 2/5 — two file moves + `use` path updates
- **Current**:
  - `value_editor.rs:4`: `use scenarium::data::{DataType, EnumDef, StaticValue};` — `StaticValueEditor` is an editor specifically for scenarium's `StaticValue`.
  - `file_picker.rs:4`: `use scenarium::data::{FsPathConfig, FsPathMode};` — picks file paths constrained by scenarium's `FsPathConfig`.
- **Problem**: A "common widget" that knows about `scenarium::data` is a domain-specific widget. They sit alongside `Button` and `Frame` (genuinely generic) but they aren't the same kind of thing. A reader looking for the `StaticValue` editor wouldn't think to look in `common/`; a reader picking widgets from `common/` for a new place might incorrectly assume `value_editor` is generic.
- **Alternative**: Move both into `gui/` (or wherever the F1 split puts the chrome). They're consumed exclusively by `gui/const_bind_ui` (`StaticValueEditor`) and `value_editor` itself (`FilePicker` has no other caller).
- **Recommendation**: Do it. Fold into the F1 move if you do that one; otherwise standalone.

### [F3] `font.rs::ScaledFontId` is dead
- **Category**: Dead code
- **Impact**: 1/5
- **Effort**: 1/5
- **Current**: `font.rs:3-14` defines a `ScaledFontId` trait with one method (`scaled(&self, scale: f32) -> FontId`) and one impl on `FontId`. Grep shows zero callers. Style scaling actually goes through a local `scaled()` function in `gui/style.rs` (line 176+).
- **Problem**: Dead trait + dead impl. Survives because clippy's dead-code lint doesn't trigger on pub items at the crate level — they look like API.
- **Alternative**: Delete `font.rs` entirely (the file contains only the dead trait + impl) and drop `pub mod font;` from `common/mod.rs`.
- **Recommendation**: Do it. 30-second cleanup.

### [F4] `FilePicker` has no callers outside `common/` itself
- **Category**: API
- **Impact**: 1/5 — borderline whether this is a real finding
- **Effort**: 1/5
- **Current**: `FilePicker` is `pub struct FilePicker<'a>` in `file_picker.rs`. The only consumer is `value_editor.rs:9`, which is in the same directory. Re-exported via `pub use file_picker::FilePicker;` in `mod.rs:24`.
- **Problem**: The `pub` + the re-export advertise a public widget that isn't actually used externally. That's an API claim with no callers.
- **Alternative**: Drop the re-export from `mod.rs` and downgrade visibility to `pub(super)` (or `pub(in crate::gui)` post-F1). Saves one line of pretend API.
- **Recommendation**: Borderline — if F1 + F2 land, both files end up next to each other anyway and `pub(super)` becomes natural. Defer.

## Considered and rejected

- **Rename `common/` → `widgets/` or `chrome/`.** Tempting, but the issue isn't the name — it's that the directory contains *both* widgets and pure helpers with different layering rules. Renaming wouldn't fix that, just rename the same problem.

- **Move `Gui` and `Style` from `gui/` down into `common/`.** This is the alternative direction for F1 — instead of moving widgets up to `gui/`, move the foundational types down. Smaller diff (2 files vs 12+), but it's the wrong direction: `Gui` is the framework heart, the widgets are accessories. Putting the framework in "common" muddies the gui/ identity ("if Gui isn't in gui/, what's gui/ for?"). The widgets should be siblings of their framework, not the other way around.

- **Introduce a third tier** (`common/` → `widgets/` → `gui/`). Looks clean on paper but creates a circular layering temptation: widgets need `Gui` (top), `Gui` needs the chrome to render itself (middle). With a third tier you'd have to maintain the layer discipline manually anyway. F1's "fold widgets into `gui/`" is structurally simpler.

- **Make `font.rs::ScaledFontId` actually used** (refactor `gui/style.rs::scaled()` to call it). The trait wraps trivial arithmetic — `FontId { size: self.size * scale, family: self.family.clone() }`. The local helper in style.rs is a free function with no `self`. Promoting it to use the trait would gain nothing semantically. Just delete the trait.

- **Split `text_edit.rs` (1280 LOC, biggest file in the directory).** That's a separate review, not a `common/` structural concern. Worth dedicated treatment if you want it.

## Big-picture take

`common/` as it stands is doing three jobs (pure helpers, gui-coupled widgets, domain-coupled editors) under a single name. F1 is the meaningful structural fix; F2 is its smaller cousin; F3 + F4 are 1-minute cleanups.

If you only do one thing here, **F1 is the lever** — it removes the pattern that's been generating one-off layering corrections for the past several reviews. Painful diff, but you stop fishing the same bug out of new files.

If you don't want to do F1, at minimum land **F2 + F3** — they're small and unambiguous wins on their own, and they don't conflict with F1 if you decide to do it later.
