# Open issues: proposal and plan

Scope: what is left of five `.notes/ISSUES.md` entries: the key API, the crossing rule, the doc links and the CI core dumps landed. Two are flaky CI failures with no cause in hand
yet; for those the plan gets the evidence first and names the fix each finding points to. Items
marked **[public]** change palantir's exported surface and wait for a go-ahead (palantir
AGENTS.md "Public API"). Items marked **[drive]** change the `cargo-drive` tool, a repository of
its own.

# Part 1: Proposal

## A crash with no evidence: the darkroom lib tests on lavapipe

- **What is known.** Run 40434805558 ended `cargo test -p darkroom --lib` with SIGSEGV 2 s into
  267 tests; the rerun passed, and it never crashed locally. libtest runs every test in one
  process, so a crash takes the binary down with no test name and no backtrace.
- **The likely area, not yet the cause.** Two driver tests (`driver/tests.rs:469`, `:601`) each
  open their own `RequestedGpu::headless`, a wgpu instance and device apiece, on threads that run
  at once. palantir's own tests never do that: `headless_test_gpu` shares one device per process
  and takes an interprocess lock "so no other test binary uses the GPU concurrently". Crashes that
  depend on thread count are a known pattern in llvmpipe-family drivers
  ([mesa #6917](https://gitlab.freedesktop.org/mesa/mesa/-/issues/6917),
  [wgpu #7655](https://github.com/gfx-rs/wgpu/issues/7655)), but nothing ties this crash to it.
- **Target: a crash leaves a backtrace.** The Linux job writes core dumps to a known directory
  (`ulimit -c unlimited` in the step that tests, `kernel.core_pattern` set with `sysctl`, as
  [HAProxy's CI](https://git01.mediatek.com/plugins/gitiles/haproxy/+/6435801d09cac089e705baed57f839495f61a972%5E%21)
  does), and a step that runs only on failure prints `thread apply all bt` from gdb for each core
  and uploads the cores. The next crash then names its thread and frame. Refused: retrying the
  test binary, which hides the crash; serializing the tests or `LP_NUM_THREADS=1` before a
  backtrace shows they matter, which is a guess at a fix.

## A peak 144 B past its figure: fits-well on four workers on macOS

- **What is known.** `run_of_views_on_four_workers_peaks_within_its_view_bytes` held 60218 B
  against a figure of 60074 B on macos-26, once; Linux never fails. The counting allocator counts
  every thread but the harness's, rayon's workers included, so it also counts what the pool
  allocates for itself.
- **The likely cause.** On macOS std boxes its pthread mutexes and condvars and allocates them
  on first use, where Linux uses futexes and allocates nothing
  ([rust #93740](https://github.com/rust-lang/rust/issues/93740)). Each rayon worker's sleep state
  is a `Mutex<bool>` and a `Condvar` (`rayon-core` `sleep/mod.rs`), so a worker that first blocks
  during the measured pass adds about a hundred bytes that no view holds. Work stealing decides
  which workers block when, which fits a failure that comes and goes. Against it: the warm pass
  leaves idle gaps between views, in which every worker should block once already. Only a run on
  a Mac can tell, which is why the trace comes first.
- **Target: confirm, then warm what the pool makes once.** A test-only trace in the counting
  allocator records, without allocating, the size and the thread of each allocation the measured
  pass makes on a worker, and the failure message prints them. If they are the sleep primitives,
  the warm-up blocks every worker once before the measured pass, as it already warms the pool's
  threads and the dither table. Refused: a tolerance for the pool, which would also pass a real
  overrun of that size.

## Docs no gate checks

- **Cause.** The root drive table has no `docs` lane, so nothing builds the workspace's docs:
  palantir and fits-well check theirs, the root never has. The 17 errors that stood, and
  `simd`'s missing CPU flag for rustdoc, are fixed; nothing keeps them fixed.
- **Target: the root gates its docs, private items included.** `.cargo/config.toml` gives
  rustdoc the same CPU under `target.'cfg(target_arch = "x86_64")'.rustdocflags`. The root table
  sets `docs = true` with `doc = "--all-features --document-private-items"`. drive's doc lane
  sets `RUSTDOCFLAGS="-D warnings"`, which replaces every configured rustdoc flag; it passes
  `--config "target.'cfg(all())'.rustdocflags=['-D','warnings']"` instead, which cargo joins
  with the project's own `cfg` flags (checked: `simd` then documents and fails only on its link
  errors). An ambiguous `write` takes rustdoc's `fn@` disambiguator
  ([rustdoc book](https://doc.rust-lang.org/rustdoc/write-documentation/linking-to-items-by-name.html#namespaces-and-disambiguators)).

# Part 2: Plan

## Batch 1: the root's docs lane

Files: the root `Cargo.toml` drive table; drive's doc step.

- [ ] **[drive]** The doc lane joins `-D warnings` through a `cfg(all())` rustdocflags
  `--config`, not `RUSTDOCFLAGS`; a test that a project's own `target.<cfg>.rustdocflags`
  survives. Bump `DRIVE_REV` and the drive table's `version`.
- [ ] Root drive table: `docs = true`, `doc = "--all-features --document-private-items"`.
  `cargo drive gate` runs the doc and doctest lanes clean.

## Batch 2: evidence for the two flaky failures

Files: `fits-well/tests/support/counting.rs`, `fits-well/tests/support/peak.rs`.

- [ ] fits-well: a fixed-size, allocation-free trace of the measured pass's worker allocations
  (size, thread), printed by the failing assert.
- [ ] Each entry in `ISSUES.md` keeps its line until a backtrace or a trace names the cause; the
  fix is its own item then. For fits-well the expected one: block every worker once in the
  warm-up, then delete the entry.

Sources: [nextest: why process per test](https://nexte.st/docs/design/why-process-per-test/)
(a crash in a shared-process run takes every test down), and the links in Part 1.
