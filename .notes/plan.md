# Lumos open work — plan

Scope: the five open issues in `ISSUES.md` that touch `lumos` and `lens`, and what was left of
the stacking review (`lumos-stacking-review.md`, dropped here; its last text is in git at
`881ac82a5:.notes/lumos-stacking-review.md`). Paths are relative to `lumos/src/` unless stated
otherwise. Line numbers drift, so search for the named item.

## Summary

| ID | Item | Recommendation | Batch |
|---|---|---|---|
| — | First float-TIFF light decoded twice | Do, low priority — blocked on Q1 | B9 |
| — | X-Trans cosmic-ray statistic | Defer | — |
| RAW-10, DMS-6 | RAW buffer vs file open; WB inside the demosaic | Blocked on real data | — |

## How to run this plan

- **One batch per change.** Start a batch only when the tree is clean (the previous batch is
  committed). The user commits; do not commit.
- **Order.** Follow the dependency graph. Inside a batch, follow its steps in order.
- **Verification after each batch**, from `AGENTS.md`, for each crate the batch names:
  `cargo fmt -p <crate> && cargo clippy -p <crate> --all-targets --all-features -- -D warnings &&
  RUSTDOCFLAGS="-D warnings" cargo doc -p <crate> --no-deps --document-private-items
  --all-features && cargo test -p <crate> --tests --all-features`, with `--features ml` in place
  of `--all-features` in `cargo test` for `lumos`. A batch that changes a public `lumos` API also
  runs `lens` and `darkroom`. A change inside a submodule (`imaginarium`) runs its own chain from
  its own directory and stays valid in a standalone checkout.
- **SIMD kernels**: after a change to one,
  `cargo rustc -p lumos --release --lib -- --emit=asm && grep -E "call.*(Avx2|core_arch)"
  $(ls -t ../target/release/deps/lumos-*.s | head -1) | grep -vc 5enter` must print 0.
- **Snapshots.** `internals/characterization` pins outputs. A batch that moves an output on purpose
  updates the pin and states why. An unexplained move is a regression.
- **Measure-first items** adopt a change only when the bench shows the stated gain and the output
  is bit identical.
- **New dependencies**: none. `libc` is already a Linux dev-dependency of `lumos` (Q6).
- **Other platforms.** Only Linux builds here. Code for macOS and Windows is reported as not
  compiled unless the user runs it on the macOS test host or gives a Windows check.

## Dependency graph

```
B9                          independent
```

---

## B9. A float TIFF's dimensions from its header (low priority) — blocked on Q1

**Evidence.** `pipeline/light_source.rs`, `LightFiles::shape`: `LinearImage::peek` reads no TIFF
header, so the first float-TIFF light is decoded in full to size the run, before the plan exists,
and decoded again by the intake.

**Solution.** `LinearImage::peek` for TIFF through a header-only probe in `imaginarium`, a
matching submodule change. Where the probe cannot answer, keep today's decode. Blocked on
[Q1](plan_QUESTIONS.md#q1-where-a-float-tiffs-dimensions-are-read-from-its-header-b9): the
submodule is its own repository.

**Test.** Peek gives the decode's dimensions on mono and RGB float TIFFs; the pipeline decodes
the first TIFF light once (decode counter).

**Crates:** imaginarium (own chain), lumos.

---

## Deferred

- **X-Trans cosmic-ray statistic** (research). A statistic that is zero on a linear slope,
  negative on a star's convex wing and positive at a peak, for example the residual of a robust
  local plane through the same-colour neighbours; `objlim` calibrated against mono L.A.Cosmic on
  Gaussian stars (σ 0.8–4 px, nine sub-pixel positions, every phase), with no star pixel flagged
  at the default and the mono path's false-positive rate on pure noise. No reference tool exists to
  check against, and X-Trans lights are refused cleanly now, so it waits for a real X-Trans set.
- **RAW-10**: `bench_unpack_file_vs_buffer` (`bench` + `real-data`) on CR2, CR3, NEF and RAF.
  Needs `test_data/lumos_data`.
- **DMS-6**: per-colour background gains used only inside the demosaic for OSC FITS without white
  balance; compare star colour and zipper artifacts with and without, adopt only if it helps. Needs
  real OSC data.

## Checks left for the user

- `DECODE_PINS.raw_cfa` is a placeholder: run `raw_decode_snapshot` with `real-data` and pin the
  digest it prints.
- The flagged fraction per colour on a cooled-CMOS master and on a DSLR set; if it is large on a
  well-stacked cooled master, the threshold multiplier is the lever.
- The dark-scale factor `k` across a DSLR session, and the residual hot-pixel pattern with and
  without it.
- `common/src/file_utils/platform/windows.rs` is checked only by a Windows clippy: run the `common`
  tests on Windows, `FileIdentity::of` above all.
- `lumos/src/platform/macos.rs` is not compiled here and `platform/windows.rs` only through a
  Windows clippy: build and test lumos on macOS and Windows, the free space and the Job-object
  memory above all.
