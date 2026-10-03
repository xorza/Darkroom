# Test and harness review

> Whoever addresses an item deletes it. When a section is empty, delete its heading too.

Review of all test code and test harnesses in `common`, `scenarium`, `lumos`, `lens`, `darkroom`, `fits-well`, `imaginarium` and `quickbench`. Palantir is not in the scope.

Groups are sorted by severity. Each group has one subsection for each scope, and each subsection gives the base of its short paths.

The production bugs that these tests hid are in `.notes/ISSUES.md`. The items here cover only the test side.

No test is slow. Each suite runs in less than 3.5 s, and the slowest single test takes 0.77 s (`registration::tests::transform_types::a_rung_that_fit_survives_a_later_rung_failing`). In scenarium, approximately 85% of the serial suite time is wall-clock waiting (see Nondeterministic tests).

One decision applies to all of lumos, and several items below depend on it:

- Real-data tests are gated on the `real-data` feature and also on `#[ignore]`. Thus `--features real-data` alone runs none of them. Some dataset readers have no gate at all.

Two items need a manifest change, which needs your approval: `common` with `internals` in the `lens` dev-dependencies, and tokio `test-util` in the `lens` dev-dependencies.

## Tests that cannot fail, or do not run the code their name claims
The fixture, the tolerance or the call path makes the assertion true whatever production does. These tests report coverage that does not exist, so they come first.

### lumos — combine, drizzle
Paths are relative to `lumos/src/stacking/`, except those that start with `lumos/` or `src/` (`lumos/src/`).

- [ ] `combine/tests/mod.rs:162`, `:236` — the `("gesd", StackConfig::gesd())` rows stack 14 frames, but `StackConfig::gesd()` sets `small_n: SmallN::median_below(15)` (`combine/config/mod.rs:243`) and `run_stacking` resolves it against the frame count (`combine/stack/mod.rs:335`). Both "GESD" rows therefore run a median. That makes them duplicates of the `median` row, and GESD is never exercised end-to-end. Use ≥15 frames or `small_n: SmallN::none()`, and assert the resolved method.
- [ ] `combine/stack/tests.rs:2177` `noise_weighting_with_rejection` — three frames with the default `small_n` (`median_below(5)`) downgrade to a median. The median of (99.9, 90, 999) is 99.9, which passes the `±10.0` check. Neither sigma clipping nor noise weights run. Use `SmallN::none()` and assert the exact weighted survivor mean.
- [ ] `combine/rejection/tests.rs:933` `winsorized_correction_factor_applied` — the 0.5 tolerance is larger than the effect of the factor. With the 1.134 correction removed, `robust_estimate` returns 2.98 against the asserted 3.43, a difference of 0.45, so the test still passes. The expected value is also built about centre 5.5, but the estimator's centre is `working[len/2] = 6.0` (`winsorized_clip_config.rs:83`). Assert the exact value: σ about 6 = √(85/9) × 1.134 = 3.48499, with no clamp on the first pass.
- [ ] `combine/rejection/tests.rs:910` `winsorized_robust_estimate_uses_stddev_not_mad` — it asserts only `0 < σ < 2` and centre ±0.2. A MAD-based σ (≈0.74) passes as well, so the property in the name is never checked. Assert the exact stddev-based value.
- [ ] `combine/tests/mem_budget.rs:39` `load_budget_is_respected_across_configs` — the test passes its own `transient = 2 * frame` (`:44`) into `load_concurrency`. The loader's call sites (`combine/cache/loader/mod.rs:183-185`, `:275-277`) are never involved, so the "divisor reverts to the resident frame size" regression named in the doc comment cannot be detected. The invariant `resident + c·transient ≤ usable || c == 1` is also true by construction of `load_concurrency` (`src/memory.rs:99-111`), and `src/memory.rs:323` already pins the same function with exact cases. Either test the loader's chosen concurrency, or delete this test.
- [ ] `combine/stack/tests.rs:1316` `registered_global_normalization_uses_paired_signal_samples` — the second frame is the first one shifted by a constant (bilinear on a linear ramp). The MAD ratio of a constant shift is exactly 1, so a plain MAD-ratio gain passes the `±1e-3` check too. The test cannot tell the paired fit from what it replaced. Use frames that differ in scale and in noise.
- [ ] `combine/normalization/tests.rs:73` `paired_gain_recovers_scale_after_residual_clipping` — on noiseless collinear data Deming returns the true slope for any noise ratio, so the `1.0, 4.0` noise variances have no effect. This is the only caller of `paired_photometric_gain`, so `deming_gain`'s noise-ratio term and its fallbacks (`photometric_gain.rs:172-194`) are untested. Add a noisy case with a hand-computed Deming slope for two noise ratios A→X and B→Y, and assert X ≠ Y.
- [ ] `combine/rejection/tests.rs:1328` `no_outliers_possible_clean_data_skips_quickselect` — uniform data returns 100 survivors with or without the early exit, so the "skip" in the name is unobservable. It also duplicates `no_outliers_possible_tight_cluster` (`:1235`).
- [ ] `combine/rejection/tests.rs:1038` `linear_fit_sigma_is_mean_abs_dev` — it asserts only that a perfect line loses nothing. The mean-abs-dev σ is never checked, and the test is a near-copy of `linear_fit_preserves_trend` (`:1050`).
- [ ] `combine/rejection/tests.rs:782` `linear_fit_tighter_than_sigma_clip` — `lf_remaining <= sc_remaining` passes when the two are equal, so "tighter" is never shown. Assert the exact survivor counts for both methods.
- [ ] `combine/stack/tests.rs:2144` `noise_weighting_equal_noise_gives_equal_weights` — it asserts `None` (the zero-MAD fallback), not equal weights. The name describes a different behavior, and `:2156` is the real equal-weights test.

### scenarium, common
Paths without a crate prefix are relative to `scenarium/src/`.

- [ ] `scenarium/src/library/tests.rs:67-81` (`add_rejects_invalid_function_declarations`): none of the three funcs has a lambda. `Library::add` panics on a missing implementation (`graph/func/mod.rs` validate, "has no implementation"), so the "nil id" and "bad wildcard" cases would panic even if those checks were removed. Give the first two cases `with_stub_lambda`, and assert the panic message per case, the way `graph/func/mod.rs:436-476` already does for `validate()`. Or reduce this test to one case proving `add` calls `validate`.
- [ ] `scenarium/src/execution/engine/tests/compile_regressions.rs:166-204` (`update_with_a_grown_output_list_retires_the_shorter_snapshot`): the test has no assertion. It only fails through `validate_debug`'s debug-only invariant panic. The doc (lines 163-165) says release builds carried the bad snapshot, and a release run of this test passes vacuously. Assert the outcome: after the second run `e.outputs("generate").len() == 2`, `e.output_i64("generate", 1) == Some(2)`, and `run.ran()` contains `"generate"`.
- [ ] `scenarium/src/execution/engine/tests/compile_regressions.rs:121`: `n.pure().output(DataType::Int).returns(1i64)` declares **two** outputs, because `returns` (`testing/graph/mod.rs:481`) already adds `pure()` and an output. The fixture's `generate` has an unwritten `out1`, which is not the "same declaration gains an input" shape the test describes. Use `n.returns(1i64)`.
- [ ] `scenarium/src/worker/batch/tests.rs:158-173` (`batch_intent_update_overwrites_earlier_update_in_same_batch`): both updates are `empty_compiled()`, and the assertion is `Replace(_)`. The test passes whether the first or the last update wins. Keep the two `Arc`s and `Arc::ptr_eq` the second. Then fold it into the `batch_intent_last_write_wins_per_slot` table below it, whose Clear/Update rows have the same blind spot.
- [ ] `scenarium/src/worker/tests/runs.rs:103-110` (`sync_fires_after_execution`): `w.run()` waits up to 5s for the completion, so the test passes even if `Sync` acked *before* the run executed. To test the ordering, assert after `settle` returns that the completion is already queued, without waiting: `w.drain()` contains a `Completed` status.
- [ ] `scenarium/src/testing/graph/tests.rs:28-33`: the comment "`returns` declared the output its literal implies" has no matching assertion. The lines only check `contains` and the node count. Assert `g.compile().output_types("src") == [DataType::Int]`.
- [ ] `common/src/file_format.rs:113-116` (`test_all_formats_for_testing_count`): `[Self; 3].len() == 3` is a type-level tautology. The real risk is that `all_formats_for_testing` (`file_format.rs:49`) silently misses a new variant. Make it exhaustive by construction, e.g. with a `match` that fails to compile on a new variant, and delete this test.
- [ ] `common/src/introspect/tests.rs:50` and `:448-453`: `assert_ne!(Speed::TYPE_ID, Mode::TYPE_ID)` and `assert_ne!(Mode::TYPE_ID, other::Mode::TYPE_ID)` compare hand-typed uuid literals, so they cannot fail. The derived `Speed::TYPE_ID` is never pinned to its attribute value. Assert `Speed::TYPE_ID == "3effbd19-d4a8-4a9b-a931-78fd0e4f8adb"`. Delete the `other::Mode` test: both enums are hand-written, so it tests nothing.
- [ ] `scenarium/src/execution/executor/tests.rs:332` (`assert_ne!(a, b)`) and `scenarium/src/graph/tests.rs:62-64` (`assert_ne!(CacheMode::Ram, CacheMode::Disk)`): distinct by construction, so they cannot fail. Delete them.

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

### lumos — combine, drizzle
Paths are relative to `lumos/src/stacking/`, except those that start with `lumos/` or `src/` (`lumos/src/`).

- [ ] `combine/stack/tests.rs:1820-1861` — gain ±0.1 and offset ±1.0 where the exact values are 1 and −100, and gain 0.5 ± 0.15 (30%) where MAD ratio = 0.5. Also `:1864-1931`, `:1934-1958`, `:1961-1988` (±1.0/±2.0 on uniform frames where MAD = 0 gives gain 1 and offset exactly ref−frame), `:2084-2107` (±2.0), `:2110-2141` (`> 10×` where the exact 1/σ² weights follow from the asserted MADs), and `:1628` (`dark < 0.05` where the mean is exactly 0.2/5 = 0.04).
- [ ] `combine/stack/tests.rs:2031-2081` `norm_uses_lowest_noise_reference` — the non-reference frames are checked with a negative "not identity" OR-condition. The expected gain is MAD₁/MAD₀ and the offset follows from it. This also duplicates `combine/normalization/tests.rs:27`.
- [ ] `combine/rejection/tests.rs:100-123`, `:126-144`, `:237-262`, `:497-603`, `:732-762`, `:1063-1075` — `mean < 10.0`, `remaining < 8`, `>= 9`, `< 2.5`, `< 1.1`, `±0.25`. Every fixture is ≤10 hand-written values, so the survivor set and the mean are exact. `:378` checks percentile with `r >= 1`. `:420-439` states the survivors are indices 3, 2, 4 but asserts only that 0 and 1 are excluded.
- [ ] `combine/tests/mod.rs:100` (20% on √N, with no stated reason; the RMS estimate over 16384 px has ~0.6% sampling error), `:134-139`, `:171`, `:215` (`ratio > 1.5` where inverse-variance weighting of 6+6 frames predicts ≈ the noise ratio), `:243` (`1.5×` the mean RMS).
- [ ] Throughout scope — 231 hand-rolled `assert!((a - b).abs() < tol, ...)` sites and zero uses of `assert_close!`/`assert_close_slice!`, which are in `crate::testing::prelude` and glob-imported by both modules. Exact-integer cases (for example `cache/tests.rs:455-461`, `:787-836`, `stack/tests.rs:498`, `:1093`, `:1536`, `:1565`) should be `assert_eq!`.

### scenarium, common
Paths without a crate prefix are relative to `scenarium/src/`.

- [ ] There are 49 membership checks of the form `run.ran().contains(&"x")` / `run.cached().contains(&"x")`, in `engine/tests/cache_persistence/{blob_recovery,cache_modes,frontier}.rs`, `const_bindings.rs`, `node_seeds.rs`, `topology.rs`, `resource_binds.rs` and `events.rs`. Each run's full set is knowable, and an extra node recomputing goes unnoticed. Assert exact `ran()`/`cached()` lists, as `frontier.rs:193-197` already does. `blob_recovery.rs:218` (`calls.count() > after_run1`) should be `== 2`.
- [ ] Error kinds are asserted by Display substring: `engine/tests/error_propagation.rs:19,26` (`.contains("upstream")`) and `execution/executor/tests.rs:165-166`. Match `RunError::Invoke { .. }` / `RunError::SkippedUpstream { .. }` as `executor/tests.rs:493-504` does. `executor/tests.rs:564-566` uses `is_some()` where the variants are known.
- [ ] `scenarium/src/execution/cache/disk_store/format/tests.rs:448-460`: seven distinct corruptions are each checked only with `.is_err()`. A corruption rejected for the wrong reason, such as a short read, passes. Table-drive each corruption with its expected error. Line `:476` (`assert_eq!(DESCRIPTOR_LEN, 32)`) is a stray check at the end of an unrelated test.
- [ ] `common/src/serde.rs:286-306`: the trailing-data branch accepts "both Ok" or "both Err" per format. Pin which one each format does.
- [ ] `scenarium/src/execution/engine/tests/argument_values.rs:30,34,41` and `topology.rs:21`: `approximately_eq(2.0)` on values that are exactly `2 as f64`. Use `==`, since an unexplained tolerance hides nothing here.
- [ ] `scenarium/src/worker/batch/tests.rs:59-69`: `len()` + `contains` where `assert_eq!(.., [event])` / `[node_id]` is exact. Same at `engine/tests/events.rs:140` (`triggered_events.len() == 1`, where `[tick]` is known).

## Risky behavior with no test
Branches with real failure modes that no test reaches.

### lumos — combine, drizzle
Paths are relative to `lumos/src/stacking/`, except those that start with `lumos/` or `src/` (`lumos/src/`).

- [ ] `combine/normalization/mod.rs:380-402` — `stratified_valid_indices` subsampling above `PHOTOMETRIC_SAMPLE_LIMIT` (65 536) never runs. The largest Global-normalization fixture is 64×48 (`stack/tests.rs:1317`). `source_noise_variance` with real confidence (`:419-446`) is also never exercised, because every fixture uses `from_coverage`, which sets confidence to 1.
- [ ] `combine/stack/quantization.rs:28-38`, `:61-92`, `:95-130` — no test covers `SourceSigmas::measure` rejecting non-finite or zero σ, `combined_median` with norms (the conservative branch), the `gain.abs()` in `conservative`, or `MaxSigma` (the bit-ordering `fetch_max`, and `get()` returning `None` on 0 bits).
- [ ] `stack_product/mod.rs:73-87` — the single-channel `assert_eq!` in `into_cfa_master` has no `#[should_panic]` test. `stack_product/` has no tests of its own; its conversions are covered only in `lumos/tests/public_api.rs:384-441`.

### scenarium, common
Paths without a crate prefix are relative to `scenarium/src/`.

- [ ] `scenarium/src/elements/math_library.rs:371-393`: Square Root, Sine, Cosine, Tangent, Arcsine, Arccosine and Arctangent are never checked for computing the right op. "Sine" wired to `f64::cos` passes, because Sine appears only in the text-rejection case. One table of `(name, input, expected)` covers all of them.
- [ ] `common/common-derive/src/lib.rs`: no tests at all. The `type_id` attribute errors (missing, non-uuid, non-canonical, duplicate, `lib.rs:416-450`) are untested. Unit-test `enum_type_id` with `syn::parse_quote!` inside the proc-macro crate, which needs no new dependency.
- [ ] `scenarium/src/execution/engine/tests/stats.rs:5-27`: the doc says "a dangling subscription and pin wire nothing", but only the missing port is asserted. Assert `compiled.subscribers(..)` is empty. The file name `stats.rs` also does not match its contents.
- [ ] `common`'s `id_type!` macro (`common/src/macros.rs`) has no test in `common`. Its only coverage is `scenarium/src/graph/tests.rs:424-432` in another crate.

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

### lumos — combine, drizzle
Paths are relative to `lumos/src/stacking/`, except those that start with `lumos/` or `src/` (`lumos/src/`).

- [ ] In-memory `FrameCache` builders: `make_test_cache` (`combine/cache/tests.rs:23`, via `from_stack_frames`), `FrameCache::from_images` (`combine/cache/mod.rs:554`, a hand-built `CacheCore` that skips validation), `make_cfa_cache` (`combine/cache/tests.rs:390`, which hand-builds `CfaImage` instead of using `crate::testing::make_cfa`), `make_cfa_stack_cache` (`combine/stack/tests.rs:43`), and three `FrameCache { core: CacheCore { … } }` literals (`combine/cache/tests.rs:561`, `:628`, `:689`). Keep one builder in `cache/mod.rs`'s existing `internals` (plus a spilled variant). `FrameCacheParams` is also built by identical closures at `:101` and `:164`.
- [ ] `combine/tests/mem_budget.rs:44` re-types `DECODE_TRANSIENT_FACTOR` as `2 * frame` (`src/memory.rs:53`; `decode_transient_bytes` exists). `combine/tests/mem_budget_probe.rs:302` re-types the 75% budget rule (`src/memory.rs:14`) instead of calling `memory_budget`/`fits_in_memory`.

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
- [ ] `scenarium/src/testing/worker.rs:197-208` and `:256-269`: `report()` and `drain()` duplicate the installed/cleared tracking `match`. Extract one `observe(&WorkerReport)`.
- [ ] `scenarium/src/testing/engine.rs:191-217`: `run_sinks_reporting` copies `try_run_cancellable` (`:269-284`) with a different reporter. Make one private `execute_with(reporter, seeds, cancel)`. The three "filter rows, then names, then sort" bodies (`holding_ram` `:490`, `with_status` `:538`, `where_state` `:605`) can likewise share one helper.
- [ ] `scenarium/src/testing/graph/mod.rs:481-500`: `NodeSpec::counted` repeats `returns`'s type inference + `pure().output()`. Have one call the other with the body as the only difference.
- [ ] `scenarium/src/execution/engine/tests/output_demand.rs:45,57`: a hand-rolled `Arc<tokio::Mutex<i64>>` call counter. `Calls` exists to replace exactly this, and its doc (`testing/calls.rs:10-13`) names the pattern. This is also the only reason `engine/tests/mod.rs:19` imports `tokio::sync::Mutex`.
- [ ] There are two hand-built-program builders. `CompiledGraphBuilder` (`execution/compile/mod.rs:405-440`, exported at `lib.rs:29-30`) sorts ids. `ProgramBuilder` (`testing/program/mod.rs`) asserts ascending ids. `installation.rs:7-13`'s `program()` helper is exactly `CompiledGraphBuilder` re-done with `ProgramBuilder`. Keep one builder, and expose it to downstream from `scenarium::testing`.
- [ ] `scenarium/src/execution/engine/mod.rs:378-385`: `get_argument_values(&NodeId)` only forwards to `get_argument_values_at(NodeId)`, which only forwards to `argument_values_at`. Collapse the chain to one method taking `NodeId` by value.
- [ ] Serde format lists are re-typed at `common/src/serde.rs:276` (`[Ron, Bitcode, Lz4]`), `scenarium/src/graph/tests.rs:81` (`[Ron, Bitcode]`) and `scenarium/src/graph/serde/tests.rs:38`, while `graph/tests.rs:611` uses `all_formats_for_testing()`. Use the one list, and state why a sweep excludes a format when it does.
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

### lumos — combine, drizzle
Paths are relative to `lumos/src/stacking/`, except those that start with `lumos/` or `src/` (`lumos/src/`).

- [ ] `combine/rejection/tests.rs` — the fixture `[1.0, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0, 100.0]` appears in 8 tests (`:101`, `:117`, `:148`, `:194`, `:238`, `:387`, `:498`, `:546`). Exact pairs: `:188` ≡ `:386` (sigma-clip survivor indices), `:257` ≡ `:403` (linear-fit outlier), `:109` ≡ `:442`. `:1189-1232` is four `reset_indices` tests and `:1235-1307` is six `no_outliers_possible` tests, each a natural table. `:18`, `:40`, `:459`, `:722` assert constructors and defaults through `abs() < EPSILON` on literals, although the configs derive `PartialEq`; one `assert_eq!` table would do.
- [ ] `combine/config/tests.rs:32-246` — seven preset tests each assert one or two fields through `matches!` with `abs() < f32::EPSILON` (sigma_low only, never sigma_high or iterations). Make one table comparing `method`/`weighting`/`normalization`/`small_n` with `==`. `:66` `struct_update_syntax` tests the language. `combine/stack/tests.rs:2272` `light_preset_uses_noise_weighting` repeats `config/tests.rs:227`.
- [ ] `combine/stack/tests.rs:1063` ≡ `:1635` — the same 10/20 frames with coverage [1,1]/[1,0]: one asserts the image, the other the planes. Merge them, and fold in `:1510` and `:1542`.
- [ ] `combine/stack/tests.rs:1864`, `:1920`, `:1961` plus `:1880` — four tests over `make_uniform_frames(16, &[100, 200])`.
- [ ] `combine/normalization/tests.rs:129-170` and `:205-241` build the identical 3-frame × 3-channel fixture twice; extract one builder. `:28-60` repeats the 4-line `FrameStats { …, quantization_sigma: None, domain: None, row_order: None }` literal that `frame_stats()` (`:17`) exists to hide.
- [ ] `combine/cache/tests.rs:407-486` vs `:489-522` — the calibration test repeats the median-of-[1,3,2] and 17.5 weighted-mean cases with a different cache builder; make one sweep over both builders.
- [ ] `combine/stack/tests.rs` — `stack_images(frames, config, ProgressCallback::default(), CancelToken::never())` is spelled out 40 times; a two-argument local helper removes ~250 lines.

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

## Placement, gating and bench layout
Test, internals and bench code sits where the rules say it must not, or is gated so that it never runs.

### lumos — combine, drizzle
Paths are relative to `lumos/src/stacking/`, except those that start with `lumos/` or `src/` (`lumos/src/`).

- [ ] `combine/stack/tests.rs:1807-2107` (`norm_*`, `global_norm_*`, `multiplicative_*`, `normalized_stacking_rgb`, `dispatch_*`) tests `normalization::compute_frame_norms`, so normalization coverage is split across two files. Move it to `combine/normalization/tests.rs`.

## Stale, wrong and change-narrating comments
Comments that describe code that no longer exists, contradict the asserted values, or narrate history.

### lumos — combine, drizzle
Paths are relative to `lumos/src/stacking/`, except those that start with `lumos/` or `src/` (`lumos/src/`).

- [ ] Comments that narrate a change: `combine/rejection/tests.rs:16-17` ("These were six one-assertion tests"), `:191-193`, `:1020`, `:1052`, `:882-884`; `combine/cache/tests.rs:37-40`; `combine/error.rs:221-223`; `combine/stack/tests.rs:1574`.
- [ ] Empty or narrating comments: `// Cleanup` with nothing after it at `combine/cache/loader/tests.rs:77`, `:203`, `:265`, `:377`, `:417`, `:460`; `combine/cache/tests.rs:408`, `:434`.

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
