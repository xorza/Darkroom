# Structural redesign plan

Sources: `.notes/ISSUES.md`, `.notes/workspace-review.md` (WR), `.notes/tests-review.md` (TR).
Out of scope: `palantir`, and the findings in `lumos/.notes/*.md` (this plan only avoids conflicts with them).

---

## 0. Verification of the findings

The submodule update changed no reviewed code. `fits-well`, `imaginarium` and `quickbench` have no code commits after 2026-09-24, and the reviews are from 2026-10-02. `palantir` moved, but it is out of scope. `quickbench` has only lockfile changes in the working tree.

| Check | Scope | Result |
|---|---|---|
| Every cited path exists | 841 items, both reviews | All resolve. 22 short or generated paths (`port_row.rs`, `stacked_light.tiff`, proposed files) resolve by suffix or are outputs. |
| Every cited symbol exists | same | All exist. The 39 misses are external names (wcslib `celset`, LibRaw `color3_image`, `ManuallyDrop`) or name fragments (`_50_percent`). |
| Cited line is near its named symbol | 86 `path:line (name)` pairs | 71 within 8 lines. |
| Cited line is inside its file | 2663 line numbers | `tests-review.md` has wrong line numbers in some items, for example `math/fwhm.rs:78` (test is at 34), `wavelet/tests.rs:208` (at 63), `fwhm/tests.rs:240-339` (at 65-143; the file has 218 lines and never had more than 234). The content of these items is correct. Use the symbol names, not the line numbers. |
| Content, by reading code | all 6 `ISSUES.md` bugs, all 20 High groups, about 60 Medium items, 20 Low items, 20 test items | All confirmed, with the notes below. |
| Content, by running probes | fits-well WCS (5 claims), compressed table round trip | All confirmed. |

Notes from the content check:

- **Compressed tables (High):** the read fails only when a tile compresses to fewer bytes than the `TDIMn` product. Constant data fails with `KeywordOutOfRange { name: "TDIMn" }`. Incompressible data reads back. The bug is real, but it depends on the data.
- **`CalibrationMasters::from_files`:** the two examples `lumos/examples/full_pipeline.rs` and `mem_probe.rs` also call it. The finding lists only tests and benches.
- **WCS pole (`CelestialPole::from_fiducial`):** the probe gives the same pole for `LATPOLE = 90` and `-90`, which agrees with the finding. The probe does not show the pole value itself.
- **Stretch solver numbers** (`solve_asinh_beta`, `StfCurve::new`): the code shape is confirmed. The quoted output values were not re-run.

---

## 1. Rules for every step

These rules apply to every step in every phase. They exist because the plan changes numeric results, file formats and public APIs in many crates, and a regression must be loud.

1. **Test first.** A step that fixes a finding starts with a test that fails on the current code and names the finding. The fix makes it pass. A probe from section 0 becomes such a test (`fits-well` WCS domain, compressed-table round trip with constant data).
2. **Characterization snapshots before rewrites.** Before phase 5, record bit-exact hashes of the outputs of the synthetic `lumos` pipeline per stage (decode, calibrate, detect, register, warp, combine, stretch), and of the `imaginarium` conversions over `ALL_FORMATS`. Each step lists the snapshots it is expected to change and why. A snapshot that changes without a listed reason stops the step.
3. **Every step leaves every chain green.** So any step boundary is a safe commit point. Commits happen only when you say so.
4. **Bookkeeping in the same step.** A step deletes the review items it closes (WR, TR, `ISSUES.md`) and deletes empty headings. At the end of each phase, a search for the closed symbols in the review files must find nothing.
5. **A persisted format that changes fails loudly.** Section 5 lists every persisted format the plan touches, with the version bump and the failure mode for an old file: a clear error or a cache miss, never data read with the wrong meaning.
6. **Never delete what the code did not create.** Any code that removes files or directories removes only items that carry its own marker (section 4, W2).
7. **A submodule API change updates its workspace callers in the same step.** The step runs the submodule's own chain from its directory and the workspace chain for every caller. A submodule change must stay valid in a standalone checkout.
8. **SIMD on both architectures.** A step that changes a SIMD kernel runs `cargo clippy --target aarch64-unknown-linux-gnu` for the NEON code, and runs the crate's tests on the macOS laptop (aarch64) through its tmux session. If that host is not available, the step report says that NEON was only compile-checked.
9. **Every new tolerance states its reason** in the code: what error it absorbs and why that size.

---

## 2. Phase 0 — build and lint baseline

### 2.1 One dev-profile rule for dependency optimization

Replace the 30 `[profile.dev.package.<name>]` blocks in the root `Cargo.toml` with:

```toml
[profile.dev.package."*"]
opt-level = 3

# The submodules you edit and debug most: unoptimized like the workspace members.
[profile.dev.package.palantir]
opt-level = 0
[profile.dev.package.imaginarium]
opt-level = 0
[profile.dev.package.quickbench]
opt-level = 0
```

Cargo applies `"*"` to every package that is not a workspace member. The excluded submodules are not members, so `"*"` covers them too, and the explicit entries above take precedence (per **D1**, this keeps today's behaviour; `fits-well` stays optimized). `"*"` does not apply to build scripts and proc-macros, which use `[profile.dev.build-override]`, so `syn` and the derive crates keep their fast build. This is the form the Cargo book documents and the form Bevy recommends.

Verification: `cargo build -v` shows `-C opt-level=3` for one dependency (for example `tiff`) and no `opt-level` flag for `palantir`. Record the clean-build time and the time to run the `lumos` test suite before and after.

### 2.2 Lint set

Measured with clippy 0.1.99 on all eight crates (`--all-targets --all-features`, lints passed on the command line, `clippy.toml` given through `CLIPPY_CONF_DIR`, no change to the repo). Counts are distinct sites.

**Level policy:** every lint is `warn`, not `deny`. The verification chain runs clippy with `-D warnings`, so a warning fails the chain, but a build in the middle of an edit still works. The current workspace `deny` entries change to `warn`. Groups (`rust_2018_idioms`, `clippy::pedantic`) take `priority = -1` so that single lints override them.

**Suppressions:** every suppression is `#[expect(lint, reason = "…")]`, never `#[allow]`. `clippy::allow_attributes` (20 sites) and `clippy::allow_attributes_without_reason` (31 sites) enforce this. An `expect` that no longer fires is itself a warning, so a stale suppression cannot stay.

**Submodules** have their own `[lints]` tables and must not inherit from the workspace. The same set goes into `imaginarium`, `fits-well` and `quickbench` by copy, with a `clippy.toml` in each.

| Lint | Hits | Verdict | Reason |
|---|---|---|---|
| `rust_2018_idioms` (group) | 45 | adopt | 44 `elided_lifetimes_in_paths`, 1 `unused_extern_crates`. Mechanical. |
| `missing_debug_implementations` | 0 | keep | Already on (`deny` → `warn`). Matches the `#[derive(Debug)]` rule. |
| `unreachable_pub` | 0 | keep | Already on. Matches the visibility rule. |
| `redundant_imports` | 195 | adopt | Mechanical (`cargo clippy --fix`). |
| `unused_qualifications` | 85 | adopt | Mostly `std::result::Result` spelled out beside an import. |
| `trivial_numeric_casts` | 6 | adopt | |
| `unused_lifetimes` | 0 | adopt | Guard. |
| `unused_macro_rules` | 2 | adopt | Both in `lumos`. Matches "remove unused code". |
| `unsafe_code` | 323 | per crate, not workspace-wide | SIMD needs `unsafe` (`lumos` 169, `imaginarium` 122, `fits-well` 23). `#![forbid(unsafe_code)]` in `common`, `scenarium`, `lens`, `quickbench` (0 hits; `-F unsafe_code` compiles `common` and `scenarium`, and `lens` has no derive that emits `unsafe`). `#![deny(unsafe_code)]` in `darkroom`, with `#[expect(unsafe_code, reason = …)]` on `alloc_audit` (9 sites, a `GlobalAlloc`). If a later derive emits `allow(unsafe_code)`, that crate moves from `forbid` to `deny`. |
| `clippy::pedantic` (group) | ≈ 4 300 | adopt, with the allow-list below | |
| `clippy::print_stdout` / `print_stderr` | 325 / 53 → 90 / 16 | adopt | With `allow-print-in-tests = true` in `clippy.toml`, what remains is in `examples/`, in `quickbench`'s report printer and in two real-data helpers. Those get a file-level `expect` with a reason. |
| `clippy::absolute_paths` | 766 | adopt | Matches the rules "no inline paths" and "free functions stay namespace-qualified" (`use std::fs; fs::create_dir_all`). The one exception in the rules (a gated inline statement in place of a cfg'd import) gets an `expect` with a reason. |
| `clippy::clone_on_ref_ptr` | 0 | keep | Already on. |
| `clippy::dbg_macro`, `clippy::todo` | 0 | adopt | Guards. |
| `clippy::let_underscore_must_use` | 36 | adopt | `let _ = fs::remove_file(..)` swallows an error. Deliberate best-effort cleanup in `Drop` gets an `expect` with a reason. |
| `clippy::unused_result_ok` | 11 | adopt | Same class (`.ok();` to discard). |
| `clippy::map_err_ignore` | 59 | adopt | `map_err(\|_\| ..)` drops the source. Keep the source, or `expect` with a reason where the source carries nothing (`TryFromIntError`). |
| `clippy::self_named_module_files` | 0 | adopt | Guard for the rule "never `foo.rs` beside `foo/`". |
| `clippy::allow_attributes`, `clippy::allow_attributes_without_reason` | 20, 31 | adopt | See "Suppressions" above. |
| `missing_errors_doc`, `missing_panics_doc`, `must_use_candidate`, `assert_is_empty` | — | allow | As in your list. |

Pedantic allow-list, each with a reason in the manifest comment:

| Allowed lint | Hits | Reason |
|---|---|---|
| `cast_precision_loss` | 1 038 | `usize → f32/f64` in numeric code. A hit says nothing unless the value passes 2²⁴, and the code cannot show that. |
| `cast_possible_truncation` | 785 | `f64 → f32` narrowing is the design of the f32 pipelines. |
| `similar_names`, `many_single_char_names` | 161 | Math code (`x`, `y`, `dx`, `dy`, `cx`). |
| `too_many_lines` | 59 | Size is a review question, not a lint. |
| `inline_always` | 30 | `#[inline(always)]` on `#[target_feature]` SIMD helpers is deliberate. |
| `float_cmp` | 42 | Exact comparison is deliberate (`SampleDomain::commensurate_with`, sentinels, tests that assert exact values). |
| `cast_ptr_alignment` | 61 | Every hit feeds an unaligned load (`_mm_loadu_*`). |

Kept from pedantic, with notes: `cast_lossless` (478, autofix to `f64::from`), `uninlined_format_args` (462, autofix), `doc_markdown` (296 → 241 with a first `doc-valid-idents` list; the list grows as the sweep finds names), `unreadable_literal` (280, autofix), `ignore_without_reason` (119, every `#[ignore]` states why), `return_self_not_must_use` (85, catches a dropped builder), `ptr_as_ptr` (70, `.cast()`), `wildcard_imports` (40 → 15 with `allowed-wildcard-imports` for `std::arch`), `manual_midpoint`, `stable_sort_primitive`, `trivially_copy_pass_by_ref`, `needless_pass_by_value` and the rest of the small ones.

Per **D2**, `cast_sign_loss` (308) and `cast_possible_wrap` (348) are on. Both flag `imaginarium/src/drawing.rs:49`, which is the `draw_circle` wrap bug in the workspace review. In SIMD index code with proven bounds, the `expect` sits on the kernel function and its reason names the bound; elsewhere the cast becomes `try_from` or a type change.

**Migration path.** Cargo does not allow a member to override single lints while it has `[lints] workspace = true`, so a workspace-level lint is on in every member at once. The migration therefore goes per crate through source attributes, which take precedence over the flags Cargo passes:

1. Add the full set to `[workspace.lints]` with every new lint at `allow`, plus `clippy.toml`.
2. Per crate, add `#![warn(<lint>)]` at the crate root for one lint group at a time, autofix first, then hand fixes, then the chain for that crate.
3. When every member is clean for a lint, set it to `warn` in `[workspace.lints]` and remove the crate attributes.

A lint whose hand fixes fall inside code that a later workstream rewrites stays at the crate-attribute stage for that crate, and the workstream lists it.

---

## 3. Decisions

All decisions were made on 2026-10-02.

| ID | Question | Decision | Effect on the plan |
|---|---|---|---|
| D1 | Which submodules stay at `opt-level = 0` in dev? | `palantir`, `imaginarium`, `quickbench` | Same behaviour as today (2.1). |
| D2 | `cast_sign_loss` and `cast_possible_wrap`? | Enable both | About 650 sites get `try_from`, a type change or a reasoned `expect` (2.2). |
| D3 | Thin-plate spline? | Keep as work in progress | `tps/` stays with its `cfg_attr(not(test), allow(dead_code))` and module note; that attribute becomes an `expect` with a reason. W3 does not touch it. The TR item that asks to remove it is closed as "kept deliberately". |
| D4 | `imaginarium` GPU in this workspace? | Drop the `wgpu` feature from the workspace dependency | The GPU code stays in `imaginarium` with its fixes (W11). `scenarium`'s `ContextType` and the `&mut ContextStore` parameters go (W9). |
| D5 | `scenarium` wildcard outputs? | Keep, make cheap | The feature stays. Darkroom recomputes `OutputTypes` only when the graph changes. `OutputTypes::update` reads the const kind without a `ConstValue` clone (W9, W13). |
| D6 | Drizzle with a SIP registration? | Support it | Newton inversion of the SIP polynomial per point, with a stated tolerance and a stated failure path (W3). |
| D7 | Palantir data types in `darkroom::core`? | Allow | The module doc names the allowed types. Core never imports `crate::gui` (W13). |
| D8 | Calibration-master construction? | lumos presets, lens per role | `MasterRole` owns the preset table. `from_files` and `RoleStack` go; the two examples call per role (W7, W10). |
| D9 | `lumos` public API with no non-test caller? | Keep public | `stack`, `stack_images`, `StackFrame`, `align_and_stack` and `DefectMap` stay `pub`. They get the same validation as the used paths (W1, W6, W7). |
| D10 | `Normalization::Global` estimator? | Paired photometric fit for every frame | W7 fixes the Deming inlier window first, then uses the paired fit over the common domain for all frames. |
| D11 | `lens` dev-dependencies `common/internals` and tokio `test-util`? | Approve both | W14 adds both to `lens/Cargo.toml` `[dev-dependencies]`. |
| D12 | `lens` processing-node port shape? | Preset picker plus optional `Config` | All six nodes take a preset and an optional `Config` port. The override is declared in the func, so darkroom shows it without running the graph (W9, W10). |
| D13 | Graph panes? | Single pane | Delete the per-pane narration and the unreachable branch in `GraphUI::appearing` (W13). |

---

## 4. Workstreams

Each workstream lists the review groups it closes, the target design and the tests that prove it. Its first step is always the failing tests (rule 1).

### W1 — lumos: one sample domain, one frame validation

Root cause: the meaning of a sample (its domain, row order, CFA pattern) is a fact that several types restate, that the writer does not save, and that each entry point checks in its own way.

Closes: WR "A reloaded calibration master always fails the sample-domain check", "Frame validation differs by entry point…", "Four parallel enums describe one sensor fact…", "RAW and FITS loading misclassify or reject valid files", the `CfaImage`/`XTransImage`/`BitPix` items in "Placeholder values…".

Target design:

- `SampleDomain` is a persisted fact. The FITS writer saves its scale as the real keyword `LUMSCALE`; `BUNIT` already carries the unit. When the reader finds `LUMSCALE`, it uses divisor 1 (the samples are already normalized) and restores `TransferProvenance::FitsNormalized { physical_scale: LUMSCALE }`. A master that was stacked from RAW darks reloads with its RAW scale. The keyword is the f32 scale written through f64, so the round trip is exact; a test checks it with `to_bits`.
- `CFA_FITS_VERSION` goes from 1 to 2, so a master written before this change fails to load with a version error, and `lens` rebuilds it (section 5).
- One `FrameSet` validator checks geometry, sample domain, row order, CFA pattern and sample finiteness, in one fixed order, in every `FrameCache` constructor, `from_tiered_paths` included. It runs incrementally: each frame is checked against the reference frame as it loads, so the first mismatch stops the run before the rest decode. Reused cache planes also pass `validate_frame_quality`. `CalibrationMasters::from_images` checks flat against flat-dark with the same validator.
- `CfaType` is the only sensor-pattern type. `SensorType` becomes `Option<CfaType>`. LibRaw's `filters == 0 && colors == 3` (linear DNG, sRAW, Foveon) goes to the LibRaw processed-image path for a `LinearImage` load, and fails a CFA load with "not a CFA frame". `DemosaicKind` and `DemosaicProvenance` become methods of `CfaType`. `CfaImage` holds a `CfaType`, not `Option`. `BitPix` goes; `ImageMetadata` stores `fits_well::SampleType` where the source had one.
- Optional FITS keywords degrade to `None` on a type mismatch. Only `cfa_type`, row order and `QNTZSIG` can fail a load. `BAYERPAT = 'TRUE'` fails unless `FitsLoadOptions` gives a pattern override.
- `fits-well` exposes the shape and stored `Bitpix` of any image HDU, so lumos deletes `compressed_shape` and its HDU-selection copies (W12 first, same step per rule 7).

Tests: a RAW-sourced master saved, reloaded and used to calibrate a RAW light; a version-1 master rejected with the version error; a `uint16` and a `float32` FITS of the same ADU rejected by every entry point, before the third frame decodes; a table over entry points × mismatch kinds.

### W2 — lumos: frame store, disk cache and memory plan

Root cause: the spill directory, the cache key, the sidecar names and the memory figure each have several owners.

Closes: WR "The frame spill directory deletes a directory the run did not create", "The disk frame cache keys, names and sidecars…", "Memory budgeting re-implemented at every entry point…", the null-mask half of "Frame validation…", "File-source identity is implemented three times…".

Target design:

- **Two spill modes, one safety invariant.** Cross-run reuse needs a stable directory, so one unique directory per run is not enough.
  - *Ephemeral* (`keep_cache = false`): `SpillDirectory` creates a new subdirectory `root/lumos-run-<pid>-<n>` with `create_dir` (not `_all`), writes a marker file into it, and removes it on drop.
  - *Persistent* (`keep_cache = true`): lumos writes into `root/lumos-cache/`, which carries the marker and the cache format version. Lumos never removes this directory; it replaces only entries whose key it owns.
  - Invariant: lumos removes a directory only when that directory carries the lumos marker. A user path given to `with_cache_dir` is never removed. A run killed before drop leaves an ephemeral directory; the next run removes such directories only when they carry the marker and their owning process no longer exists.
- `CacheKey { source: FileIdentity, decoder: DecoderKind, decode_version: u32 }` names every cached plane. `DECODE_VERSION` is a constant, and a golden test pins the hash of the decoded planes of the RAW and FITS fixtures: when decode output changes, that test fails and its message says to bump `DECODE_VERSION`. The bump is enforced, not a comment.
- `FrameSpill` owns every file name, including sidecars, and uses `FramePlane`'s `Display` for `coverage` and `confidence`. One `cache_frame()` path serves frame 0 and the rest, so frame 0 gets its sidecars and the source-change check.
- `StoredImage` spills its null mask as a bit plane and restores it, so the spill tier warps with `MaskedWarp` like the RAM tier.
- `FileIdentity { len, mtime_ns: i128 }` moves to `common::file_utils`; `lumos`, `scenarium` and `lens` use it. Its doc states the limit: a filesystem with coarse timestamps (FAT: 2 s) can miss an edit that keeps the length within one tick. `lens` computes `frame_set_key` only when its cache is on.
- `RunMemory { system: u64, user_override: Option<u64> }` is read once per run at the entry and passed down. `CacheConfig::available_memory` becomes `memory_override` and is never rewritten. `CacheCore` stores `chunk_memory: u64`, not a `OnceLock`. One tier rule (`MemoryPlan`) charges input frames and the resident output planes, for `load_tiered`, `frames_fit_in_memory` and the pipeline. W4 changes the detection working-plane count; `MemoryPlan` reads that constant, and the memory tests derive their ceilings from it (TR "Memory-model constants are re-typed in tests").

Tests: `with_cache_dir` pointed at a directory with a sentinel file, which survives the run and the drop; a persistent cache reused by a second run; a stale ephemeral directory with the marker removed, one without the marker kept; a RAW decoded as `LinearImage` then loaded as `CfaImage` with `keep_cache`, which must not reuse planes; a masked FITS light on the spill tier against the RAM tier, bit-exact.

### W3 — lumos: registration geometry

Root cause: `Transform` does not enforce its own normalization, the SIP model has no stated frame, and drizzle takes a bare transform with the opposite direction.

Closes: WR "SIP distortion is fitted in the target frame…", "Registration accepts a transform supported by 2–4 stars…", "`Transform` and drizzle disagree…", "Registration hot loops recompute work…", registration items in "Placeholder values…", "lumos stacking numerics…" and "The same constant or formula…"; `ISSUES.md` `recover_matches`.

Target design:

- `Transform::from_matrix` normalizes so that `m[8] = 1`. It rejects a matrix whose `|m[8]|` is too small relative to the other entries. The threshold is derived in that step from the f64 rounding that the division amplifies, and the derivation goes in the code (rule 9); the plan sets no number in advance. Non-homography types also require `m[6] = m[7] = 0`. Every constructor goes through it. The accessors (`rotation_angle`, `scale_factor`) then read a normalized matrix. The SIMD bilinear kernels keep `h·y + 1`, which is now correct by construction.
- Warp positions are computed in f64 and split into an integer part and an f32 fraction. Only the fraction is narrowed.
- SIP follows the FITS convention: the correction applies in the reference pixel frame, before the transform. SIP can follow any model, homography included, so the fit target uses the local Jacobian of the transform at each reference point: `J(r)⁻¹·(t − T(r))`. For an affine model `J` is the constant linear part. The fit is a first-order linearization; after it, the existing corrected-residual pass measures `T(r + c(r)) − t` in target pixels, and sigma clipping uses those residuals.
- `register` applies `min_matches` to the final inlier count and returns `RegistrationError::TooFewInliers` below it.
- RANSAC checks the adaptive bound on every iteration, not only on an improvement.
- `recover_matches` stops when the match *set* is unchanged, not the count, and always refits on the returned set.
- `DrizzleFrame` takes the registration's `WarpTransform` (reference → target) and maps input pixels with its inverse. Per **D6**, a SIP warp is inverted per point with Newton iteration: solve `r + c(r) = T⁻¹(t)` from the start value `T⁻¹(t)`, stop when the step is below a stated fraction of a pixel, and stop with a failure after a fixed iteration count. A point that does not converge contributes nothing, sets its coverage to 0, and is counted in the drizzle diagnostics; it is never used with an unconverged position. `quad_row_extent` uses the local Jacobian of the full transform.
- Non-linear warps evaluate the transform once per pixel into a scratch row buffer and share it between channels, quality maps and validity. SIP evaluation uses incremental powers. Homographies use row stepping (numerators and denominator are affine in x).
- Per **D3**, the thin-plate spline stays as it is.

Tests: SIP fit and warp under 10°, 180° and a homography, residual within the fit noise; a 3-star registration rejected; a homography stored with `m[8] = 2` against its normalized twin, bit-exact in SIMD and scalar; the Newton inverse against the forward map to the stated tolerance, and a forced non-convergence that leaves coverage 0; drizzle with a registration result, compared with the accumulator path bit for bit.

### W4 — lumos: star detection on a residual plane

Root cause: each stage decides for itself whether the sky is removed, and the configuration encodes modes as numeric sentinels.

Closes: WR "Star measurement rectifies sky noise into signal", "Deblending runs on sky-included pixel values", "FWHM auto-estimation measures on the minimum stamp", "The Gaussian profile fit is axis-aligned…", "Star-measurement fits and peak selection…", "Star-detection config holds variant fields…", "lumos star-detection deblender structure…", star-detection items in "SIMD backends…" and "Placeholder values…".

Target design:

- The detect stage computes the residual plane `pixels − background` once, in a pooled buffer. Threshold, labeling, both deblenders and measurement read the residual. No stage sees sky-included values. This adds one working plane per detector unless the matched-filter input buffer is reused for it; either way `DETECTION_WORKING_PLANES` states the new count and W2's `MemoryPlan` reads it.
- Flux, core flux, peak and SNR use the signed residual sum. Only the moment seed and centroid weights clip at zero. The shot-noise term of the noise model uses `max(flux, 0)`, because a Poisson variance cannot be negative.
- FWHM estimation seeds the stamp from `fwhm.expected` and repeats the measurement once at the radius the first estimate implies.
- The Gaussian fit models an elliptical, rotated profile through its inverse covariance: `amp · exp(−½(a·dx² + 2b·dx·dy + c·dy²)) + bg`, with `a·c − b² > 0`. This form has no angle wrap and no degenerate angle for a round star, which a `(σx, σy, θ)` form has. Principal widths and eccentricity derive from `(a, b, c)` after the fit. One FWHM definition lives in `math::fwhm`: the geometric mean of the principal widths (the circle of equal area), used by the moments path and both fits.
- Config: `Deblend::{LocalMaxima { min_prominence }, MultiThreshold { n_thresholds, min_contrast }}`; `FwhmMode::{Fixed(f32), Auto { fallback: f32 }}`; `BackgroundRefinement::Iterative { mask_dilation }`; `Option` instead of `0` sentinels. `validate` names match the field names.
- Peak lists keep the brightest `MAX_PEAKS`, not the first in raster order. All per-frame buffers come from `JobScratchPool`.
- `SATURATION_PEAK` derives from `ImageMetadata::data_max`.

Tests: an empty-sky stamp with SNR ≈ 0 at r = 7, 13, 15; a star on a 0.1 sky against the same star on a 0 sky, same peaks and same split; FWHM 5…10 recovered within the fit noise; a 45° elongated star with eccentricity above the cut under `GaussianFit`; a round star whose fit converges with the same iteration count as an elongated one.

### W5 — lumos: image-op numerics

Closes: WR "The default gradient removal puts both auto stretches on their degenerate branch", "Normalization and image ops lose precision…", image-op items in "lumos stacking numerics…", "SIMD backends…", "The same constant or formula…".

Target design:

- `BackgroundMode::Subtract` writes `p − m + mean(m)`, which keeps the sky level, as Siril does (`remove_gradient` in `src/algos/background_extraction.c`). Siril adds one mean over all channels, which also neutralizes the sky colour. Lumos adds each channel's own model mean, like its `Divide` mode, because colour neutralization is a separate op (`NeutralizeBackground`).
- `solve_asinh_beta` and `StfCurve::new` return an error when the target is out of reach, instead of the range limit.
- RAW normalization stores the span and divides, as the FITS path does. `BlackLevel::span` returns the stored span.
- `hdr_map` uses `math::sum::sum_f32`. GHS uses the `expm1`/`ln_1p` forms. Local contrast interpolates between LUT bins.
- One derived constant for each of `FWHM_TO_SIGMA` (named for what it is), `MAD_TO_SIGMA`, Lanczos, Rec. 709 luma, with an f32 cast from the f64 value.
- One subsample helper with a rounded-up stride; one `PointNormalization`; one spline evaluator; one Lanczos weight table.

### W6 — lumos: untrusted-input boundary

Root cause: release asserts guard values that come from files, user graphs and user configs.

Closes: WR "Untrusted data reaches code-contract asserts and panics" (lumos and lens items), "Stage configuration that is never validated…", "The stacking pipeline applies values resolved for its inputs to the survivors".

Target design:

- `calibrate` returns `CalibrationError::AlreadyCalibrated`. Flat normalization returns `CalibrationError::NonPositiveFlat`. `duplicate_min_separation = 0` means no deduplication.
- `CosmicRayConfig::validate` and a `TiledOnnxConfig` validation join `AlignStackConfig::validate`. `stride` must be in `1..=WINDOW`.
- Manual weights are indexed by input frame and selected for the survivors. `AlignStackConfig::validate` checks the count before decode.
- Lights are checked for non-finite samples at entry. Dimensions are checked at decode time. The detector pool uses `MemoryPlan`. Per **D9**, `align_and_stack`, `stack` and `stack_images` get the same checks.
- The `cfa_type == None` policy for cosmic rays is one place: skip with a warning.

### W7 — lumos: combine and normalization ownership

Closes: WR "Normalization and combine: measured statistics thrown away…", "lumos error types route through each other…", "Calibration-master construction is duplicated…", lumos items in "Public API… with no production user" (the `lumos::lib.rs` item is closed as "kept" per **D9**).

Target design:

- Per **D10**, the paired photometric fit over the common domain is the only estimator for `Normalization::Global`. First fix the Deming inlier window (`combine-review.md` §12) so the stars with a photometric lever arm stay in the fit. Partial coverage then changes only where statistics are measured. The paired fit runs chunked over the stored planes, so the spill tier reads each plane once for it. Darks and bias keep `Normalization::None` and flats keep `Multiplicative`, so the paired fit never runs on a frame without stars.
- Per **D8**, `MasterRole` owns the role → preset table. `CalibrationMasters::from_files` and `RoleStack` go.
- Per **D9**, `DefectMap` stays public: `dimensions` stops being an `Option`, and `correct` keeps its mask instead of rebuilding it per light.
- `FrameCache` owns `frame_norms`. `process_chunked` reads them from `self`. `run_stacking` takes the normalization from the config only.
- Normalization measures medians only (no unread MAD).
- Pipeline `Error` gets `Cancelled`, one `NoFrames`, `From<FrameStoreError>`. Calibration errors leave the combine `Error`.
- Code-contract checks on frames the pipeline produced become `debug_assert!`.

Tests: a set of registered frames with a known gain and offset per frame, recovered by the paired fit; the same set with one `BLANK` pixel in one frame, which must give the same gains within the fit noise.

### W8 — common: typed ids and introspection

Closes: WR "Typed ids are parsed from strings at run time…", "File-source identity…" (with W2), common items in "Public API…" and "common, quickbench: small API…", "Missing `const fn`…" for `id_type!`.

Target design:

- `id_type!` gets `pub const fn literal(s: &str) -> Self`, built on `uuid::Uuid::try_parse`, which is `const fn` in uuid 1.26. A bad literal in a `const` item fails at compile time. `From<&str>` and `From<String>` go. All 57 string-literal ids in production become `const` items. `nil`, `is_nil`, `as_u128`, `as_uuid` become `const fn`.
- The two hand-typed ids in `lens/src/image/nodes/processing.rs` stay. `AGENTS.md` says that a shipped id is the identity saved graphs bind to and never changes, and that rule is more specific than "no backward compatibility". The risk the finding names is a collision. `Library::add` already asserts that func ids are unique, and a new test asserts that every func and type id across the `lens` and `scenarium` libraries is unique. The WR item is closed as "kept: shipped ids never change".
- `#[derive(Introspect)]` takes `#[config(type_id = "…")]` like `IntrospectEnum`. `lens`'s `NodeConfig` trait goes. Introspection metadata is `&'static str` and `&'static [..]`.
- `serde.rs`: one `serialize_into(&T, ..)` signature, a real scratch for Bitcode, `SerdeFormat::Lz4` and `deserialize_from` removed.

### W9 — scenarium: worker protocol and engine state

Closes: WR "A disabled producer's stale digest keys its consumer's cache", "scenarium's `WorkerStatus` folds three messages into one record", "scenarium threads and stores state for features nothing uses", "scenarium flattens errors to strings…", "scenarium's graph API makes hosts mirror its private rules", scenarium items in "Placeholder values…" and "Public API…".

Target design:

- The `Bind` arm of `node_digest` folds `InputTag::Unbound` for a producer that is not runnable, with the predicate `collect_inputs` uses. The digest changes, so `FORMAT_VERSION` goes from 9 to 10 and old blobs miss (section 5).
- `WorkerReport::{Activity(WorkerActivity), Progress { node_id, phase }, Completed(RunSummary)}`. `Progress` is sent by value: it holds no heap data, so there is no `Arc` and no buffer per event. `RunPhase::Finished` carries the outcome, so a failed node is never painted executed.
- Per **D4**, `ContextType`, `ContextStore` and the `&mut ContextStore` parameters go.
- Per **D5**, wildcard outputs stay. `OutputTypes::update` reads the const kind without a `ConstValue` clone and reuses its tables. A full recompute on a graph edit costs what today's per-frame update costs, so the worst-case frame does not get worse.
- **Port-signature guard.** Bindings are stored by port index, and today a binding to a port the func no longer declares silently becomes unbound, while a reordered port binds the wrong input. Each func gets a signature digest over its input and output names and types. The document stores the digest per func it uses, and loading a document whose digest differs from the library fails with an error that names the node and the func. This lands before W10 changes any node's ports (**D12**), and it protects every later port change.
- **Declared overrides (D12).** `FuncInput` can name the input it overrides (`overrides: Option<usize>`). The compiler resolves the override. Darkroom marks the overridden input from the declaration, without running the graph.
- `FuncLambda` and `EventLambda` are required constructor arguments. `NodeState::MissingLambda` and `RunError::MissingLambda` go.
- `DiskStore` holds the root only. Codecs move into `CompiledGraph` as a shared handle from the library. Codec coverage is checked before any I/O, and the verdict is stored.
- `CompileError`, `RunError::Invoke` and `InvokeError::External` keep typed sources. `StampError::Io` carries the path.
- `Graph::find` and `Library::by_id` return `None` for a nil id. `DetachedNode` gets a fallible constructor that owns the rules `attach_node` checks. `Library::types` becomes private with an idempotent `register_type`.
- `ExecutionEngine::compiled` is a `CompiledGraph` (default empty), not an `Option`.
- `Invocation` gets a typed accessor for required inputs, so `lens` drops 37 `.expect` calls.

Tests: the golden digest of one small node and the golden bytes of one blob (W14), updated in this step with the version bump; a document saved against one func signature and loaded against a changed one, which fails with the named error.

### W10 — lens: adapters

Closes: WR "lens re-states lumos's presets and defaults", "lens node lambdas repeat boilerplate…", "Pixel layout is converted and copied at crate boundaries", lens items in "Untrusted data…", "Public API…".

Target design:

- Per **D12**, all six processing nodes take a preset picker and an optional `Config` port that overrides it through W9's declared override. Their signature digests change, so a document saved with the old ports fails to load with W9's error, never with a wrong binding.
- `lens` uses the `lumos` enums directly (`StretchMethod`, `ScnrMethod`, `BackgroundMode`, `CombineMethod`). The four mirror enums, `preset_enum!` and the copied defaults go. Preset becomes a trait with an associated knob type and default methods.
- `register_blend` compares descriptors and returns `InvokeError`. `register_transform` rejects a zero scale.
- Stacking outputs leave planar (`Image::from(LinearImage)`). `AlignStackConfig.stack.quality` follows `demand`. The codec stores the layout it was given; its format byte changes, so the image codec version goes up (section 5). `Image::take_or_copy` replaces the three copies of that match.
- The Build Masters "Sigma" description names the defect threshold. "Reference" becomes an optional input.

### W11 — imaginarium: one format type, one rounding rule

Closes: WR "Image conversion and SIMD kernels return wrong or CPU-dependent pixels", "GPU blend and contrast dispatch fails…", "imaginarium integer outputs truncate or round differently…", "imaginarium `ColorFormat` admits 18 combinations…", "imaginarium's GPU stack has no consumer…", "imaginarium plumbing…", imaginarium items in "Untrusted data…", "Public API…".

Target design:

- `ColorFormat` is a 9-variant enum with `channel_count()`, `channel_size()`, `channel_type()`. `validate()`, `ALL_FORMATS.contains` and the tuple `From` go. The dispatch tables match exhaustively, with no fallback arm. The callers in `lens`, `lumos` and `darkroom` change in the same step (rule 7).
- One `Sample` trait states full scale, widening and narrowing. Every narrowing rounds half to even, on CPU, SIMD and WGSL (WGSL `round` is half-to-even). u16 → u8 is `round(v·255/65535)`. Every SIMD tail calls the scalar reference, so phase 2's tail fix and this rule do not conflict. Float → int multiplies in f64, or in an f32 form that an exhaustive sweep over the input range proves equal.
- `convert_to` copies when the formats are equal. `convert_image` asserts different formats.
- GPU (kept in `imaginarium`, dropped from the workspace per **D4**): 2-D dispatch, limits clamped to `adapter.limits()`, every GPU test compared with `apply_cpu`.
- `Buffer2` index: `debug_assert!` on `x < width`, slice bound for the rest. Owned-data constructors reuse the allocation.
- `cfg_x86_64!` / `cfg_aarch64!` become `#[cfg(target_arch = …)]`, which also brings the SIMD test files under `rustfmt`.

### W12 — fits-well: WCS correctness and API shape

Closes: WR "WCS evaluates invalid or unprojectable input…", "WCS unit scaling…", "Compressed tables written by fits-well cannot be read back", "fits-well errors carry the wrong meaning", "fits-well's reader re-parses…", "fits-well data shapes…", "fits-well table compression disagrees with §10.3…", "fits-well `-TAB` coordinates…", "fits-well module structure…"; `ISSUES.md` ASCII float and examples.

Target design:

- Each projection gets its native-latitude domain test and returns `FitsError::WorldOutOfDomain` (wcslib `PRJERR_BAD_WORLD`). The pole candidates wrap to [−180°, 180°] before selection, as wcslib `celset` does; no valid candidate is an error.
- A lone celestial axis or a mixed-system pair is marked unsupported. `axis_world` reads the celestial transform.
- One `unit.rs` resolver ("[multiplier][SI prefix]base") for angle, spectral and time units. An unknown unit is an error. `WcsAxis` stores `crval` and `cunit` in the same unit.
- Compressed tables: the read path applies `TDIMn`/`TSCALn`/`TZEROn`/`TNULLn` to the uncompressed column only. Every VLA is compressed with its `ZCTYPn`. The read path honours the declared codec.
- `Card` is an enum with a payload per kind. Column data is one buffer plus row ranges.
- The `Header` forwarders go; `Wcs::from_header` and `FitsTime::from_header` are the one path. The writer has five operations, each with `header: Option<&Header>`. The `lumos` callers change in the same step (rule 7).
- The ASCII float parser builds `"{mantissa}e{exp}"` and parses once (strtod parity).
- The examples take their file from an argument.

Reference values: the WCS tests use values derived by hand from Calabretta & Greisen (2002), as the current golden headers do. An astropy cross-check would be a stronger oracle, but astropy is not installed on this host; installing it needs your approval.

### W13 — darkroom: layering and editing

Closes: WR "Raising a node is never a no-op…", "Esc commits the text a user tried to cancel", "Undo coalescing has no gesture identity…", "`darkroom::core` claims to be frontend-free…", "Loaded documents and preferences…", "Canvas gestures…", "Canvas per-frame work…", "darkroom core keeps containers…", "darkroom GUI repeats per-frame lookups…"; `ISSUES.md` format carry.

Target design:

- Undo has gesture sessions. A pointer press opens a gesture with a new `GestureId`. The open entry stays decoded on `ActionStack`. Release, Esc or any other step seals it: a no-op entry is dropped, otherwise it is encoded once. Coalescing zips the latched member lists. Encoding moves from every drag frame to the release frame, so the worst-case frame gets cheaper.
- `front_z` excludes the raised item, and a raise of the frontmost item emits no step.
- One `DraftEdit` widget helper returns `Commit(text)`, `Cancel` or `Editing`. The value editor, the preferences path field and the inline rename use it. `EditBuffer::blur_edge` becomes `blurred = latch && !focused; latch = focused`.
- Esc deselects only when no gesture was in flight. Group drag and preview drag honour cancel.
- `core::edit` owns `DocumentRequest` and the layout-impact flag. `gui::Requests` wraps the document queue and the app queue. `core` never imports `gui`. Per **D7**, the `core` module doc names the palantir data types it may use.
- `Document::validate` checks that `Graph` is present and pinned and that the seed is `DOCK_SEED`, and it runs W9's signature guard. `Preferences::load` reports a parse error and does not overwrite the file.
- Per-frame costs: the palette keeps its row buffer and re-filters on a query change; the breaker keeps a scribble bounding box; `GraphCtx` recomputes `OutputTypes` only when the graph changes (**D5**). Darkroom marks an overridden input from W9's declaration (**D12**).
- Per **D13**, the per-pane narration and the unreachable branch in `GraphUI::appearing` go.
- `fmt_elapsed` and `fmt_bytes` choose the unit after rounding.

### W14 — test infrastructure

Closes: TR "The test harness breaks its own contracts", "Nondeterministic and environment-dependent tests", "SIMD and GPU backends without an exact cross-check…", "Placement, gating and bench layout", "Unused fixtures and stray test output", "Test-only code in production…".

Target design:

- **Benches:** `lumos` and `fits-well` get a `bench` feature that implies `internals`, per the rules. `lumos` gates `mod bench` on `all(test, feature = "bench")` (quickbench benches are `#[test]`). `fits-well` makes `criterion` optional behind `bench`. A `bench`-gated `pub mod bench` facade replaces `pub mod internals` in `fits-well`. Every documented run command names `--features bench`.
- **Real data:** one gate, the `real-data` feature. Remove `#[ignore]` from those tests. Every dataset reader is gated. The pipeline bench writes into a temp directory, never into the dataset.
- **lens dev-dependencies (D11):** add `common` with `internals` and tokio with `test-util` to `lens/Cargo.toml` `[dev-dependencies]`. The `fs_watch` debounce tests then run on paused time and assert the fire at exactly 250 ms.
- **Temp files:** `common::internals::TempDir` everywhere. `ScratchDirectory`, fixed `test_output/` paths and the three `lens` schemes go. Debug image output is opt-in through an environment variable.
- **Determinism:** one seeded `test_config` for registration; every `RansacEstimator` in tests is seeded; scenarium waits use `settle` and paused time; no `chmod 000` fixtures without a root check and a restoring guard; fixed ids instead of random ones where order matters.
- **SIMD:** `assert_simd_matches_scalar` gets a tier argument and runs every tier the host supports, with `DATA_SHAPES × SWEEP_WIDTHS` including exact `.5` ties and out-of-range lanes. A missing feature is reported, not a silent pass. imaginarium conversion gets the `kernel_tiers()` sweep.
- **GPU:** `test_gpu()` failure prints a skip line and counts; GPU tests compare with `apply_cpu`.
- **Harness split:** `lumos/src/testing/mod.rs` splits per type (`TestRng`, CFA builders, real-data paths). `TestRng::next_f32` uses 24 bits.
- **Golden bytes:** one node digest and one blob layout in scenarium; the lumos decode hashes for W2's `DECODE_VERSION` guard; the characterization snapshots of rule 2.

The individual test items in TR (cannot-fail tests, loose tolerances, duplicates, missing tests) are done inside the workstream that owns the code under test, because they verify that workstream's changes.

### W15 — comments and docs

Closes: WR "Docs that describe code that no longer exists", TR "Stale, wrong and change-narrating comments". Done when a workstream touches the file, per the comment rules. A final pass covers the files no workstream touched.

---

## 5. Persisted formats

Every persisted format the plan changes, and what an old file does after the change. "Fails loudly" means an error that names the file and the reason.

| Format | Changed by | Version | Old file after the change |
|---|---|---|---|
| Lumos CFA master FITS (`LUMOSFMT`/`LUMOSVER`) | W1 (`LUMSCALE`) | `CFA_FITS_VERSION` 1 → 2 | Fails loudly; `lens` rebuilds the master from its frames. |
| Lumos frame cache (sidecars, planes) | W2 (`CacheKey`, frame 0 sidecars, null plane) | `SIDECAR_FORMAT` 1 → 2, plus `DECODE_VERSION` in the key | Cache miss; the frame decodes again. Persistent cache entries of the old format are replaced. |
| Lens master cache marker | W1, W2 (`FileIdentity`) | marker carries the master format version | Cache miss; rebuild. |
| Scenarium disk blobs | W9 (digest of disabled producers) | `FORMAT_VERSION` 9 → 10 | Cache miss; recompute. |
| Lens image codec payload | W11 (format bytes, done: version 3), W10 (layout byte) | codec version up | Cache miss; recompute. |
| Darkroom document (graph bindings) | W9 (signature digest), W10 (D12 ports) | document carries per-func signature digests | A document with changed ports fails loudly naming the node; one with unchanged funcs loads. |
| Darkroom preferences | W13 (load error is reported) | none | A parse error is reported and the file is kept, not overwritten. |
| Undo history | W13 | not persisted | — |

---

## 6. Order

| Phase | Content |
|---|---|
| 0 | 2.1 dev profile; 2.2 lint config and the per-crate migration for the mechanical lints |
| 1 | W14 core: bench feature, real-data gate, `TempDir`, seeding, SIMD tier harness, golden-bytes tests, characterization snapshots (rule 2) |
| 2 | Local High fixes that need no redesign, each test-first: spill directory (W2's two modes and marker), raise no-op, `DraftEdit`, RCD step 4.0 start, signed flux, background pedestal, dedup 0, `draw_circle` clamp, `convert_to` copy, SIMD tails, the six `ISSUES.md` bugs |
| 3 | W8 (ids) — touches every crate, so it goes before the crate rewrites |
| 4 | W12 (fits-well) and W11 (imaginarium), each with its workspace callers in the same step; they can run beside phase 5 |
| 5 | W1 → W2 → W3 → W4 → W5 → W6 → W7 (lumos: domain before cache, cache before registration, geometry before detection) |
| 6 | W9 (signature guard and declared overrides first) → W10 → W13 |
| 7 | W15 final pass; the remaining lint crate attributes move to `[workspace.lints]`; every review file is empty |

Verification after each step: the chain for the touched crates (`cargo fmt -p <crate> && cargo clippy -p <crate> --all-targets --all-features -- -D warnings && cargo test -p <crate> --tests --all-features`, with `--features ml,internals` for `lumos` tests). Submodules run their own chain from their own directory. A change to a public type that other crates use runs `--workspace`. SIMD changes add rule 8.

---

## 7. Coverage

Every WR group maps to a workstream:

| WR group | Workstream |
|---|---|
| The frame spill directory deletes a directory… | W2 (phase 2) |
| SIP distortion is fitted in the target frame… | W3 |
| Registration accepts a transform supported by 2–4 stars… | W3 |
| Star measurement rectifies sky noise… | W4 (phase 2) |
| Deblending runs on sky-included pixel values | W4 |
| FWHM auto-estimation measures on the minimum stamp | W4 |
| The Gaussian profile fit is axis-aligned… | W4 |
| A reloaded calibration master always fails… | W1 |
| Frame validation differs by entry point… | W1, W2 |
| A disabled producer's stale digest… | W9 |
| WCS evaluates invalid or unprojectable input… | W12 |
| WCS unit scaling… | W12 |
| Compressed tables written by fits-well cannot be read back | W12 |
| Image conversion and SIMD kernels… | W11 (phase 2) |
| GPU blend and contrast dispatch fails… | W11 |
| Untrusted data reaches code-contract asserts… | W6, W10, W11 (phase 2) |
| Raising a node is never a no-op… | W13 (phase 2) |
| Esc commits the text a user tried to cancel | W13 (phase 2) |
| RCD demosaic reads the wrong pixels… | phase 2 |
| The default gradient removal puts both auto stretches… | W5 (phase 2) |
| Typed ids are parsed from strings at run time… | W8 |
| Undo coalescing has no gesture identity… | W13 |
| Pixel layout is converted and copied at crate boundaries | W10, W11 |
| `darkroom::core` claims to be frontend-free… | W13 |
| Loaded documents and preferences… | W13 |
| Canvas gestures… | W13 |
| Canvas per-frame work… | W13 |
| The stacking pipeline applies values resolved for its inputs… | W6 |
| Stage configuration that is never validated… | W6 |
| Memory budgeting re-implemented… | W2 |
| Normalization and combine… | W7 |
| The disk frame cache keys, names and sidecars… | W2 |
| File-source identity is implemented three times… | W2 |
| Calibration-master construction is duplicated… | W7 |
| `Transform` and drizzle disagree… | W3 |
| Registration hot loops recompute work… | W3 |
| Star-measurement fits and peak selection… | W4 |
| Star-detection config holds variant fields… | W4 |
| RAW and FITS loading misclassify… | W1 |
| Normalization and image ops lose precision… | W5 |
| Four parallel enums describe one sensor fact… | W1 |
| imaginarium integer outputs truncate or round differently… | W11 |
| imaginarium `ColorFormat` admits 18 combinations… | W11 |
| imaginarium's GPU stack has no consumer… | W11 |
| scenarium's `WorkerStatus` folds three messages… | W9 |
| scenarium threads and stores state for features nothing uses | W9 |
| lens re-states lumos's presets and defaults | W10 |
| fits-well errors carry the wrong meaning | W12 |
| fits-well's reader re-parses and copies… | W12 |
| fits-well data shapes… | W12 |
| fits-well table compression disagrees with §10.3… | W12 |
| fits-well `-TAB` coordinates and projection dispatch | W12 |
| The same constant or formula is defined more than once… | W3, W5, W11 |
| File-format and keyword knowledge spelled in several places | W1, W11, W12 |
| Public API, dependencies and derives with no production user | W7, W8, W9, W10, W11 |
| Placeholder values and derived fields stored beside their source | W1, W3, W4, W9, W12, W13 |
| lumos stacking numerics with avoidable loss | W3, W5, W7 |
| SIMD backends and duplicated kernels disagree… | W4, W5, W11 |
| lumos star-detection deblender structure… | W4 |
| lumos error types route through each other… | W7 |
| scenarium flattens errors to strings… | W9 |
| scenarium's graph API makes hosts mirror its private rules | W9 |
| lens node lambdas repeat boilerplate… | W10 |
| darkroom core keeps containers and checks it does not need | W13 |
| darkroom GUI repeats per-frame lookups and theme values | W13 |
| fits-well module structure, dispatch and API shape | W12 |
| imaginarium plumbing that could be shared or simpler | W11 |
| lumos io and support leftovers | W1 |
| common, quickbench: small API and data-shape issues | W8, W14 |
| Docs that describe code that no longer exists | W15 |
| Style rules not applied | 2.2 and the owning workstream |

TR sections map to W14 (harness, determinism, SIMD/GPU, placement, fixtures, test-only code) and to the owning workstream (cannot-fail tests, loose tolerances, missing tests, second sources of truth, duplicates). The six `ISSUES.md` bugs are in phase 2. The feature list in `.notes/todo.txt` is not part of this plan.

---

## 8. Progress log

Decisions taken during implementation that the plan did not foresee, newest last.

- **Sample domains convert instead of matching exactly (W1, done early).** The real dataset failed calibration: each RAW frame is normalized by its own `maximum − black`, the black level moves between frames (1019, 1023, 1024), and the domain check demanded equal spans — so the real-data pipeline had failed since 2026-08-12. `SampleDomain` now records whether its scale was declared or assumed; two declared scales in one unit convert by their exact ratio, an assumed one must match. `FitsFloatScale::Normalized` counts as assumed: "already in [0, 1]" says nothing about ADU, and declaring it would convert a Siril-normalized frame against 16-bit ADU by 65 535. Masters record `LUMSCALE`; CFA FITS version 2.
- **RAW normalization divides by an exact span (W5 item, done with W1).** The f32 reciprocal rounded twice and made the span read back as 15359.999.
- **Real-data fixture.** `stacked_light.tiff` is produced by `bench_full_pipeline` (TR item still open). It was regenerated once so the real-data tests could run; 22 of 24 pass. The two failures are logged in `ISSUES.md`.
- **`ColorFormat` is `ChannelCount × SampleType`, not a 9-variant enum (W11).** All nine products are formats, so the struct admits no invalid value, and most dispatch keys on the sample type alone. `ChannelSize`/`ChannelType` are gone.
- **Luminance rounds (W11).** The Q16 Rec. 709 sum rounded down; it now rounds half to even like every other narrowing, and the weights are derived from the standard's decimals at compile time.
- **NaN passes every clamp the way `f32::clamp` does (W11).** The SSE/AVX clamps put the value second, where `maxps`/`minps` return it for a NaN. Test comparisons treat any NaN as equal to any NaN, since Rust leaves NaN payloads unspecified.
- **The GPU transform interpolates in native units, like the CPU (W11).** GPU tests compare with the CPU within 1 LSB for integers and `1e-6` for `f32` (WGSL division is 2.5 ULP and may fuse multiply-add).
- **Lens image codec version 3 (W11).** The format is two header bytes now; old cache entries miss (section 5).

- **WCS follows wcslib, with two stated departures (W12).** Each projection refuses a world point outside its native-latitude domain (`PRJERR_BAD_WORLD`), and the pole candidates wrap before selection as in `celset`. The SZP limb uses the exact tangency radius √((z_p−1)² + s²) where wcslib uses an approximation; a one-element `-TAB` index is a valid axis (Υ = ψ − Ψ₁ + 1) where wcslib rejects it. Both departures are documented at the code.
- **Unit prefixes follow wcslib's table (W12).** `rad`, `s`, `m`, `Hz`, `J` and `eV` take every SI prefix; `a`/`yr` take multiples only; `deg`, `arcmin`, `arcsec`, `mas`, `min`, `h`, `d`, `cy`, `erg` and `Angstrom` take none. A numeric multiplier is an exact power of ten.
- **Raw VLA cells stay (W12, review item closed).** The review asked to compress every VLA cell with its `ZCTYPn`. cfitsio's `imcompress.c` stores a cell raw when the codec does not shrink it, and reads a stored length equal to the raw length as raw; files from cfitsio rely on that. fits-well keeps cfitsio's rule on both sides.
- **The reader resolves geometry once (W12).** `Hdu` keeps the image geometry and table schema its header declares; a malformed header leaves them unresolved and the accessor reports why. A compressed image decodes from its table viewed in place; sections compact their tile rows into a reused buffer.
- **The public modules are `lib.rs` facades (W12).** The internal modules are `bintable`, `header_model`, `time_coordinates` and `world_coordinates`; `#[path]` is gone. The `Header` forwarders went: `Wcs::from_header`, `FitsTime::from_header`, `TimeBounds::from_header`, `TimeCoordinate::observation` and `PhaseAxis::from_header` are the one path, and the observation time now carries its scale.
- **A null mask still decodes in the codecs' `i64` plane (W12, review item closed).** Every codec decodes into `Vec<i64>`; the mask reuses the worker's buffer, so a masked tile allocates nothing. A `u8` mask plane would need a second output type in every codec for a rare path, so the review's "per-tile waste" item is closed on the two parts that were waste: integer images no longer decode the quantization columns, and the Rice writer emits words, not bits.
- **Variable-length rows are `Ragged<V>` (W12).** One buffer of values plus the row ends, for every `P`/`Q` decode, the writer's input, ASCII text and compressed-table arrays. An `A` column is its bytes, one element each as FITS counts them; `CharacterField` is a borrowed view of one field.
- **An MJD rounds at its own precision (W12, found by the time tests).** `Datetime::to_mjd`, the numeric epochs and a `JDREFI`+`JDREFF` split went through a JD, which rounds 64 times coarser in the present era; each now shifts its whole-day part first. Test tolerances that hid this are exact equalities; the remaining ones state where their bound comes from (printed golden precision, `ZSCALE`, the spectral cancellation scale).
- **fits-well has the full lint set at `warn`, clean (plan 2.2, W12).** The sign-cast sweep found two silent wraps in the codecs: a PLIO line list of 2³⁰ words or more, and an HCOMPRESS tile dimension past `i32`, each written into a header field too narrow for it. Both are refused now. Same-width reinterpretations use `cast_signed`/`cast_unsigned`; codec kernels with a proven bound carry an `expect` that names it.
- **fits-well benches follow the bench convention (TR).** The criterion groups live in `bench.rs` files beside the code under a `bench` feature with `criterion` optional; `benches/*.rs` only wire `fits_well::bench`. The `internals` feature now serves only the allocation-counting integration test.
- **SIMD cross-checks run every tier the host has (W14).** `testing/simd_check` takes a list of `Backend { tier, kernel, min_width }` and runs each one the CPU supports; one it does not support prints a `SKIPPED` line. Sums are held to `2·γₖ·Σ|tᵢ|` per element (`ScalarSimd::of_sums`, Higham §3.1) and transcendental results to a stated relative bound, instead of one absolute tolerance per test. The shapes gained exact halves and lanes outside a unit domain.
- **The background spline kernels compute `t` per lane from its index (W14, found by the per-tier test).** All three stepped the parameter by repeated addition and drifted from the scalar `start + i·step` along a segment (AVX2: 9e-5 on a value of 68 at width 31). The SSE4.1 path is now bit-exact with scalar; the FMA paths differ only by rounding.
- **NEON is compile-checked without an aarch64 C++ toolchain.** `CXX_/CC_/AR_aarch64_unknown_linux_gnu=/bin/true cargo clippy -p lumos --target aarch64-unknown-linux-gnu --all-targets --features ml,bench,real-data` runs `libraw-rs-sys`'s build script as a no-op, which is enough because clippy does not link. The first run found five aarch64-only breaks (x86 items imported unconditionally in `math/sum` tests and benches, lints in three NEON kernels), now fixed. NEON tests did not run: the macOS host was not available.
- **Characterization snapshots exist (rule 2).** `lumos/src/testing/characterization` pins a 64-bit BLAKE3 prefix per stage — FITS decode, RAW decode (`real-data`), calibration, demosaic, detection, registration, warp, combine, stretch — on fixed synthetic input; `imaginarium` `conversion_snapshot` pins one FNV-1a digest per source format over every conversion. The lumos digests are pinned on `x86_64` with AVX2+FMA (any other host prints `SKIPPED`), and they hold at 1, 3, 7 and 16 rayon threads. The two decode snapshots are the golden test W2's `DECODE_VERSION` guard attaches to. A step that moves a snapshot states why in this log.
