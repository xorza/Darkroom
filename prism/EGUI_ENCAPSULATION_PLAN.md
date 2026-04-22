# Egui encapsulation plan

Goal: no code outside `prism/src/gui/widgets/` ever calls methods on
`egui::Ui` or `egui::Context`. Every interaction with egui from the
application layer goes through a widget in `gui/widgets/`.

## Scope

**In scope** — 46 `gui.ui()...` call sites across 10 application-layer files:
`node_details_ui.rs` (19), `new_node_ui.rs` (6), `node_ui.rs` (6),
`connection_bezier.rs` (6), `graph_ui/*` (6), `graph_background.rs` (1),
`log_ui.rs` (1), `value_editor.rs` (1).

**Out of scope** — 39 `gui.ui()` call sites inside `gui/widgets/` itself.
Chrome widgets necessarily talk to raw egui — that is their job.
Also out of scope: importing `egui` *types* (`Rect`, `Vec2`, `Color32`,
`Sense`, `Key`). Those are neutral POD-ish re-exports.

## Call-site inventory

| Pattern | Count | Target |
|---|---|---|
| `label(..)` / `colored_label(c, ..)` | 9 | `widgets::Label` |
| `add_space(..)` | 4 | `widgets::Space` |
| `separator()` | 1 | `widgets::Separator` |
| `image((id, size))` | 1 | `widgets::Image` |
| `interact(rect, id, sense)` + `allocate_response` | 11 | `widgets::HitRegion` |
| `make_persistent_id(str)` | 6 | replace with `StableId::new` (existing) |
| `set_min_width/height`, `set_max_height`, `set_clip_rect`, `take_available_width` | 7 | `ScopedGui` / `Frame` builder extensions |
| `is_rect_visible(r)`, `rect_contains_pointer(r)`, `input(\|i\|..)`, `ctx()` | 5 | `widgets::culling` free fns + `widgets::pointer` query fns |
| `ctx().load_texture(..)` | 2 | `widgets::texture::load` free fn |

## Widgets to add

1. **`widgets::Label`** — `Label::new(text).color(c).font(f).show(gui)`.
2. **`widgets::Space`** — `Space::size(amount).show(gui)`. Also
   `Space::padding()` and `Space::small_padding()` that read `gui.style`.
3. **`widgets::Separator`** — `Separator::horizontal().show(gui)`.
4. **`widgets::Image`** — `Image::new(texture_id, size).show(gui)`.
5. **`widgets::HitRegion`** — `HitRegion::new(id).rect(r).sense(s).show(gui) -> Response`.
   Rect defaults to `Rect::NOTHING`, sense defaults to `Sense::hover()`.
   Covers both `interact(r, id, s)` and `allocate_response(v, s)` (use
   `.size(v)` for the allocate path).
6. **`widgets::texture::load(gui, name, image, options) -> TextureHandle`** —
   thin wrapper; lives in `widgets/` because it touches `ctx()`.
7. **`widgets::culling`** — `is_rect_visible(gui, rect) -> bool`,
   `rect_contains_pointer(gui, rect) -> bool`. Free fns because they
   don't produce widgets, they answer questions.
8. **`widgets::pointer`** — `hover_pos(gui) -> Option<Pos2>`,
   `any_pressed(gui) -> bool`, `key_pressed(gui, Key) -> bool`, etc.
   Curated set covering the 4 `input(\|i\| ...)` app-layer sites.

## `ScopedGui` / `Frame` extensions

Layout constraints currently set by calling `ui.set_min_width(..)` etc.
inside a scope should be declared on the scope builder:

- `ScopedGui::min_size(Vec2)` → calls `ui.set_min_size` inside the body.
- `ScopedGui::max_height(f32)` → calls `ui.set_max_height`.
- `ScopedGui::clip_rect(Rect)` → calls `ui.set_clip_rect`.
- `ScopedGui::fill_width()` (covers `take_available_width`).

Mirror the same methods on `Frame` where they make sense.

## The gate

- Rename `Gui::ui()` → `Gui::ui_raw()` with doc comment "restricted to
  `gui/widgets/` — use widgets for everything else".
- Add tripwire test next to `no_bare_ui_builder_in_crate` in
  `common/id_salt.rs`: scan all `.rs` files outside `gui/widgets/` for
  `ui_raw()`; fail unless the line (or ≤2 above) carries `// egui-direct-ok`.
- Accept one whitelisted exception: `main.rs` / `main_ui.rs` where the
  very first `Gui::new(ui, ..)` wraps the root `egui::Ui` from eframe.

## Phased execution

Each phase compiles, tests green, is a viable stopping point. Commit
after each phase once I've verified.

### Phase 1 — display widgets (Label, Space, Separator, Image)
Rewrites 15 call sites across `node_details_ui.rs` and one each in
`node_ui.rs`/`log_ui.rs` if applicable.

- Add `widgets/label.rs`, `widgets/space.rs`, `widgets/separator.rs`,
  `widgets/image.rs`; register in `widgets/mod.rs`.
- Rewrite sites in `node_details_ui.rs`.
- `cargo nextest run -p prism && cargo clippy --all-targets -- -D warnings`.

### Phase 2 — HitRegion widget
Rewrites 11 call sites.

- Add `widgets/hit_region.rs`.
- Rewrite sites in `node_details_ui.rs`, `new_node_ui.rs`, `node_ui.rs`,
  `connection_bezier.rs`, `graph_ui/mod.rs`, `graph_ui/connections.rs`,
  `value_editor.rs`.

### Phase 3 — `make_persistent_id(str)` → `StableId::new`
Rewrites 6 call sites. No new widget; uses existing `StableId`.

### Phase 4 — culling / pointer query helpers
Rewrites 5 call sites.

- Add `widgets/culling.rs` and `widgets/pointer.rs` (or fold into one
  `widgets/query.rs`).
- Rewrite sites in `node_ui.rs`, `connection_bezier.rs`, `graph_ui/mod.rs`,
  `graph_ui/pan_zoom.rs`.

### Phase 5 — ScopedGui / Frame layout constraints
Rewrites 7 call sites.

- Extend `ScopedGui` in `gui/mod.rs` with `.min_size`, `.max_height`,
  `.clip_rect`, `.fill_width`.
- Extend `widgets/frame.rs` with the subset that makes sense for frames.
- Rewrite sites in `new_node_ui.rs`, `graph_ui/mod.rs`, `log_ui.rs`,
  `graph_ui/overlays.rs`.

### Phase 6 — texture loader
Rewrites 2 call sites.

- Add `widgets/texture.rs::load(gui, name, image, options) -> TextureHandle`.
- Rewrite `graph_background.rs` and `node_details_ui.rs`.

### Phase 7 — gate + tripwire
- Rename `Gui::ui()` → `Gui::ui_raw()`.
- Add tripwire test to `common/id_salt.rs`.
- Verify the only remaining `ui_raw()` call sites are inside
  `gui/widgets/` (plus maybe one whitelisted root wrapping).

## Non-goals

- Not wrapping egui *types*. Using `egui::Rect`, `egui::Vec2`, etc. from
  app code stays allowed.
- Not redesigning the `Gui` wrapper surface. Methods it already has
  (`horizontal`, `vertical`, `scope`, `painter`, `scale`, `font_height`,
  `rect`, `style`) stay.
- No compatibility shims. `Gui::ui()` gets renamed wholesale; all 85 call
  sites update in one go per phase.

## Estimated effort

~300 lines added (widgets are small), ~60 lines deleted, ~150 lines
rewritten. No architectural risk; each phase independently verifiable.

## Status

- Phases 1–7 applied. 55/55 tests pass, clippy clean.
- New widgets added: `Label`, `Space`, `Separator`, `Image`, `HitRegion`,
  `Layout`, plus `texture::load` helper.
- `Gui::ui()` renamed to `Gui::ui_raw()` with tripwire test
  (`no_raw_ui_outside_widgets`) forbidding its use outside `gui/widgets/`.
- Queries (`is_rect_visible`, `rect_contains_pointer`, `pointer_hover_pos`)
  live as read-only methods on `Gui<'_>` — they answer questions, they
  don't render, so they stayed off the widget axis per review feedback.
- **One whitelisted outlier**: `gui/log_ui.rs` still builds directly on
  raw egui chrome (`CollapsingState`, `egui::Frame`, `egui::ScrollArea`).
  Pending rewrite as a dedicated `StatusPanel` widget — punchy enough to
  deserve its own pass.
