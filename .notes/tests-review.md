# Test and harness review

> Whoever addresses an item deletes it. When a section is empty, delete its heading too.

Review of all test code and test harnesses in `common`, `scenarium`, `lumos`, `lens`, `darkroom`, `fits-well`, `imaginarium` and `quickbench`. Palantir is not in the scope.

Groups are sorted by severity. Each group has one subsection for each scope, and each subsection gives the base of its short paths.

The production bugs that these tests hid are in `.notes/ISSUES.md`. The items here cover only the test side.

No test is slow. Each suite runs in less than 3.5 s, and the slowest single test takes 0.77 s (`registration::tests::transform_types::a_rung_that_fit_survives_a_later_rung_failing`). In scenarium, approximately 85% of the serial suite time is wall-clock waiting (see Nondeterministic tests).

## Tests that cannot fail, or do not run the code their name claims
The fixture, the tolerance or the call path makes the assertion true whatever production does. These tests report coverage that does not exist, so they come first.

### darkroom, lens, imaginarium, quickbench, root `test_resources/`
Paths are relative to the repository root.


## Risky behavior with no test
Branches with real failure modes that no test reaches.

### darkroom, lens, imaginarium, quickbench, root `test_resources/`
Paths are relative to the repository root.

- [ ] `quickbench/src/lib.rs:289-306` (`compute_stats`) has no test. That leaves the empty-times path (`n = max(1)`) and the median definition untested; for an even count it takes the upper middle, `times[len/2]`. `bench_persists_results_when_output_dir_set` (`tests.rs:67-84`) never runs twice, so the `vs_previous` line is never produced. `compare_to_previous` (`tests.rs:52-65`) asserts only the signs of `pct`, never the values (+2.0 %, −20.0 %, +20.0 %), and does not test the ±5 % edges. Add a table with hand-computed means and medians and an exact percentage table.
- [ ] `quickbench/quickbench-macros/src/lib.rs` — no tests at all. The argument parser, the error for an unknown key, and the rule that iterations-only disables the time limits (`:174-184`) are all untested. Add a `tests/macro.rs` integration test that uses `#[quick_bench(ignore = false, warmup_iters = 1, iters = 3)]` and asserts the iteration count.
- [ ] `darkroom/src/gui/graph_ctx/tests.rs:98-101` — `known_node.inputs().len() > 0`. `Add` declares exactly 2 inputs, so assert `== 2`.

## Second sources of truth: tests and harness re-implement production or each other
A copy of a formula, a constant or a pipeline step changes with the code it copies, or drifts away from it. Either way it cannot catch the bug it exists for.

### darkroom, lens, imaginarium, quickbench, root `test_resources/`
Paths are relative to the repository root.

- [ ] `darkroom/src/gui/widgets/format/tests.rs:3-6`, `:28-29` — the "≤ 7 chars" bound is hand-linked by a comment to `RUN_TIME_MIN_WIDTH = 52.0` px (`gui/pane/graph/node/header.rs:57`). If either side changes, the test stays green. Measure `"999.99s"` with the header's text style and assert it fits `RUN_TIME_MIN_WIDTH`.
- [ ] `darkroom/src/gui/state/process_memory.rs:98-106` — the 1000 ms schedule is retyped instead of derived from `SAMPLE_INTERVAL` (`:18`). Compute the table from the constant.
- [ ] `darkroom/src/core/edit/step/tests.rs:266-271` — the `5e-5` and `2e-4` probes retype `VIEWPORT_EPS` (`step/set_viewport.rs:14`). Derive them as `EPS / 2` and `EPS * 2`.
- [ ] `darkroom/src/gui/pane/graph/gesture/pan_zoom/tests.rs:59-66`, `:94-98` — the expected value is `SCROLL_ZOOM_BASE.powf(18.0)`, the production formula, under a 1e-6 tolerance; the comment's hand value (≈1.04604) is never asserted. `:169-172` and `:187-190` compare a clamped zoom with a `1e-5` tolerance, but `clamp` returns the bound exactly, so use `assert_eq!`. Their comment also claims that pivot invariance holds at the clamp, and nothing asserts it. The tolerances at `:122`, `:199`, `:210`, `:213` give no reason.
- [ ] `darkroom/src/gui/pane/graph/gesture/breaker/tests.rs:116-126` — the comment says the point lands "at exactly (2000, 0)", while the assert allows 1e-4. Either make it exact or state why it cannot be: `3000·(2000/3000)` in f32 is not exact.

## Duplicate tests and fixtures that should be one table or one helper
The same fixture or property is written many times, with different ad-hoc tolerances. Each copy drifts, and one fix has to be made in many places.

### darkroom, lens, imaginarium, quickbench, root `test_resources/`
Paths are relative to the repository root.

- [ ] `darkroom/src/gui/state/preview_store/tests.rs:10-15` (`image_value`) vs `preview_store/mod.rs:308-312` (`internals::opaque_image_value`) — two image-value builders in one module. Keep `image_value(w, h, fmt)` in `internals` and define the opaque 2×1 case through it.
- [ ] `darkroom/src/core/document/harness.rs:105-115` (`DocFixture::stubs`) duplicates `stub_at` (`:133`). Its only callers are `pan_zoom/tests.rs:17`, which could use `stub_at` and its returned ids, and `:41`, where `stubs([])` is just `DocFixture::default()`.
- [ ] `darkroom/src/core/edit/graph_intent/tests.rs:32-43` (`func_node`, `add_node`) vs `darkroom/src/gui/app/session/mod.rs:344-356` (`func_node`, `add`) — both build `AddNode`, and in different ways. `graph_intent/tests.rs:156-161` and `session/mod.rs:366-371` then write the literal by hand anyway. One `GraphIntent` add builder beside `DocFixture` would serve both files.
- [ ] `darkroom/src/gui/pane/graph/gesture/pan_zoom/tests.rs:45-99`, `:101-151` — the four `scroll_to_zoom_factor` tests and the two pivot-invariance tests should each be one table.
- [ ] `darkroom/src/gui/pane/graph/gesture/breaker/tests.rs:132-173` — the three `intersects_cubic_*` cases should be one table.

## Stale, wrong and change-narrating comments
Comments that describe code that no longer exists, contradict the asserted values, or narrate history.

### darkroom, lens, imaginarium, quickbench, root `test_resources/`
Paths are relative to the repository root.

- [ ] Change narration in test comments: `darkroom/src/gui/graph_ctx/tests.rs:9` ("no longer asks"), `darkroom/src/core/document/mod.rs:475-477` ("they were four separate spellings … before"), `darkroom/src/core/io/document/tests.rs:71-72` ("Before this, save only asserted"), `darkroom/src/gui/app/session/mod.rs:263-264`, `:439-441` ("The single-slot arbitration this replaced"), `darkroom/src/gui/app/session/harness.rs:168`, `darkroom/src/gui/pane/graph/gesture/selection/tests.rs:14`, `darkroom/src/gui/pane/graph/node/port_color/tests.rs:47`, `darkroom/src/gui/pane/graph/paint/inspector.rs:448-449`, `darkroom/src/gui/requests.rs:134`, `darkroom/src/gui/pane/viewer/mod.rs:651-652`, `darkroom/src/gui/state/preview_store/tests.rs:21-23`, `darkroom/src/gui/app/discard_dialog.rs:163-166`.
- [ ] Stale or wrong descriptions:
  - `darkroom/src/gui/pane/graph/gesture/selection/tests.rs:21-23` says the row assigns "by map iteration order", but `DocFixture::add` places by slot count, deterministically.
