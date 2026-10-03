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

## darkroom GUI repeats per-frame lookups and theme values
Severity: Low.

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
