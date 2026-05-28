AI coding rules for Rust projects.

## Project state

Scenarium is a Cargo workspace for a node-based data processing pipeline framework with a visual editor. Workspace members: `common` (shared utilities, buffers, file formats), `scenarium` (the node-graph framework: graph, execution, functions), `darkroom` (the editor app, the default-member), `lens` (image-processing funclib), `lumos` (astronomical image processing), `imaginarium` (image library with wgpu GPU ops), `quickbench` (in-test benchmarking utilities), and `palantir` (our GUI library). `default-members = ["darkroom"]`; only `.tmp` is `exclude`d. `imaginarium`, `quickbench`, and `palantir` are git submodules (see `.gitmodules`). The frozen `darkroom-egui-deprecared/` crate is **not** a workspace member.

**GUI rewrite in progress.** The editor is mid-migration off egui:

- **`darkroom-egui-deprecared/`** — the old egui-based editor. Frozen, kept only as a reference for porting features. Do not add new functionality here. Bug-fix only if something blocks the rewrite. The directory name is intentionally misspelled (`-deprecared`); keep using it verbatim.
- **`darkroom/`** — the new editor, being rewritten on top of **Palantir** (the in-development immediate-mode GUI library in `palantir/`). This is where new editor work goes.
- **`palantir/`** — our own Rust GUI library, also under active development. A workspace member (and git submodule) with its own conventions in `palantir/CLAUDE.md` and `palantir/DESIGN.md`. Treat it as a sibling project: changes to `darkroom/` may require coordinated changes in `palantir/`.

Both `darkroom/` and `palantir/` are pre-1.0 and break freely. The egui UI conventions section below applies **only** to `darkroom-egui-deprecared/`.

## Workflow

- **Never commit or push without explicit user confirmation.** This rule is non-negotiable and overrides auto mode, "just do it" instructions, or any implied approval from earlier in the conversation. The trigger must be a fresh, unambiguous command like "commit", "commit push", "ship it". "Do the refactor" / "apply F3" / "go" authorize the code change, not the commit. Finish the change, run tests/clippy/fmt, then stop and wait for the user to inspect the diff and explicitly say to commit.
- **Use `.tmp/` for source investigation of external dependencies.** Cloning a dep into `.tmp/<crate>` (gitignored) lets Read/Grep work without per-file approval prompts on cargo registry paths. Match the version to the one resolved in `Cargo.lock` (e.g. `git clone --depth 1 --branch 0.34.1 https://github.com/emilk/egui .tmp/egui`). Leave clones in place across sessions — `.tmp/` is gitignored and persists.

## Available CLI tools

These are installed and available — prefer them over slower/regex-based equivalents:

- **`ast-grep`** — AST-aware structural code search. Use for "find all call sites of X", "find all `Arc::get_mut().unwrap()` patterns", or any search where regex is fragile. Beats `rg` for code-pattern matching.
  - `ast-grep run --pattern 'Arc::get_mut($X).unwrap()' --lang rust`
- **`scc`** — fast LOC counter with language stats. Use for design-review scope decisions (faster + smarter than `wc -l`).
  - `scc darkroom-egui/src/gui/`
- **`hyperfine`** — statistical benchmarking. Use when validating performance claims rather than guessing.
  - `hyperfine 'cargo test --release my_test'`
- **`cargo-machete`** — finds unused crate dependencies.
  - `cargo machete`
- **`bacon`** — Rust-aware continuous build/test loop. Useful when iterating on a refactor.
  - `bacon` (in repo root)

Plus the standard set: `rg`, `fd`, `jq`, `gh`, `cargo`, `cargo-nextest`, `rustfmt`, `clippy`, `sqlite3`.

## Error Handling

- Use `Result<>` only for expected failures (network, I/O, external services, user input).
- Avoid `Option<>` and `Result<>` for cases that cannot fail.
- For required values, use `.unwrap()`. For non-obvious cases, use `.expect("clear message")`.
- Crash on logic errors. Do not silently swallow them.
- Add asserts for function inputs and outputs to catch logic errors. Do not assert on user input or network failures.

See `CODING_STYLE.md` for Rust code-style rules (comments, visibility, accessors, tests layout, mechanical refactoring).

## Verification

- After changing code, run before confirming:
  ```
  cargo nextest run && cargo fmt && cargo check && cargo clippy --all-targets -- -D warnings
  ```
  Skip doc-tests.
- Check test run times are reasonable. Research and fix slow tests.
- Check online documentation for best practices and patterns.

## Testing

- Write tests for ALL new and modified non-GUI code. No exceptions.
- Tests must verify **correctness**, not just "it runs without panicking":
  - Use hand-computed expected values. Show the math in comments.
  - Assert exact outputs (survivor counts, indices, computed values), not vague ranges.
  - Verify edge cases: empty input, minimal input, boundary conditions.
- For algorithms with parameters, test that parameters actually change behavior:
  - Test with parameter A: expect result X. Test with parameter B: expect result Y. Assert X != Y.
- For SIMD implementations, test against scalar reference for identical results.
- For rejection/filtering: verify exactly which elements survive and which are rejected.
- For numerical code: validate against known-good reference values or analytical solutions.
- Do NOT write tests that only check `result < 10` or `remaining > 0`. These catch nothing.

## UI conventions (egui, darkroom-egui-deprecared crate only)

These rules apply to the frozen `darkroom-egui-deprecared/` crate. The new `darkroom/` uses Palantir — see `palantir/CLAUDE.md` for its widget-id and scoping rules.


- **Every widget id must come from `StableId`** (`darkroom-egui/src/common/id_salt.rs`).
  Sanctioned constructors:
  - `StableId::new(name)` — `#[track_caller]` mixes `file!()`/`line!()` into
    the hash. Use a tuple for per-instance widgets:
    `StableId::new(("cache_btn", node.id))`.
  - `StableId::from_id(id)` — wrap an existing `egui::Id` (e.g. inherited
    from a caller). No rehash.
- **Never call `UiBuilder::new()` directly.** Use `gui.scope(id).show(|gui| ...)`
  (optionally chain `.max_rect(r)` / `.sense(s)` before `.show`).
  It applies `UiBuilder::id(id.id())` (`global_scope=true`) so the scope's
  registered widget id equals the salt verbatim — bypassing egui's
  `unique_id = stable_id.with(parent_counter)` formula
  (`egui-0.34.1/src/ui.rs:297`) that drifts whenever conditional siblings
  appear/disappear in the parent and trips the
  "widget rect changed id between passes" warning.
- **Never call bare `ui.allocate_rect`/`allocate_exact_size`/`allocate_space`
  on a `Gui<'_>`'s parent ui.** Wrap in a `gui.scope(StableId::new(..)).show(...)`
  first so the auto-id starts from a stable seed.
- **Don't bake transient runtime keys (e.g. `selected_node_id`) into a
  fixed-rect widget's salt.** The widget id changes on every selection
  while its rect stays constant → "rect changed id" warning. Use a stable
  string salt; let the *content* change instead.
- **Whitelisting**: if you genuinely need raw `UiBuilder::new(` (e.g.
  inside a function that takes raw `egui::Ui`, not our `Gui`), put
  `// id-drift-ok` on the same line OR up to two lines above. The
  tripwire test `no_bare_ui_builder_in_crate` enforces all of this.

## Documentation

- Read `NOTES-AI.md` files for summarized project knowledge. Check current directory and relevant subdirectories.
- `NOTES-AI.md` files are AI-generated notes on implementation details and structure. Place in any directory where context is needed. Store only current state, not change history. Split files >300 lines into subdirectory files with a brief parent overview.
- Avoid editing root `README.md` unless asked; update `NOTES-AI.md` instead.
- Add `README.md` to folders that benefit from human-readable docs (crates, examples, benchmarks, complex modules).

## Optimization Workflow

- Before optimizing, always run or create a relevant benchmark and save the baseline results.
- After optimizing, run the same benchmark again and compare against the baseline to verify the optimization actually improved performance.
- If the optimization is a regression or no improvement, revert it.

## Benchmarks and Profiling

- Run benchmarks: `cargo test -p <crate> --release <bench_name> -- --ignored --nocapture`
- Save benchmark results to a txt file in the bench directory. Maintain a `bench-analysis.md` with interpretations. Update on re-runs.
- Add readme files to benchmark folders explaining which optimizations were tried.
- Use nextest for running tests and measuring execution time.
- Perf profiling: use 3000 samples per second.
- If `addr2line` errors appear in `perf report`/`perf script`, use `perf script --no-inline`.
