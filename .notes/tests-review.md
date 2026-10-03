# Test and harness review

> Whoever addresses an item deletes it. When a section is empty, delete its heading too.

Review of all test code and test harnesses in `common`, `scenarium`, `lumos`, `lens`, `darkroom`, `fits-well`, `imaginarium` and `quickbench`. Palantir is not in the scope.

Groups are sorted by severity. Each group has one subsection for each scope, and each subsection gives the base of its short paths.

The production bugs that these tests hid are in `.notes/ISSUES.md`. The items here cover only the test side.

No test is slow. Each suite runs in less than 3.5 s, and the slowest single test takes 0.77 s (`registration::tests::transform_types::a_rung_that_fit_survives_a_later_rung_failing`). In scenarium, approximately 85% of the serial suite time is wall-clock waiting (see Nondeterministic tests).

Two items need a manifest change, which needs your approval: `common` with `internals` in the `lens` dev-dependencies, and tokio `test-util` in the `lens` dev-dependencies.

## Tests that cannot fail, or do not run the code their name claims
The fixture, the tolerance or the call path makes the assertion true whatever production does. These tests report coverage that does not exist, so they come first.

### scenarium, common
Paths without a crate prefix are relative to `scenarium/src/`.


### darkroom, lens, imaginarium, quickbench, root `test_resources/`
Paths are relative to the repository root.

- [ ] `darkroom/src/core/document/mod.rs:490-494` — `!doc.holds_preview_node(node) || doc.holds_node(node)` runs right after `assert!(doc.holds_node(node))` (`:489`), so the right-hand side is always true and the assertion can never fail. The subset claim is only meaningful after the detach, and that is already checked at `:505-513`. Delete the loop assertion.
- [ ] `darkroom/src/gui/pane/graph/node/port_color/tests.rs:73-77` — "Same id → same color" compares `port_color(..custom(7)..)` with itself. That is a determinism tautology of a pure `%` lookup. Delete it.
- [ ] `lens/src/astro/nodes/tests.rs:130-131` — after `fs::remove_file(&first)` the test calls `frame_set_key(&[])`, the empty set, so the removal plays no part in it. The `FrameSetKeyError::Metadata` path (`calibration.rs:242`) is never exercised. Assert that `frame_set_key(&[first])` is `Err` naming the path. Separately, pin the order dependence of the key: `[a,b]` vs `[b,a]`.
- [ ] `darkroom/src/gui/pane/graph/gesture/breaker/tests.rs:60`, `.../connection/tests.rs:127`, `.../preview_drag/tests.rs:54` — "The harness carries the pane assertion: … commits against the pane …". `CanvasHarness::frame` (`gui/pane/graph/harness.rs:163-186`) asserts nothing about panes, and `DocumentRequest` (`gui/requests.rs:30-33`) carries no pane at all. These comments describe a check that no longer exists, so pane routing is not tested here. Delete the comments, or restore the assertion if routing still exists somewhere.
- [ ] `darkroom/src/gui/pane/viewer/camera.rs:83-90`, `:106-111` — the "on a 2x display … physical px" cases never pass a scale factor, because `fit_viewport` and `zoom_about_pane_center` take none. These are the same logical-space computation on a smaller image, labelled as DPI coverage. Rewrite the comments to say what is computed, or test the scale path where it actually lives.
- [ ] `darkroom/src/gui/pane/graph/gesture/new_node/tests.rs:69-76` — the same doc paragraph appears twice on `assert_fits`.

## Loose tolerances where the exact answer is known
Deterministic, often noiseless fixtures are asserted with bands that have no stated reason. A regression of the size of the band passes. In some cases the band hides a production bug.

### scenarium, common
Paths without a crate prefix are relative to `scenarium/src/`.

- [ ] There are 49 membership checks of the form `run.ran().contains(&"x")` / `run.cached().contains(&"x")`, in `engine/tests/cache_persistence/{blob_recovery,cache_modes,frontier}.rs`, `const_bindings.rs`, `node_seeds.rs`, `topology.rs`, `resource_binds.rs` and `events.rs`. Each run's full set is knowable, and an extra node recomputing goes unnoticed. Assert exact `ran()`/`cached()` lists, as `frontier.rs:193-197` already does. `blob_recovery.rs:218` (`calls.count() > after_run1`) should be `== 2`.
- [ ] Error kinds are asserted by Display substring: `engine/tests/error_propagation.rs:19,26` (`.contains("upstream")`) and `execution/executor/tests.rs:165-166`. Match `RunError::Invoke { .. }` / `RunError::SkippedUpstream { .. }` as `executor/tests.rs:493-504` does. `executor/tests.rs:564-566` uses `is_some()` where the variants are known.

## Risky behavior with no test
Branches with real failure modes that no test reaches.

### scenarium, common
Paths without a crate prefix are relative to `scenarium/src/`.


### darkroom, lens, imaginarium, quickbench, root `test_resources/`
Paths are relative to the repository root.

- [ ] `lens/src/image/codec/tests.rs:26-50` — the round trip covers only `RGB_U8`. The risky part of the codec is the 3-byte format tag (`codec/mod.rs:43-45`, `:64-72`), and a swapped `channel_size`/`channel_type` byte for u16 or f32 would go unnoticed. Sweep the round trip over `ALL_FORMATS`. `:53-107` also only asserts `is_err()`: match the message so each case proves which guard rejected it (short, unknown format, length mismatch, overflow).
- [ ] `lens/src/image/codec/tests.rs:109-115` — `register_image_type_wires_the_codec` asserts only that a `HashMap` insert happened. Assert the entry's codec, for example `version() == 2` and a decode through `library.types[&id]`.
- [ ] `quickbench/src/lib.rs:289-306` (`compute_stats`) has no test. That leaves the empty-times path (`n = max(1)`) and the median definition untested; for an even count it takes the upper middle, `times[len/2]`. `bench_persists_results_when_output_dir_set` (`tests.rs:67-84`) never runs twice, so the `vs_previous` line is never produced. `compare_to_previous` (`tests.rs:52-65`) asserts only the signs of `pct`, never the values (+2.0 %, −20.0 %, +20.0 %), and does not test the ±5 % edges. Add a table with hand-computed means and medians and an exact percentage table.
- [ ] `quickbench/quickbench-macros/src/lib.rs` — no tests at all. The argument parser, the error for an unknown key, and the rule that iterations-only disables the time limits (`:174-184`) are all untested. Add a `tests/macro.rs` integration test that uses `#[quick_bench(ignore = false, warmup_iters = 1, iters = 3)]` and asserts the iteration count.
- [ ] `darkroom/src/core/edit/action_stack/tests.rs:194-233` — `history_bounded_by_byte_budget` asserts `entries.len() < 200` and `actions.len() <= 2 × budget`. The selection-toggle entry has a fixed byte size, so the number of retained entries can be computed by hand: `floor(256 / entry_bytes)`. Assert that exact count, and the exact compaction point.
- [ ] `darkroom/src/gui/graph_ctx/tests.rs:98-101` — `known_node.inputs().len() > 0`. `Add` declares exactly 2 inputs, so assert `== 2`.

## Second sources of truth: tests and harness re-implement production or each other
A copy of a formula, a constant or a pipeline step changes with the code it copies, or drifts away from it. Either way it cannot catch the bug it exists for.

### scenarium, common
Paths without a crate prefix are relative to `scenarium/src/`.

- [ ] `scenarium/src/testing/engine.rs:619-642` (`NameMap::of`): run outcomes are named from `graph.node.name`. `Compiled` (`testing/graph/compiled.rs:34`) is named from `TestGraph.ids`. These diverge for every `TestGraph::instance` node (`testing/graph/mod.rs:94`): `place` (`:325`) builds `Node::from(&func)`, so the node's name is the *func's* name. In `topology.rs:52` (`print_b` instance of `print_a`) and `cache_persistence/frontier.rs:117` (`print_direct`), `RunOutcome::ran()/cached()/status("print_b")` would report `"print_a"` twice. This is not caught today only because those tests assert on logs. Derive `NameMap` from `TestGraph.ids`, the same map `Compiled` inverts, and have `instance` set `node.name = name`.
- [ ] Three definitions of "ran". `Executor::ran` (`execution/executor/mod.rs:749`, internals) counts `Ran | Failed` and feeds `RunOutcome::snapshot` via `ran_in_schedule_order` (`testing/engine.rs:389-396`). `RunOutcome::published` (`:456-460`) and `ExecutionOutcome::ran` (`execution/report.rs` internals, used by `testing/program/runs.rs:213`) count only `Executed`. `RunOutcome::ran()`'s own doc (`testing/engine.rs:464`) says "invoked their lambda and succeeded", which is wrong for the engine path. Its field doc (`:373`) says "invoked their lambda". Pick one meaning, put it in one place, and make both `RunOutcome` constructors use it.
- [ ] The stub lambda `async_lambda!(|_| { Ok(()) })` is written five times: `testing/mod.rs:30` (`with_stub_lambda`), `testing/graph/mod.rs:363` (`NodeSpec::new`), `testing/program/node_builder.rs:81` (`stub`), `execution/executor/tests.rs:203,243`, `graph/func/mod.rs:535`. Route all of them through `with_stub_lambda`, or a `FuncLambda::stub()` in `graph::func::lambda::internals`.
- [ ] There are two spellings of "a test failure error". `graph/func/lambda.rs:98-119` defines a `TestInvokeError` wrapper behind `internals::failure`. `testing/graph/mod.rs:344` (`failing_lambda`) uses `InvokeError::external(std::io::Error::other(..))`. Delete `TestInvokeError`, and implement `failure` as the `io::Error::other` form so `failing_lambda` can call it.
- [ ] Priming a cache hit is spelled three times: `testing/program/runs.rs:122-142` (`Runs::cached`), `testing/program/sweep.rs:86-91` (`Sweep::run`), and `RuntimeCache::hydrate` (`execution/cache/runtime/mod.rs` internals). The first two each re-do `stamp_digests` + `expect(current_digest)` + `load_output(.., Some(digest))`. Make it one `RuntimeCache` internals method that both call.
- [ ] `scenarium/src/testing/program/runs.rs:97-100,237-240`: `Runs::demand` and `remaining_reads` build `OutputAddr { node_idx, port_idx }` by hand. `Placed::addr(port)` (`testing/program/mod.rs:59`) exists for exactly this.
- [ ] `scenarium/src/testing/program/runs.rs:46-50`: `Runs::new` re-implements `ProgramBuilder::planned()` (`testing/program/mod.rs:150`) with a different initial state. Give `planned` the state parameter (or expose `staged` + roots) so the "every node a plain root" convention lives once.
- [ ] `scenarium/src/worker/tests/cache.rs:36,161`: the blob path is re-derived as `dir.join(id.as_uuid().simple().to_string())`, a second copy of `DiskStore::node_path`'s naming (`disk_store/mod.rs:93`). Use `DiskStore::blob_path` (`disk_store/mod.rs:259` internals), as `TestEngine::blob_path` does.
- [ ] `common/src/file_utils/tests.rs:31-48` and `scenarium/src/execution/cache/disk_store/tests.rs:58-68` both re-implement the publication temp-file naming (`"<name>."` prefix + `.tmp`). Add one `common::file_utils::internals::publication_temp_files(path)` next to the code that names the temps, and use it from both.
- [ ] There are three near-identical `Blob(Vec<u8>)` + byte-codec fixtures: `execution/cache/disk_store/tests.rs:70-152` (`VersionedCodec`), `execution/cache/disk_store/format/tests.rs:97-182` (`BlobCodec`), and `execution/engine/tests/cache_persistence/cache_modes.rs:363-415` (`BlobCodec`). Unit `CustomValue` boilerplate repeats in `worker/tests/cache.rs:221-238,311-334`, `execution/cache/runtime/tests.rs:501-529`, `execution/engine/tests/mid_run_release.rs:46-62` and `data/dynamic_value.rs:193-215`. One `testing` fixture (a `Blob` value plus a codec with version, decode-counter and fail/under-read knobs, and a `ram_bytes` override) would replace about 200 lines.
- [ ] `scenarium/src/graph/tests.rs:24-31` (`output_type`) and `scenarium/src/execution/engine/tests/compile_regressions.rs:39-46` (`authoring_output_type`) are the same helper. The passthrough spec `input(Any).wildcard(0)` is restated at `graph/tests.rs:35`, `compile_regressions.rs:51`, `execution/compile/tests.rs:460,561` and `graph/output_types/tests.rs:122`. Add `TestGraph::output_type(name, port)` and `NodeSpec::passthrough()`.
- [ ] `scenarium/src/execution/engine/tests/compile_regressions.rs:25,90,109` read `program.outputs[program.by_id(e.id(name)).outputs][0]` by hand. `Compiled::output_types(name)` (`testing/graph/compiled.rs:96`) is the accessor for this.
- [ ] `scenarium/src/testing/worker.rs:148` and `scenarium/src/testing/engine.rs:167` are identical `event(name, idx)` builders. `testing/worker.rs:125` (`TestWorker::compile`) duplicates `TestGraph::compile` (`testing/graph/mod.rs:554+`). `worker/tests/live_progress.rs:108` spawns a whole `Worker` (`TestWorker::over(graph).compile()`) just to compile. Put `event` and `compile` on `TestGraph` only.
- [ ] `scenarium/src/testing/engine.rs:191-217`: `run_sinks_reporting` copies `try_run_cancellable` (`:269-284`) with a different reporter. Make one private `execute_with(reporter, seeds, cancel)`. The three "filter rows, then names, then sort" bodies (`holding_ram` `:490`, `with_status` `:538`, `where_state` `:605`) can likewise share one helper.
- [ ] `scenarium/src/testing/graph/mod.rs:481-500`: `NodeSpec::counted` repeats `returns`'s type inference + `pure().output()`. Have one call the other with the body as the only difference.
- [ ] `scenarium/src/execution/engine/tests/output_demand.rs:45,57`: a hand-rolled `Arc<tokio::Mutex<i64>>` call counter. `Calls` exists to replace exactly this, and its doc (`testing/calls.rs:10-13`) names the pattern. This is also the only reason `engine/tests/mod.rs:19` imports `tokio::sync::Mutex`.
- [ ] There are two hand-built-program builders. `CompiledGraphBuilder` (`execution/compile/mod.rs:405-440`, exported at `lib.rs:29-30`) sorts ids. `ProgramBuilder` (`testing/program/mod.rs`) asserts ascending ids. `installation.rs:7-13`'s `program()` helper is exactly `CompiledGraphBuilder` re-done with `ProgramBuilder`. Keep one builder, and expose it to downstream from `scenarium::testing`.
- [ ] `scenarium/src/execution/engine/mod.rs:378-385`: `get_argument_values(&NodeId)` only forwards to `get_argument_values_at(NodeId)`, which only forwards to `argument_values_at`. Collapse the chain to one method taking `NodeId` by value.
- [ ] `scenarium/src/execution/cache/runtime/tests.rs:21-23`: `complete_snapshot` is an identity wrapper over `OutputSnapshot::new`. Also `:260-264`, `:300-304`, `:351-355` re-spell `resident_slot(Some(d), Some(d), out())` inline.
- [ ] `scenarium/src/execution/executor/tests.rs:20` and `scenarium/src/execution/schedule/tests.rs:440` define the same `value(i64) -> DynamicValue` helper. Move it next to `ProgramBuilder`.
- [ ] `scenarium/src/execution/schedule/tests.rs:221-225,264-268,282-287`: the `Planner::default()` + `RunSchedule::default()` + `plan(..)` boilerplate is repeated where `ProgramBuilder::plan/try_plan` already exist. `dependency_cycle_is_rejected` is `prog.try_plan(&RunSeeds::sinks())`. The sites that deliberately reuse one planner across calls are fine.

### darkroom, lens, imaginarium, quickbench, root `test_resources/`
Paths are relative to the repository root.

- [ ] `darkroom/src/gui/widgets/format/tests.rs:3-6`, `:28-29` — the "≤ 7 chars" bound is hand-linked by a comment to `RUN_TIME_MIN_WIDTH = 52.0` px (`gui/pane/graph/node/header.rs:57`). If either side changes, the test stays green. Measure `"999.99s"` with the header's text style and assert it fits `RUN_TIME_MIN_WIDTH`.
- [ ] `darkroom/src/gui/state/process_memory.rs:98-106` — the 1000 ms schedule is retyped instead of derived from `SAMPLE_INTERVAL` (`:18`). Compute the table from the constant.
- [ ] `darkroom/src/gui/pane/graph/gesture/new_node/tests.rs:55-61` — the popup cap formula from `new_node/mod.rs:116-120` is re-implemented in the test, as `clamp` against `min().max()`. Expose the production computation, for example `fn popup_cap(theme, surface_h)`, and call it.
- [ ] `darkroom/src/core/edit/step/tests.rs:266-271` — the `5e-5` and `2e-4` probes retype `VIEWPORT_EPS` (`step/set_viewport.rs:14`). Derive them as `EPS / 2` and `EPS * 2`.
- [ ] `darkroom/src/gui/pane/graph/gesture/pan_zoom/tests.rs:59-66`, `:94-98` — the expected value is `SCROLL_ZOOM_BASE.powf(18.0)`, the production formula, under a 1e-6 tolerance; the comment's hand value (≈1.04604) is never asserted. `:169-172` and `:187-190` compare a clamped zoom with a `1e-5` tolerance, but `clamp` returns the bound exactly, so use `assert_eq!`. Their comment also claims that pivot invariance holds at the clamp, and nothing asserts it. The tolerances at `:122`, `:199`, `:210`, `:213` give no reason.
- [ ] `darkroom/src/gui/pane/graph/gesture/breaker/tests.rs:116-126` — the comment says the point lands "at exactly (2000, 0)", while the assert allows 1e-4. Either make it exact or state why it cannot be: `3000·(2000/3000)` in f32 is not exact.
- [ ] `darkroom/src/gui/theme/mod.rs:259-279` (`theme_roundtrips_through_ron`) compares fields one by one because `Theme` and `palantir::Theme` lack `PartialEq`. A field added later goes unchecked.

## Duplicate tests and fixtures that should be one table or one helper
The same fixture or property is written many times, with different ad-hoc tolerances. Each copy drifts, and one fix has to be made in many places.

### scenarium, common
Paths without a crate prefix are relative to `scenarium/src/`.

- [ ] `scenarium/src/graph/tests.rs:334-358` (`node_remove_test`): it returns `TestResult` without any `?`, and the disable-everything loop (`:341-344`) has nothing to do with removal.
- [ ] `scenarium/src/execution/engine/tests/node_seeds.rs:88` duplicates `:94`: the `output_i64("mult") == None` assertion is already covered by `outputs("mult").is_empty()`.
- [ ] `scenarium/src/library/tests.rs:254` (`invoke_by_id_and_index`): there is no "index" in the test, and the `by_name` → `.id` → `by_id` round trip (`:276-277`) tests lookup only incidentally.

### darkroom, lens, imaginarium, quickbench, root `test_resources/`
Paths are relative to the repository root.

- [ ] The preset and variant-name pins are written two or three times each. `["wide_field", …]` appears at `lens/src/astro/config/preset.rs:148-156` and `lens/src/astro/nodes/tests.rs:213-221`. `["auto_asinh","auto_stf"]` appears at `preset.rs:187`, `astro/nodes/tests.rs:275`, and `astro/config/processing.rs` (`StretchMethodChoice::variants`). `["subtract","divide"]` appears at `preset.rs:194`, `astro/nodes/tests.rs:439`, and `processing.rs` (`BackgroundMode::variants`). The `ExtractBackground` field order appears at `processing.rs` (`builder_ports_follow_the_lumos_field_order`) and `astro/nodes/tests.rs:405-416`. `auto_stretch_node_is_registered` (`astro/nodes/tests.rs:257-282`) repeats `preset_nodes_use_value_variant_picks_with_build_overrides` (`:350-396`). Pin each fact once.
- [ ] `lens/src/astro/config/preset.rs:178-210` — `let _cfg = RegistrationPreset::Default.config();`, `let _cfg: Stretch = …`, and `let _scnr: Scnr = …` only check that the call runs. Assert one field each, as `:206` does for `BackgroundMode::Divide`.
- [ ] `darkroom/src/gui/state/preview_store/tests.rs:10-15` (`image_value`) vs `preview_store/mod.rs:308-312` (`internals::opaque_image_value`) — two image-value builders in one module. Keep `image_value(w, h, fmt)` in `internals` and define the opaque 2×1 case through it.
- [ ] `darkroom/src/core/document/harness.rs:105-115` (`DocFixture::stubs`) duplicates `stub_at` (`:133`). Its only callers are `pan_zoom/tests.rs:17`, which could use `stub_at` and its returned ids, and `:41`, where `stubs([])` is just `DocFixture::default()`.
- [ ] `darkroom/src/core/edit/graph_intent/tests.rs:32-43` (`func_node`, `add_node`) vs `darkroom/src/gui/app/session/mod.rs:344-356` (`func_node`, `add`) — both build `AddNode`, and in different ways. `graph_intent/tests.rs:156-161` and `session/mod.rs:366-371` then write the literal by hand anyway. One `GraphIntent` add builder beside `DocFixture` would serve both files.
- [ ] `darkroom/src/core/edit/action_stack/tests.rs:34-42` — `History::node(i)` indexes `graph.iter()` and calls that insertion order. `DocFixture::node(i)` already indexes paint order for the same fixture. Keep the `DocFixture` and use its indexing.
- [ ] `darkroom/src/gui/pane/graph/gesture/new_node/tests.rs:31-32`, `:35-41`, `:111-119`, `:151-156` — the 60-func and 12-func stub libraries are built separately; one helper `bulk_library(n)` would do. The restyle is written twice: as `restyled` (`:111-114`) and again inside the closure (`:115-119`). Define it once as a `fn(&mut Theme)` and apply it to both.
- [ ] `darkroom/src/gui/pane/graph/gesture/pan_zoom/tests.rs:45-99`, `:101-151` — the four `scroll_to_zoom_factor` tests and the two pivot-invariance tests should each be one table.
- [ ] `darkroom/src/gui/pane/graph/gesture/breaker/tests.rs:132-173` — the three `intersects_cubic_*` cases should be one table.

## Stale, wrong and change-narrating comments
Comments that describe code that no longer exists, contradict the asserted values, or narrate history.

### scenarium, common
Paths without a crate prefix are relative to `scenarium/src/`.

- [ ] Comments that narrate change: `scenarium/src/worker/event_loop/tests.rs:168-172` ("Stale-event filtering is now structural…", also `//` rather than `///` on a test doc), and `scenarium/src/worker/batch/tests.rs:160-162` ("implicit today (Option::replace)").

### darkroom, lens, imaginarium, quickbench, root `test_resources/`
Paths are relative to the repository root.

- [ ] Change narration in test comments: `darkroom/src/gui/graph_ctx/tests.rs:9` ("no longer asks"), `darkroom/src/core/document/mod.rs:475-477` ("they were four separate spellings … before"), `darkroom/src/core/io/document/tests.rs:71-72` ("Before this, save only asserted"), `darkroom/src/gui/app/session/mod.rs:263-264`, `:439-441` ("The single-slot arbitration this replaced"), `darkroom/src/gui/app/session/harness.rs:168`, `darkroom/src/gui/pane/graph/gesture/breaker/tests.rs:80-85`, `darkroom/src/gui/pane/graph/gesture/new_node/tests.rs:16-19`, `:140-143`, `darkroom/src/gui/pane/graph/gesture/selection/tests.rs:14`, `darkroom/src/gui/pane/graph/node/port_color/tests.rs:47`, `darkroom/src/gui/pane/graph/paint/inspector.rs:448-449`, `darkroom/src/gui/requests.rs:134`, `darkroom/src/gui/pane/viewer/mod.rs:651-652`, `darkroom/src/gui/state/preview_store/tests.rs:21-23`, `darkroom/src/gui/app/discard_dialog.rs:163-166`, `lens/src/image/codec/tests.rs:20-21` ("no longer reads"), `lens/src/astro/config/processing.rs` (`builder_ports_follow…`: "live in another crate now").
- [ ] Stale or wrong descriptions:
  - `lens/src/astro/nodes/tests.rs:1` says "Registration tests", but the file also covers the domain boundary, the frame-set key and invocation.
  - `darkroom/src/core/edit/graph_intent/tests.rs:211` says "The clone of `b`", but the helper also finds `a`'s clone.
  - `darkroom/src/gui/pane/graph/gesture/selection/tests.rs:21-23` says the row assigns "by map iteration order", but `DocFixture::add` places by slot count, deterministically.
