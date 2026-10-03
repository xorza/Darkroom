# Workspace review

> **Delete an item when you address it.** This file lists open findings only: no "done"
> markers, no history. When a group has no items left, delete its heading too.

Scope: production code of `common`, `quickbench`, `fits-well`, `imaginarium`, `scenarium`,
`lumos`, `lens` and `darkroom` (`palantir` is out of scope). Tests and the APIs they use were
not reviewed. Findings already listed in `lumos/.notes/*.md` are not repeated here.

Items are anchored to file paths and symbol names; line numbers, where given, go stale fast.
Groups are named after their shared root cause and ordered by severity, then benefit.
"Probe" means the claim was confirmed by running code against a scratch copy.

---

# High — wrong results, data loss, crashes on user data

# Medium — wrong in edge cases, duplicated truths, hot-path waste

# Low — duplication, dead surface, local simplifications

## Placeholder values and derived fields stored beside their source
Severity: Low — `Option`s that must be `Some`, sentinels, and caches of computable values.

- [ ] `darkroom` — `gui/frame/geometry/mod.rs` `PortLayer::record`/`snapshot` `node_min: Option<Vec2>` is always `Some`; `widgets/port_glyph.rs` `PortGlyph::new` defaults the fill to a `WHITE` every caller overrides; `theme/card_theme.rs` `CardBorder` is a one-field wrapper whose only caller reads `.color`; `Theme` derives serde (only a test uses it) and serializes derived values (`const_value_editor_revealed`, `inline_rename_title`, `menu_button`, `palantir_theme`) beside their sources; `Theme::build` stores `text: TypeScale::DEFAULT` and passes a separate `&TypeScale::DEFAULT` to `palantir_theme_for` / `menu_button_for`.

## darkroom core keeps containers and checks it does not need
Severity: Low.

- [ ] Defensive branches that disagree with document invariants: `gui/app/mod.rs` `App::run_node`'s `debug_assert!(false)` cites panes and paths that no longer exist (the reachable case is stale data, covered by `RuntimeHost::run_node`); `gui/graph_ctx/mod.rs` `GraphCtx::nodes`/`node` skip "a placement whose node is gone" while `NodeState::capture` and `GraphIntent::duplicate` `expect` the bijection.
- [ ] Small duplications: `core/document/mod.rs` `GraphView::validate` counts placements with a loop (`item_placements.len()`); `core/io/document/mod.rs` writes size and extension checks twice, once per error enum, in opposite control-flow styles.

## darkroom GUI repeats per-frame lookups and theme values
Severity: Low.

- [ ] `ConnectionUI::apply`, `SubscriptionUI::apply`, `preview_drag` `scan_output_drag_start` — on every idle frame each walks every port, event and pin through a `HashMap` probe for a one-frame `drag_started` edge that `CanvasGeometry::rebuild` already saw. `MainWindow::reconcile` → `CanvasGeometry::retain_nodes` walks every cached offset with a `graph.find` each frame to catch rare deletions. `NodeCtx::exec_status` (a hash lookup) is re-resolved at each call site (`node_shadow`, three times in `header.rs` `status_row`, once per input in `port_row.rs` `input_label_cell`). `connection/mod.rs` `data_tint` via `port_data_type` does two node lookups and two `DataType` clones per visible wire per frame (`accepts_wire` repeats it). `port_row/mod.rs` `output_cell` collects consumers into a `Vec` only to iterate it.
- [ ] Theme state read two ways: `port_row/mod.rs` `ports_row` sizes tracks from `theme.palantir_theme.text.font_size_px` while labels use `ui.theme().text`; `preferences/mod.rs` `model_row` clones the whole `TextEditTheme` per frame per broken path to recolour a border (the pattern `const_value_editor_revealed` exists to avoid); `const_value_editor_theme.rs` `VALUE_EDITOR_MAX_WIDTH` doc says `Theme::build` reads it.
- [ ] Theme values hard-coded in panes: `paint/inspector.rs` `Inspectors::draw_one` builds `Shadow::drop(ambient_shadow, (0,3), 12.0)` by hand instead of `theme.card.elevation_shadow(12.0)` (whose doc names the inspector), border `1.0` literal; `node/header.rs` `status_row` `Spacing::new(8.0, 7.0, 8.0, 2.0)` repeats `CARD_HEADER_PAD_X/Y`, `gap(4.0)` repeats in five places, the run-time label uses `BADGE_FONT` (reserved for badge glyphs) with an inline style instead of `support::mono_text`; `port_row.rs` `port_label`/`event_cell` spell the same `TextStyle` twice; literals in `inspector.rs` `log_color`, `viewer/glyph.rs` `draw_swatch`, `preferences/mod.rs` `download_hint`, `widgets/badge.rs` `Badge::show`, `theme/palantir_bridge.rs` `tab_roles`. The `ViewerBackground` → fill mapping is written twice (`viewer/mod.rs`, `viewer/glyph.rs`); `node/memory_row.rs` pairs RAM↔`badge_cache`/VRAM↔`badge_graph` twice; `graph/toolbar.rs` `draw_play_bar` draws its own sharp triangle beside `support::play_triangle`.
- [ ] Pan/zoom spelled per surface: `gesture/shortcuts.rs` Ctrl+0 sets zoom 1.0 and keeps the pan (zooms about the top-left) while the toolbar's `Framing::Reset` centres and every other zoom anchors on cursor or centre; `viewer/mod.rs` `ImageViewer::apply_gestures` re-implements `pan_zoom::fold_pan_drag` with a bare `Option<Vec2>`; "did the scroll move" is written three ways (`fold_scroll_zoom` `abs() > EPSILON` without reason, `!= 0.0`, `!= Vec2::ZERO`); `scroll_to_zoom_factor`'s doc says it is shared with the viewer, which never calls it.
- [ ] Dead configurability: `widgets/inline_rename.rs` `InlineRename` — the only caller always passes `.style(..)`, so the `style: Option` fallback (rebuilding `flattened` per frame) and the `Right`/`Center` `halign` branches are test-only; `Theme::inline_rename` docs mention "boundary port" and "graph tab" renames that don't exist.
- [ ] `node/wid.rs` claims the node subtree's id vocabulary, but `port_row` `event_glyph_wid` (takes `(NodeId, usize)` instead of `EventRef`), `inspector` `inspect_badge_wid`/`inspect_panel_wid` and `preview_row` `preview_image_wid` use raw `WidgetId::from_hash` with ad-hoc prefixes.

## Docs that describe code that no longer exists
Severity: Low — prose that misleads readers about behaviour.

- [ ] `darkroom` core — `core/edit/validate.rs` says "saving validates only in debug builds" (it validates always); `Editor::frame`, `Editor::apply_edit`, `App::frame` are cited in `gui/app/mod.rs`, `gui/app/commands/mod.rs`, `gui/window/mod.rs`; `gui/app/mod.rs` mentions a `Load` variant (now `OpenPicked`); `gui/app/session/mod.rs` `menu_shortcut` claims Save-As ordering matters (palantir compares modifiers exactly); `PreviewStore::ingest_preview` self-references; the `Document` doc says "two halves are public" over three public fields.
- [ ] `lumos` — `star_detection/mod.rs` mentions "adaptive thresholding" and `measure_star` "Laplacian SNR" (neither exists), an orphan doc line sits above `compute_star`, `LabelMap::from_pool` says "Four (default)" (default is `Eight`), `compute_annulus_background` documents arguments it does not take; `Stretch::auto_stf`'s "0.25 is PixInsight's STF default" sits beside `0.2`, and `auto_asinh` is "gentler than STF's 0.25" though both are 0.2; `math/statistics` docs describe two entry points where one exists; `magsac/mod.rs` `MagsacScorer::new` derives `outlier_loss` from a removed formula; `ScratchBuffers` claims `for_each_init` allocation (it leases from `JobScratchPool`); `CacheConfig` says "(median, sigma-clipped)"; `stacking/mod.rs` omits `frame_store` and `stack_product`; `load_raw_cfa`'s doc is attached to `raw_cfa_frame_info`; `markesteijn`/`urect` cite a "pinned toolchain" the workspace does not have.

## Style rules not applied
Severity: Low — breaches of the standing Rust rules.

- [ ] Missing `const fn` on trivial accessors and constructors across the workspace: `lumos` `fwhm_to_sigma`, `sigma_to_fwhm`, `mad_to_sigma`, `mad_floored`, `MedianMad::sigma`, `Size2us::pixel_count`, `DMat3::{mul_mat, determinant}`, `cubic_spline_eval`, `TileStats::get`, `TileD2y::{get, get_mut}`, `stretching::mtf`, `wavelet::{reflect, max_scales}`, `QualityPlanes::resolve`, `PixelCoverage::new/contributes`, `SigmaBounds::symmetric/asymmetric`, `SmallN::none/median_below`, rejection `*Config::new`, `FrameSpill::new`, `CombinedSample::value_only`, `MasterRole::extname/prepared`, `CalibrationComponent::extname`, `ImageDimensions::{size, width, height, channels, is_grayscale, is_rgb}`, `BitPix::is_integer`, `CfaPattern::{flip_vertical, flip_horizontal, at_raw_origin, color_at}`, `CfaType::num_colors`, `SensorLayout::cropped`, `XTransPattern::color_at`, `libraw_filter_color`; `darkroom` has no `const fn` at all (`fmt_elapsed`, `fmt_bytes`, `PortTheme::radius`, `CardTheme::border_width_total`, `GestureSlot::{get, is_idle}`, `WireTint::{new, flat}`, `Chip::new`, `Badge::{control, action, marker}`).
- [ ] Several major types per file: `darkroom` `breaker/mod.rs` (three), `anchored_menu.rs` (two), `GlyphDrag` in `paint/wire/mod.rs`; files not named after their struct (`drag_anchor/` holds `GroupDrag`, `widgets/toolbar.rs` holds `Chip`).
- [ ] Other: `darkroom` `pan_zoom/mod.rs` imports through `graph/mod.rs`'s private `use`s; `frame/geometry/mod.rs` implements `GlyphKey` for `PortRef`/`EventRef` outside their files. `lumos` `image_ops/rgb/mod.rs` `Rgb` has `pub` fields on a `pub(crate)` struct; `math/statistics/float.rs` `impl_float!` generates two impls of seven one-line methods; `concurrency` `try_par_map_bounded`'s assert names another function's parameter.
