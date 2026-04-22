# Design review: prism/src/gui/widgets/  (2026-04-22)

## Current design

12 chrome widgets plus a `mod.rs` of re-exports, ~3000 LOC total. After the recent moves out of `common/`, this is now the legitimate "generic egui chrome" layer for the editor — every file depends on `Gui` + style, no scenarium types reach in. The widgets compose cleanly: `combo_box` → `popup_menu`, `popup_menu` → `area + button + frame`, `drag_value` → `text_edit`, `file_picker` → `button`, `column_flow` → `scroll_area`. No cycles.

The conventions are mostly consistent — builder-style construction, `show(self, gui, ...) -> Response`, `style: Option<X>` with `unwrap_or_else(|| gui.style.default)` fallback, optional `font` / `pos` / `align` fields where they make sense. The repetition (each widget rolls its own style/font fallback against a different default) is honest — each widget really does need a different default.

The outlier is `text_edit.rs` at 1280 LOC — a fork of egui's TextEdit, an order of magnitude larger than every other file in the directory. Its API is also the most idiosyncratic (`id` AND `id_salt` builder methods, different field-mutation pattern). Worth its own review separately.

## Overall take

The directory is in good shape after the structural reshuffle. Cross-widget composition is clean, builder patterns are consistent, dependency graph is acyclic. Two real consistency issues stand out: the `show()` ID parameter type varies in a way that bypasses the `StableId` discipline (real bug source), and the word "align" means two different things across widgets (cosmetic but misleading). One small dead-weight — `drag_value` calls `make_persistent_id` twice on the same key.

## Findings

### [F1] `show()` ID parameter type varies; two widgets bypass `StableId` discipline
- **Category**: Contract / Types
- **Impact**: 3/5 — the bypassing widgets re-introduce the rect-id-drift class of bugs the StableId rule exists to prevent
- **Effort**: 2/5 — change two widget signatures + adjust callers
- **Current**: Across all `show()` methods that take an id parameter:
  - **`StableId` (correct)**: `button.rs:105`, `file_picker.rs:55`, `popup_menu.rs:211` (`ListItem::show`), `positioned_ui` (constructor)
  - **`impl std::hash::Hash` (loose)**: `drag_value.rs:106`, `combo_box.rs:70`. These then call `gui.ui().make_persistent_id(&id_salt)` internally — which gives an `egui::Id` derived from the salt, not a `StableId`-pinned id.
  - **Other**: `text_edit` exposes both `.id(Id)` and `.id_salt(impl Hash)` builder methods; `expander` derives default id from text via `Id::new(&text)`.
- **Problem**: The CLAUDE.md rule says *every* widget id should come from `StableId`. `drag_value` and `combo_box` accept anything `Hash` and route it through `make_persistent_id`, which is the auto-counter-aware path egui uses for unstable ids. Callers can pass tuples or strings without getting the call-site-pinned salt that `StableId::new` provides. Two widgets is enough to silently regress a fix elsewhere — the next time someone hits a "rect changed id between passes" warning in code that calls `combo_box`, the cause won't be obvious.
- **Alternative**: Change `drag_value::show` and `combo_box::show` signatures to take `id: StableId` like the other widgets. Internally use `id.id()` to get the egui `Id`. Callers that currently pass `("foo", node_id)` as the salt change to `StableId::new(("foo", node_id))` — same site, more explicit.
- **Recommendation**: Do it. Two function signatures + ~5 caller updates.

### [F2] `align` field means two different things across widgets
- **Category**: Types / Naming
- **Impact**: 2/5 — readability + prevents one accidental cross-use
- **Effort**: 1/5 — rename one of them
- **Current**:
  - **`align: Align2`** (2D anchor against a `pos: Pos2`): `combo_box`, `drag_value`, `file_picker`. The widget calls `self.align.anchor_size(self.pos, total_size)` to position itself.
  - **`align: Align`** (1D text alignment within a fixed rect): `button`, `popup_menu::ListItem`. The widget aligns the text/icon within a pre-allocated rect.
- **Problem**: Same field name, two semantically different things. A reader scanning a builder chain `.align(...)` has to know which group the widget belongs to. A future widget author copying the pattern from `button` (using `Align`) into a free-positioned widget would be confused.
- **Alternative**: Rename to express the role, not the type:
  - `Align2` → `anchor: Align2` (the field controls *where* the widget sits relative to `pos`)
  - `Align` → `text_align: Align` (the field controls text within the widget)
  - Or: keep `align` for anchor, rename the inner-text version to `text_align`. Either flavor surfaces the distinction at the call site.
- **Recommendation**: Do it when next touching either widget. Not urgent; no real bugs flow from the current naming.

### [F3] `drag_value::show` calls `make_persistent_id` twice on the same id_salt
- **Category**: Dead weight
- **Impact**: 1/5 — wasted work; mildly confusing
- **Effort**: 1/5 — delete one line
- **Current**: `drag_value.rs:122` and `drag_value.rs:147` both compute `let id = gui.ui().make_persistent_id(&id_salt);` (the second call uses `id_salt` by value, moving it). Both produce the same `Id`. Looks like leftover from a refactor.
- **Recommendation**: Delete the second call; rely on the `id` already in scope.

## Considered and rejected

- **Extract a shared "style fallback" helper** to dedupe the `let style = self.style.unwrap_or_else(|| gui.style.X.clone());` pattern that appears in 6+ widgets. Each widget falls back to a *different* default style field (`gui.style.popup`, `gui.style.node.const_bind_style`, etc.), so a generic helper would still need a closure or trait per widget. The duplication is honest — each line says "use my style if provided, else this specific default."

- **Same for the `font: Option<FontId>` fallback.** Four widgets, four different default fonts (`mono_font`, `sub_font`, etc.). Same conclusion.

- **Force every widget to derive `Default`.** Only `Button` and `ScrollArea` have `Default`; the others require a value reference or anchor (e.g. `DragValue::new(&mut value)`, `FilePicker::new(&mut path, ...)`). Default doesn't make sense for those — there's no sensible "empty drag value." Current asymmetry is correct.

- **Split `text_edit.rs` (1280 LOC) into smaller files.** It's a fork of egui's TextEdit and stands as one logical unit. Splitting at file boundaries would obscure what's modified-from-egui vs original. Left for a dedicated review focused on the fork.

- **Reduce cross-widget composition** (e.g. inline the `Button` calls inside `popup_menu`). The composition is what makes the directory work — each widget is small because it leans on its neighbors. Inlining would duplicate styling logic.

- **Move `expander` and `column_flow` to a `layouts/` subdir** to separate "layout containers" from "interactive controls". Plausible but the directory is already small (12 files), and the distinction would need its own bookkeeping. Not enough payoff.

## Big-picture take

A small, cohesive directory with one real consistency bug (F1) and one cosmetic naming inconsistency (F2). F1 is worth doing soon — every additional caller of `drag_value::show` or `combo_box::show` is another site that bypasses the `StableId` discipline and could become a source of "rect changed id" warnings. F2 + F3 are 5-minute polish.

If you only do one thing here, **F1**. If you also want a separate dedicated review, **`text_edit.rs`** is the obvious candidate — it's by far the biggest, oldest-shaped file in the directory and probably has its own design questions worth treating individually.
