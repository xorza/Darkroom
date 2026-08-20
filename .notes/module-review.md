# Workspace review

> **Delete each item as you address it.** This file lists open findings only — no
> "done" markers, no resolved section, no history.

Scope: all nine workspace crates (~219k lines of production Rust, excluding
tests and benches). Findings are grouped by root cause and sorted by severity ×
benefit. Test structure and test-facing APIs are out of scope.

---

## Doc prose has drifted from the code it describes

The codebase carries very high comment density (darkroom 32% of production
lines, palantir 30%) and the prose is load-bearing — it is where invariants,
sweep responsibilities and cross-module contracts are recorded. That prose is
not compiler-checked: `broken_intra_doc_links = deny` only validates `[link]`
form, so backticked identifiers in prose rot silently. Several already have.

- [ ] Identifiers asserted by doc prose that are defined nowhere in the
      workspace:
      - `PORT_HIT_SCALE` — `darkroom/src/gui/pane/graph/node/header.rs:66`
        ("the `PORT_HIT_SCALE`-grown box")
      - `MULTI_CLICK_RADIUS` — `palantir/src/input/capture.rs:33`, which is the
        doc comment sitting directly above the constant it means,
        `DOUBLE_CLICK_RADIUS` (`:34`)
      - `Full` and `AnimOnly` — `palantir/src/ui/wake_reasons.rs:5-6` names them
        as the two `Ui::frame` processing modes; `FrameProcessing`
        (`palantir/src/ui/frame_report.rs:21`) has `PaintOnly`, `SingleLayout`
        and `DoubleLayout`
      - `ElementSlots` — `palantir/src/layout/types/align.rs:104`, cited as an
        example of an existing packed field
- [ ] 326 comments across the workspace describe superseded designs rather than
      the current one — "used to", "formerly", "the old …", "replaces the …",
      "it replaced". Concentrated in production doc comments
      (`darkroom/src/core/document/dock/mod.rs:2`,
      `darkroom/src/gui/graph_ctx/mod.rs:16`,
      `darkroom/src/gui/dock/strip.rs:24`, `palantir/src/ui/mod.rs:90-97`),
      where they describe code a reader cannot see and cannot verify.
- [ ] `darkroom/src/core/document/mod.rs:262-299`: `holds_node`'s doc carries a
      markdown table naming three caches and the three functions that sweep
      each, plus instructions to extend the table when a fourth is added. The
      table is the only thing linking those call sites; nothing enforces it.

## Files still holding several unrelated major types

The convention is one major struct per file, named for it, with satellites
allowed. These three still hold four or more independent types, each with its
own inherent impl, so the file name identifies none of them.

- [ ] `fits-well/src/wcs/mod.rs` — 1781 lines, 8 major types including a
      four-type table-WCS family (`TableWcs`, `TableWcsResolver`,
      `TableAxisKeyword`, `TableMatrixKeyword`, `TablePoleKeyword`).
- [ ] `fits-well/src/compress/decode.rs` — 1250 lines, 9 types (`DecodeBuffer`,
      `FloatQuantization`, `ImageDecodePlan`, `ImageLayout`, `ImageRegionLayout`,
      `NullMask`, `TileCells`, `TileScratchSet`, `TileSources`).
- [ ] `fits-well/src/table/mod.rs` — 1198 lines, 9 types (`BinTable`,
      `BitColumn`, `CharacterField`, `ColumnData`, `ColumnReader`,
      `TableSchema`, `Tform`, `TformKind`, `VlaColumn`).

## Inherent impls are split across files

A type's inherent impl belongs in the type's own file; these are spread, so the
full method set of each type has no single place to read.

- [ ] `App` — `darkroom/src/gui/app/mod.rs` plus five command files
      (`commands/{mod,edit,file,prefs,run}.rs`). Six files, six `impl App`
      blocks.
- [ ] `FitsWriter` — `fits-well/src/writer/{mod,ascii,image,table}.rs`.
- [ ] `Document` and `GraphView` — `darkroom/src/core/document/mod.rs` and
      `.../document/validate.rs`.
- [ ] `Blend`, `ContrastBrightness`, `Transform` — each split between
      `imaginarium/src/ops/<op>/mod.rs` and `.../gpu.rs`.
- [ ] `FrameCache` — `lumos/src/stacking/combine/cache/mod.rs` and
      `.../cache/loader/mod.rs`.
- [ ] `TestGraph` — `scenarium/src/testing/graph/mod.rs` and
      `.../graph/compiled.rs`.

## `lumos` statistics duplicates itself across `f32`/`f64`

`lumos/src/math/statistics/mod.rs` carries three pairs of functions whose bodies
are identical modulo the float type, each pair with its own doc comment
explaining that it is the twin of the other, and each with its own test.

- [ ] `median_f32_mut:96` and `median_f64_mut:119` — identical bodies
      (`select_nth_unstable_by` with `total_cmp`, even-length averaging).
- [ ] `median_f32_fast:166` and `median_f64_fast:185` — identical bodies
      (single `partial_cmp` partition, upper-middle convention, same debug
      NaN assertion).
- [ ] `mad_f32_fast:232` and `mad_f64_fast:201` — identical bodies.
- [ ] `mad_f32_fast:232` and `mad_f32_with_scratch:248` differ only in which
      median they call and whether they debug-assert; two names for one
      operation parameterised by NaN tolerance.
- [ ] Three separate sigma-clip/MAD implementations exist alongside the above:
      `statistics::sigma_clipped:331`,
      `stacking/star_detection/centroid/local_background.rs:91`
      (`sigma_clipped_median_mad`), and
      `stacking/combine/normalization/mod.rs:448` (`cancellable_median_mad`),
      with a fourth reduced form in `stacking/combine/rejection/mod.rs:98`
      (`sorted_mad`).

## Allocation on the editor's per-frame record path

- [ ] `darkroom/src/core/document/mod.rs:216` (`GraphView::paint_order`) builds
      and sorts a fresh `Vec<(NodeId, ItemPlacement)>` on every call. Its
      production caller is `gui/graph_ctx/mod.rs:190`
      (`nodes_in_paint_order`), reached from the per-frame node loop at
      `gui/pane/graph/node/mod.rs:112` — so the vector is sized to the whole
      graph, allocated once per graph pane per frame, and built *before*
      culling drops the off-screen nodes.
- [ ] `palantir` exports `fmt!` (`palantir/src/lib.rs:188`) specifically as the
      allocation-free way to author a dynamic label, documented as landing bytes
      directly in the arena the widget would copy them into. `darkroom` uses it
      zero times and builds record-path labels with `format!` / `to_string()`
      instead — `gui/window/status_bar.rs:68,71` (two `String`s per frame for
      the memory readout), `gui/pane/graph/node/port_row/mod.rs:543,583,601,603`,
      `gui/pane/graph/node/preview_row.rs:127,148`,
      `gui/pane/graph/node/value_editor.rs:277,329`,
      `gui/pane/viewer/mod.rs:326`.

## `fits-well` error handling diverges from the rest of the workspace

- [ ] `fits-well/src/error.rs` declares one `FitsError` with 52 variants
      spanning reader, writer, header, ASCII/binary tables, WCS, time and
      compression. Every `Result` in the crate carries the whole set, so no
      call site's error type says which failures are actually reachable there.
- [ ] `fits-well/src/error.rs:276-497` is a hand-written 220-line
      `impl fmt::Display` — one `write!` arm per variant, restating each
      variant's doc comment as a format string. `thiserror` is already a
      workspace dependency used by 36 files across six crates.
- [ ] `imaginarium/src/common/error.rs:18` hand-writes `Display` for its `Error`
      while `imaginarium` depends on `thiserror` and uses it elsewhere in the
      same crate.
- [ ] `lumos/src/error.rs:48,139` hand-writes `Display` for
      `FrameDimensionMismatch` and `InvalidConfigField` while `lumos` uses
      `thiserror` in 12 other files.

## Unintegrated code kept alive by blanket suppressions

- [ ] `lumos/src/stacking/registration/distortion/tps/` is 1413 lines (383
      production + 1030 tests) reachable from nothing. `mod.rs:4` carries a
      module-wide `#![allow(dead_code)]`. It compiles, links, and is tested on
      every run of the suite.
- [ ] `lumos/src/stacking/registration/distortion/point_normalization.rs:32`
      (`denormalize`) is `#[allow(dead_code)]` and, per its own comment, exists
      only for the unintegrated TPS module.
- [ ] `lumos/src/stacking/registration/ransac/mod.rs:98` — an
      `#[allow(dead_code)]` struct field (`iterations`) that is written and
      never read.
- [ ] `palantir/src/common/platform.rs:9` — `#[allow(dead_code)]` on the
      `Platform` enum.
- [ ] `imaginarium/src/gpu/slot.rs:45` — `#[allow(dead_code)]` on `take`.
- [ ] `darkroom/src/gui/widgets/inline_rename.rs:127` — `#[allow(dead_code)]` on
      the `halign` builder method.
- [ ] Eighteen `#[allow(clippy::too_many_arguments)]` sites, concentrated in
      `lumos/src/io/raw/demosaic/xtrans/` (four),
      `lumos/src/stacking/star_detection/median_filter/simd/` (four) and
      `fits-well/src/compress/hcompress.rs` (three).

## `imaginarium`'s op modules repeat their own preamble and dispatch

- [ ] `ops/blend/cpu.rs:14-37` opens with six `assert_eq!`s pairing
      src/dst/output width, height and format. The same shape recurs in
      `ops/transform/cpu.rs:32` and `common/image_diff.rs:17,76`, each with its
      own message wording.
- [ ] `ops/blend/cpu.rs:44-93` is a three-stage dispatch — x86 SSE4.1 match, then
      aarch64 NEON match, then a scalar `match (channel_size, channel_type)`
      ending in `unreachable!`. `ops/contrast_brightness/cpu/mod.rs:67` and
      `ops/transform/cpu.rs:57` each re-implement the same cascade with
      different structure.
- [ ] `image/conversion/simd/mod.rs` repeats `if cpu_features::has_avx2()` at
      seven separate sites (`:137,298,324,350,376,402,428`), each selecting
      between the same two backends.
- [ ] `ops/blend/cpu.rs:42` — `let _ = channel_count; // Used in cfg-gated SIMD
      dispatch below`, a warning suppression standing in for a binding that
      belongs inside the `cfg` blocks that read it.
- [ ] `imaginarium` doc comments restate their own signatures:
      `ops/blend/mod.rs:69-76` spends a `# Arguments` list saying `src` is the
      source image and `output` is the output image; `ops/blend/mod.rs:14-28`
      annotates each `BlendMode` variant with the formula the variant name
      already gives. This is the only crate in the workspace written that way.

## `palantir::Ui` is the crate's shared mutable state

- [ ] `palantir/src/ui/mod.rs:88` — `Ui` holds 17 fields, 11 of them
      `pub(crate)` (`forest`, `state`, `resources`, `layout_engine`, `layout`,
      `cascade`, `cascade_engine`, `display`, `damage_engine`, `frame_runtime`,
      `window_requests`, `window_frame`). Every subsystem in a 92k-line crate
      reaches the others through it, and the boundaries between layout, cascade,
      damage, render and input are enforced only by convention. `impl Ui`
      carries 69 methods in that one file.

## Repository hygiene

- [ ] `.gitignore:18` ignores `/.gitmodules` under a "Claude Code sandbox
      sentinel files" heading. The file is tracked and is load-bearing — four
      submodules depend on it — so the rule is inert today and a hazard the
      moment the file is re-added.
- [ ] `.tmp/` at the repo root holds build and run artifacts
      (`after.log`, `darkroom-run.log`, `vk-validation.log`, `menu-preview.png`,
      `lumos_pipeline_stack/`, `lumos-test/`, `wgpu/`) plus a stray
      `fits-well/.tmp/feature-consumer/Cargo.toml`. It is excluded from the
      workspace by `Cargo.toml:14` rather than absent.
