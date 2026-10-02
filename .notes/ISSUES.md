# Issues

- `fits-well/src/ascii/mod.rs:349-356` — the ASCII-table float parser applies the exponent with `value *= 10f64.powi(e)` (and the implied decimals with `/ 10f64.powi(d)`), so it rounds twice. The cell `-3.0-1` in an `E8.1` column reads back as `-0.30000000000000004`, not the `-0.3` that cfitsio and astropy return.
- `darkroom/src/gui/widgets/format/mod.rs` — `Elapsed` renders `0.99996` s as `"1000.0ms"` (8 characters, above the 7-character width the node header allows). `Bytes` renders `1_048_575` as `"1024.0 KB"`, and the same carry occurs at the MB→GB step.
- `lumos/src/math/statistics/mod.rs` — `ClippedStats::sigma_clipped` returns the upper-middle median (`median_fast`) when clipping converges, but the averaged median (`median_mut`) on the `TooFew` and zero-iteration exits. `[1,2,3,4]` gives 3.0 with κ = 3 and 3 iterations, and 2.5 with 0 iterations.
- `lumos/src/stacking/registration/recovery.rs:94-96` — `recover_matches` stops when a pass leaves the match count unchanged, also when the pass replaced some matches with others. The returned transform is then not fitted to the returned matches.
- `quickbench/src/lib.rs:10-11` — the crate doc tells users to disable the lock with `Bencher::without_lock`, which exists only inside `#[cfg(test)] mod internals`.
- `fits-well/examples/inspect.rs:16`, `fits-well/examples/wcs.rs:16` — the examples default to files under `tests/data/fits/`, which `Cargo.toml` excludes from the package, so both fail from the published crate.
- `lumos/src/testing/real_data/milky_way.rs` `milky_way_best_pipeline` — on the stack the current real-data pipeline produces, the enhanced image's intensity minimum is `-0.50153935`, just past the test's `min > -0.5` display bound.
- `lumos/src/stacking/pipeline/tests/mem_budget_probe.rs` `pipeline_stack_budget_probe` — peak heap is 3573 MB against the 2048 MB budget across four stages; the probe reports that one stage's memory is not freed before the next.

