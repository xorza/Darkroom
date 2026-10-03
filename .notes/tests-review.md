# Test and harness review

> Whoever addresses an item deletes it. When a section is empty, delete its heading too.

Review of all test code and test harnesses in `common`, `scenarium`, `lumos`, `lens`, `darkroom`, `fits-well`, `imaginarium` and `quickbench`. Palantir is not in the scope.

Groups are sorted by severity. Each group has one subsection for each scope, and each subsection gives the base of its short paths.

The production bugs that these tests hid are in `.notes/ISSUES.md`. The items here cover only the test side.

No test is slow. Each suite runs in less than 3.5 s, and the slowest single test takes 0.77 s (`registration::tests::transform_types::a_rung_that_fit_survives_a_later_rung_failing`). In scenarium, approximately 85% of the serial suite time is wall-clock waiting (see Nondeterministic tests).

## Risky behavior with no test
Branches with real failure modes that no test reaches.

### darkroom, lens, imaginarium, quickbench, root `test_resources/`
Paths are relative to the repository root.

- [ ] `quickbench/src/lib.rs:289-306` (`compute_stats`) has no test. That leaves the empty-times path (`n = max(1)`) and the median definition untested; for an even count it takes the upper middle, `times[len/2]`. `bench_persists_results_when_output_dir_set` (`tests.rs:67-84`) never runs twice, so the `vs_previous` line is never produced. `compare_to_previous` (`tests.rs:52-65`) asserts only the signs of `pct`, never the values (+2.0 %, −20.0 %, +20.0 %), and does not test the ±5 % edges. Add a table with hand-computed means and medians and an exact percentage table.
- [ ] `quickbench/quickbench-macros/src/lib.rs` — no tests at all. The argument parser, the error for an unknown key, and the rule that iterations-only disables the time limits (`:174-184`) are all untested. Add a `tests/macro.rs` integration test that uses `#[quick_bench(ignore = false, warmup_iters = 1, iters = 3)]` and asserts the iteration count.
