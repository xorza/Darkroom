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

## Repository hygiene

- [ ] `.tmp/` at the repo root holds build and run artifacts
      (`after.log`, `darkroom-run.log`, `vk-validation.log`, `menu-preview.png`,
      `lumos_pipeline_stack/`, `lumos-test/`, `wgpu/`) plus a stray
      `fits-well/.tmp/feature-consumer/Cargo.toml`. It is excluded from the
      workspace by `Cargo.toml:14` rather than absent.
