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
- **TPS stays as work in progress (D3, review item closed).** Its gate is now `cfg_attr(not(test), expect(dead_code, reason))`; the review asked to remove it, which D3 overrides.
- **The root `test_resources/` is gone.** Its two TIFFs were unused; `full_example.fits` moved to `lumos/test_resources/`, the only crate that reads it.
- **Normalization tests stay split until W7, frame-statistics tests until W2.** The review asks to move `stack/tests.rs`' `compute_frame_norms` tests and `cache/tests.rs`' `FrameStats::measure` tests to their owners; W7 rewrites the Global estimator (D10) and W2 the frame store, so each moves them as it rewrites the code they test.
- **Ids are compile-time constants (W8).** `id_type!` lost its panicking `From<&str>`/`From<String>`; `literal` is a `const fn` over `Uuid::try_parse`, so every production id is a `const` item whose malformed literal fails the build, and `Func::new` takes a `FuncId`. The introspect derive takes `#[config(type_id = "…", name = "…")]` on structs as on enums, so lumos's configs carry their own wire ids and lens's `NodeConfig` is gone; field metadata is `&'static`. The two hand-typed lens ids stay: AGENTS.md says a shipped id never changes, and a darkroom test composes every library, which refuses any repeated id.
- **One serde codec call, no scratch parameter (W8).** `serialize_into(&T, format, &mut Vec<u8>)` appends; RON text goes straight into the buffer. Bitcode's serde encoder has no reusable buffer (only its derive `Encode` path has `bitcode::Buffer`), and it builds its column encoders on every call, so no caller-owned scratch can make that arm allocation-free. The undo stack's per-frame cost is fixed where it is caused: W10's gesture sessions encode once per release. `SerdeFormat` lives in `serde/`, `Lz4`, `deserialize_from` and the file-extension lookup are gone, and so is the `lz4_flex` dependency.
- **`Introspect::fields()` still builds its `Vec` per call (W8, review item closed).** Its callers run once per library build and in tests, and each `default` is a `FieldValue` built from `Default`, which is not `const`. A `&'static` table would buy nothing on a path no frame reaches.
- **The RAW directory scan is `lumos::raw_files` (W8).** `common::file_utils::files_with_extensions` had no production caller; every caller passed `RAW_EXTENSIONS` and lived in lumos tests and examples, so it moved there with its tests.
- **One `SimdTier` for the workspace (W11).** `imaginarium::SimdTier` replaces `cpu_features`, the conversion module's `Tier` and lumos's test-only tier enum. The tiers form a chain per arch, detected once; a tier is supported only when every narrower one is, so a kernel table asks `tier >= SimdTier::Sse41`. Every imaginarium op takes the shape `kernel(tier, format) -> Option<Kernel>`: dispatch passes `SimdTier::widest()`, and the tests sweep every supported tier, so the SSE4.1 contrast kernels now run on an AVX2 host. lumos's `dispatch!` reads the same type.
- **The two warp engines stay separate (W11, review item closed).** lumos warps planar `f32` frames through an `f64` `DMat3` with coverage and confidence maps, because registration residuals are sub-pixel and the stack weights by coverage; imaginarium warps interleaved images of any format through an `f32` `Affine2` for display. One engine would have to carry both contracts, so neither would get simpler.
- **Image file formats are `imaginarium::FileFormat` (W11).** `read_file`, `save_file`, `SUPPORTED_EXTENSIONS` (derived at compile time), lumos's `SourceContainer::from` and `PREVIEW_IMAGE_EXTENSIONS` (FITS, RAW, then imaginarium's list, concatenated at compile time) all read it; lumos's own copy of the list is gone. The preview list order changed to imaginarium's (png before tiff); the order shows only in a file dialog's filter.
- **A graph cannot crash the worker with a singular transform (W11).** `Transform::is_invertible` (finite coefficients, normal determinant, finite inverse) is public, and the lens Transform node returns an input error when it is false; imaginarium's assert stays for code that skips the check.
- **lens's `Format` port is "As Is" plus `ColorFormat::name` (W11).** `ColorFormat::name` is a `const fn`, so the variant table is built at compile time and `conversion_target` compares `&'static str`s.
- **`CfaType` is the one sensor-pattern type, and `CfaImage` owns it (W1).** `SensorType`, `DemosaicKind` and `BitPix` are gone. `CfaType::from_libraw` answers `Option<CfaType>`: `None` is a sensor LibRaw processes itself (a linear DNG, sRAW or Foveon, `filters == 0` with three colours, or an exotic CFA), which loads as a `LinearImage` through LibRaw and fails a CFA load with "not a CFA frame". The pattern left `ImageMetadata`: `CfaImage::cfa_type` is required, so the "missing pattern" errors and the "absent means mono" fallback are gone, and a demosaiced image records what it went through in its provenance only. `ImageMetadata::sample_type` is fits-well's `SampleType` for a FITS source and `None` otherwise; RAW no longer claims 16-bit samples.
- **The frame cache holds every frame to the first one's pattern (W1).** `FrameStats` carries the pattern beside the domain and row order, `validate_cfa_types` runs in both constructors, and `StackProduct::cfa_type` hands it to the master. `SIDECAR_FORMAT` went from 1 to 2 for the new field; W2's change to the sidecars takes it to 3.
- **Optional FITS keywords degrade, load-deciding ones fail (W1).** `read_metadata` is infallible: an observation keyword of the wrong type or range is `None`, with a warning. What still fails a load is what decides how samples are read: the CFA pattern, `ROWORDER`, `QNTZSIG`, `BUNIT` (half of the sample domain, so the plan's list grows by it) and, for a float image, the `DATAMAX` that sets its scale. These are read before any plane decodes. The pointing falls back to `CRVALn` only from the axis whose `CTYPEn` is `RA--`/`DEC-`. `BAYERPAT = 'TRUE'` is refused unless `FitsLoadOptions::unstated_bayer_pattern` gives the phase.
- **One frame-set check, per frame, in one order (W1).** `validate_frame` runs geometry, then the set facts (`SetFacts`: sample domain, row order, CFA pattern, each held by the first frame that states it), then the samples, then the quality pair, for every frame of `from_stored_frames` and `from_stack_frames`. The loader runs the same per-frame order as each frame decodes and checks its facts against frame 0's as soon as frame 0 is known (in the memory tier frame 0 publishes them through a `OnceLock`, so no frame waits for it); the in-order fact pass runs once over the loaded set. A mismatched set stops before the rest decodes; a test pins it for both tiers with one worker. Reused cache planes now pass the quality-pair check, and `CalibrationMasters::from_images` holds every master to one CFA pattern. `FrameFacts` groups the three facts inside `FrameStats`.
- **An X-Trans layout is checked once, where it is made (W1).** `CfaType::XTrans` holds `XTransPattern` (public, `Copy`, a `const fn` constructor, deserialization checked), so `DemosaicError` is gone: a demosaic can only be cancelled, and its three callers each map one `Cancelled` marker, now shared with the FITS reader. The RAW entry points take `&LoadContext` like the FITS ones, so `LoadContext::check_cancelled` is the one check. The shared test X-Trans array was not a valid layout (its greens had unequal red and blue neighbours); the fixture is now a valid one, and the tests that hard-coded its colours moved with it.
- **A file's extension routes it once (W1).** `InputFormat::of` names the decoder (FITS, camera RAW, or an imaginarium raster) for all four loaders, with no lowercased `String` per load; `ImageError::UnsupportedFormat` carries the path. `TransferProvenance::DeclaredLinearRaster` became `FloatRaster`: nothing in a TIFF lumos reads declares a transfer function, so the name now says what is checked — a float sample type — and the docs and the rejection message say the same.
- **lumos io leftovers (W1).** The Markesteijn arena splits its five regions once at named offsets and casts them with `bytemuck`; its `unsafe` reinterpretations are gone. A border pixel whose clipped 3×3 window has no sample of a colour takes it from the nearest window that has one, not from its own sample of another colour. `same_color` has one neighbour walk for the median and the gather, and `CfaImage::repair_nulls` visits only the set bits of the mask (`BitBuffer2::for_each_set`). Progress: `current` counts completed units from 1, no stage sends an initial 0, the memory tier of the loader reports per frame, and the parallel stages report through `StageCounter`, which holds the count and the callback under one lock, so a callback sees `1..=total` in order.
- **Both demosaics filled too narrow a border (W1, found by the tests-review sweep).** RCD interpolated a 4-pixel border, but its stages chain stencils, so pixels 4 to 9 from the edge read values no stage had computed: on a uniform grey frame with no margins, red came out up to 37 % high there. Every calibrated light demosaics with no margins (`CfaImage::demosaic`), so every one had that band. The bilinear border is now `INTERPOLATED_BORDER = 10`, the measured reach (`RawTherapee`'s RCD uses 9), and a test pins it: from 10 pixels in, a crop demosaics bit for bit as inside a larger frame. Markesteijn had the same fault one pixel deep (border 8, reach 9); its border is 9 with the same test. The demosaic snapshot moved for this reason (now `39a9fab73c044eef`).
- **One `FileIdentity` for the workspace (W2).** `common::FileIdentity { len, mtime_ns: i128 }` (`of`, `from_metadata`) is what scenarium's path stamps, lens's master-cache key and lumos's frame cache compare; its doc states the coarse-timestamp limit. lens computes the master-cache key only with the cache on, so a frame that cannot be `stat`ed no longer fails a node that never reads the cache.
- **The frame-cache version is derived, not bumped (W2).** `CacheKey { source, decoder, decode_version: u64 }`, and `DECODE_VERSION` is FNV-1a over `DECODE_PINS`: the characterization digests of the four decodes a frame cache can hold (FITS and float TIFF as `LinearImage`, mosaic FITS and RAW as `CfaImage`). A decode change fails its characterization test until its pin moves, and the pin moving changes every key, so no person has to remember a bump. The sidecar tag works the same way: `SIDECAR_FORMAT` derives from `SIDECAR_PIN`, the digest of a fixed commit record and `FrameStats` in bitcode. Demosaic has no pin here because no cached frame is demosaiced. The stacking load always decodes with the default FITS options, so the options are not in the key.
- **The review's RAW-as-`LinearImage` case cannot happen (W2).** `LinearImage::from_file` refuses camera RAW and mosaic FITS, and `CfaImage::from_file` refuses rasters and plain FITS, so no file decodes both ways. The decoder still names the cache files and sits in the key, and the test shows the result that matters: a `CfaImage` load of a TIFF that a `LinearImage` run cached decodes and is refused, where a path-only key would have mapped the planes.
- **Frame 0 goes through the cache, and is always decoded (W2).** The stack metadata comes from frame 0's decode, and `ImageMetadata` does not serialize (fits-well's `SampleType` has no serde), so a disk-tier run decodes frame 0 every time. It is written and committed like the others, with the source-change check, and its planes serve a run in which it is not first. Cost: one decode per disk-tier run. `load_in_memory` and the disk path share `check_decoded`, so the per-frame check sequence is written once.
- **A commit record says whether quality planes were written (W2).** The `.commit` sidecar holds the key and `carries_quality`, so a masked frame whose quality planes are both gone is rebuilt instead of read as a frame with no nulls; `CachedQuality` is gone. `FrameSpill` owns every name — `_c{n}.bin`, `_coverage.bin`/`_confidence.bin` from `FramePlane`'s `Display`, `_nulls.bin`, `.stats`, `.commit` — and a cached frame's stem hashes its canonical path and decoder.
- **`StoredImage` spills its null mask (W2).** The mask's words go to `_nulls.bin`, are memory-mapped, and come back on `load`, so the spill tier warps with `MaskedWarp` like the RAM tier. The tier-equivalence test now masks two lights, and it fails when the restore is removed.
- **`RunMemory` holds the planning figure, not the override (W2).** The plan named `{ system, user_override }`; a share of the figure for one of several concurrent calibration roles is neither the system reading nor a user override, so the type holds `{ system, planning }` and `share(part, whole)` scales `planning`. Every public entry (`stack`, `stack_cfa_master`, `CalibrationMasters::from_files`, `calibrate_align_stack`, `align_and_stack`) reads it once and passes it down; the decode ceiling comes from `system` alone. `CacheConfig::available_memory` became `memory_override` and nothing writes it, so `resolved_with`, `planning_memory` and `with_resolved_memory` are gone, and `from_files`' sequential roles now share one reading.
- **One tier rule (W2).** `MemoryPlan::plan(RunShape, threads, available)` charges three peaks: every frame decoded plus one decode's transient, every warped frame plus the workers' working sets, and every frame beside the combine's resident output (`QualityPlanes::resident_bytes`: per-channel image, weight and variance planes plus coverage). `load_tiered` and `frames_fit_in_memory` plan a `RunShape::decoded_stack` (no warp; the transient is the statistics scratch), the pipeline a warping shape; `fits_in_memory` and `for_decoded_frames` are gone. The loader's decode fan-out is the plan's, so the in-memory tier charges a decode only its transient beyond the frame it leaves resident.
- **`CacheCore` carries a `CacheTier` (W2).** `Resident` or `Spilled { directory, chunk_memory: u64 }`: the directory and the figure its chunks size against cannot disagree, the `OnceLock` and the never-read `CacheConfig` are gone, and the coverage pass reads the same figure as the combine by construction. The pipeline's `FrameTier::Spill` carries the same two fields and becomes a `CacheTier` when it hands over.
- **The memory tests derive their constants (W2, TR).** The demosaic costs in `memory::tests` come from `CfaType::demosaic_memory` (one test pins them in planes, with the reason for each), `available_for_usable` inverts `MEMORY_PERCENT`, the align probe's ceiling is built from `PerFrameBytes` and `QualityPlanes::resident_bytes` (it charged one coverage plane per frame where production carries two quality planes), and the probes' decode floor reads `DECODE_TRANSIENT_FACTOR`. The combine's re-implemented loader sweep folded into `planned_concurrency_never_overshoots_its_tier_budget`, which now sweeps decoded stacks too.
- **Every `Transform` is stored with `m[8] = 1` (W3).** `from_matrix` divides by `m[8]` and returns `None` below `MIN_HOMOGENEOUS_SCALE` of the largest entry. The floor is derived in the code: `m[8]` carries an absolute rounding error of order `u·M`, the division hands its relative error to the other entries, and holding the moved point to 1e-3 px out to 2²⁰ px gives `u·2²⁰/1e-3` ≈ 1.2e-7. `try_inverse` is the checked inverse, `is_valid` is "`try_inverse` succeeds", `inverse` and `compose` panic where it cannot, and models below a homography assert an exact zero perspective row. `jacobian(p)` is the closed form, which the SIP fit uses now and drizzle will.
- **One `PointNormalization` (W3).** `registration::point_normalization` has `hartley` (RANSAC's estimators), `around` (SIP) and `new` (TPS's bounding box), with `normalizing_transform`/`denormalizing_transform` built directly; the RANSAC copy, its cached `transform` field and the second `centroid`/`avg_distance` are gone. The homography DLT denormalizes on raw matrices and normalizes once.
- **SIP corrects in the reference frame under any transform (W3).** Fit targets are `J(r)⁻¹·(t − T(r))`, exact up to affine and first order for a homography; clipping and the metrics use the corrected residuals `|T(r + c(r)) − t|` in target pixels, and stop below `RESIDUAL_RESOLUTION_PX` (1e-9, the f64 resolution of a 2²⁰-px coordinate times a few operations). The solve is an SVD of the rectangular design matrix with the standard rank test (`max(m, n)·ε·σ_max`), so Cholesky, the LU fallback and their 1e10 guess are gone; a clipping pass that would leave fewer than 3 × terms points, or a singular refit, keeps the previous fit. Basis powers are built by repeated multiplication. Tests: 10°, 180° and a homography (1.6e-11 px, 1.6e-11 px and 1.2e-3 px against bounds of 1e-8, 1e-8 and 1e-2 derived in the test); a narrow strip to 1e-9 px; a rank-deficient layout refused. The order-3-vs-4 comparison was decided by solver noise — on a symmetric grid the even order-4 terms are orthogonal to an odd field — and now compares order 3 with order 5.
- **Registration is held to `min_matches` after recovery (W3).** `RegistrationError::TooFewInliers`; `quality_score` lost its own inlier floor. `recover_matches` refits on the set it returns, falling back to the last transform that fit; `register` builds the reference and target k-d trees once (`KdTree::build` takes its `Vec`), and triangle matching and every Auto rung's recovery share the target tree. Match residuals are measured through `WarpTransform::apply`.
- **RANSAC checks its adaptive bound on every iteration (W3).** `best` is an `Option`, the iteration bound is updated on improvement and read on every pass, LO starts from the hypothesis's complete score and scores into a buffer it owns, and `RansacResult::iterations` is gone (the test counts samples through `ransac_loop`'s sampler). `PointMatch` holds `MatchIndices` and a confidence set once; its unread `votes` is gone. `StarMatch` holds the now public `MatchIndices`. The registration snapshot moved for the recovery refit and the earlier stop (now `62296654082d94ea`).
- **One FWHM constant and a full-precision `MAD_TO_SIGMA` (W3).** `math::fwhm::FWHM_PER_SIGMA` is `2·√(2·ln 2)` in f64 with the f32 cast from it, and every conversion goes through `fwhm_to_sigma`/`sigma_to_fwhm`; drizzle's 2.3548 copy is gone. `MAD_TO_SIGMA` is 1.482602218505602, checked against the normal CDF; neither change moves an f32 value.
- **A warp evaluates each output pixel's source position once, in f64 (W3).** `SourcePosition` splits a position into its integer cell and fraction in f64 and narrows only the fraction (6e-8 resolution, where an f32 position at x ≈ 6000 has 4.9e-4); `RowPositions` fills one row per worker — affine rows step their `x` terms, homography rows step numerators and denominator, a SIP model is applied per pixel — with exactly `WarpTransform::apply`'s operations, so the bits match. `warp_into` is one parallel pass over the output rows: each row's positions serve every channel, both quality maps and a masked frame's validity, and the row scratch is a `JobScratchPool` on `WarpBuffers`, handed back with the planes the spill tier reuses. Measured at 2k RGB with Lanczos3: SIP 85.7 → 52.4 ms, homography 51.8 → 42.6 ms; 4k mono `warp_into` 78.8 → 76.5 ms.
- **The f32 bilinear SIMD kernels are gone (W3).** AVX2, SSE4.1 and NEON computed source positions in f32 from f32 coefficients — the precision defect — and gathered corners with scalar loads anyway; with positions computed once per row in f64, what was left for them was a 4-sample blend. A single bilinear plane is slower for it (2k: 2.6 → 3.4 ms), the one figure that went up; `Config::fast()` is its only user and a frame-level warp shares the positions across channels. `SSE_F32_LANES` and `InterpolationMethod::lanczos_param` went with them.
- **One Lanczos tap convention and one window test (W3).** `LanczosOrder` (Two/Three/Four) replaces the order integer and its panic arms, owns the tables, and `LanczosLut::weights` is the one `(a − 1 − i) + f` / `(i + 1 − a) − f` convention for the row warp, the quality maps and the x86 gather's oracle. `source_position::window_inside` is the interior test for the row warp and both quality windows; the vector accumulate keeps its own wider bound, which is a load width, not a window. The kernels take `SourcePosition`, so the footprint test and the border live with the caller. The bicubic divisor guard and the Lanczos total guard became a debug assertion and a plain division, each with the bound that makes it safe. A masked RGB frame now holds a zeroed copy per channel (four planes with the validity plane, against three before) so its channels can share each row's positions.
- **The resample tests compare against one oracle (W3).** `kernel::internals::interpolate` evaluates every method from its definition at a `DVec2`; the row sweep holds `sample_row` to it across a translation, a similarity, a homography and a SIP correction at three image sizes, exactly for the separable kernels and to the summation bound for Lanczos. The two hand-written scalar Lanczos references in `row/tests.rs` and the bilinear SIMD tests are gone. The oracle had its own off-by-one at the right and bottom edges, which the old references hid. The warp, combine and registration snapshots moved for the position split (`0844b150fa4dea5a`, `b59d8cd028469c88`, `b4547f7a0ae61a15`).
- **Drizzle takes the registration's `WarpTransform` (W3).** `DrizzleFrame::warp` is reference → input, as `RegistrationResult::warp_transform` returns it, and drizzle maps input pixels through its inverse. Without SIP that is one transform; with SIP it is `InverseWarp` — `T⁻¹` and then Newton on `r + c(r) = T⁻¹(t)` from `T⁻¹(t)`, stopping at a step under 1e-6 px (Newton's next error, `½·|c''|·δ²`, is then under 1e-14 px) and failing after 32 steps, at a singular or non-finite Jacobian. A failing pixel deposits nothing and is counted in `DrizzleResult::unconverged_points` by the first band whose scan reaches its row, so overlapping scans count it once; `drizzle_stack`, `drizzle_images` and `finalize` return `DrizzleResult`. `SipPolynomial::jacobian` is the analytic derivative, and the area a drop is magnified by is the closed-form Jacobian determinant everywhere (`local_jacobian`'s forward differences are gone).
- **The drizzle output grid puts the reference footprint on its own (W3).** Reference pixel `p` maps to `s·p + (s − 1)/2`, so `[−½, w − ½]` lands on `[−½, s·w − ½]`; a bare `s·p` lost a strip of `(s − 1)/2` output pixels at the top and left of every frame and half-covered the last row and column. At scale 2 each input pixel now owns its 2×2 block of output cells — so the hand-derived scale-2 tests changed: a lone pixel fills its block undiluted instead of half-sharing cells with its neighbours, the point kernel's centre is the block's corner and rounds onto the odd cell, and the mixing test shifts a quarter pixel to make two pixels share a cell. The Lanczos output is no longer clamped at zero: a background-subtracted frame keeps its negative pixels, tested on a constant −0.5.
- **Drizzle's input-row bound is exact without a Jacobian (W3).** `quad_row_extent` estimated the square kernel's reach in output rows from the linear part. The square drop is a box in the *input*, so `input_rows` widens the output band by the reach in output rows (the compact kernels' radius and the rounding slack) and the resulting input rows by the reach in input rows (the square kernel's half drop) — no Jacobian needed, for any transform. A SIP band's outline is sampled every 8 output pixels plus one input row for the bend between samples. The band scans are computed per frame before the scatter; the coverage bitsets live on the accumulator, cleared per band per frame instead of allocated, and the radial kernels lease their scratch from a pool. The per-frame `Vec` of ~64 band headers stays: one small allocation against an output-sized scatter.
- **Drizzle's radial kernels are evaluated per axis (W3).** Gaussian and Lanczos are both separable, so a drop evaluates `2·(2r + 1)` one-axis values instead of `(2r + 1)²` two-axis ones, and the normalizer is the product of the two axis sums. The review asked for the warp's Lanczos table instead; the table quantizes the distance to 2.4e-4 and the separable form keeps the exact kernel at a seventh of the evaluations, so exactness won. `LANCZOS_A` stays as the name of `STScI`'s Lanczos-3, the one drizzle Lanczos kernel.
- **A drizzle drop's weight is relative to the output grid (W3).** The weight was divided by the whole magnification of input → output, the grid's `s²` included. So Turbo, Point and the radial kernels deposited `w/s²` for an unmagnified input pixel, the square kernel's clipped quadrilateral deposited `w`, and the scale of the exported weight plane changed with the kernel. The weight now divides by the warp's own magnification only. An unmagnified drop deposits its frame weight under every kernel, the weight per output pixel of a frame magnified by `M` still falls as `1/M`, and Turbo and Square give the same weight plane while a drop stays axis-aligned. The image does not change: the factor was common to all frames of a run.
- **Resample and drizzle tests assert exact values or derived bounds (W3).** `InterpolationMethod::ALL` and `DrizzleKernel::ALL` replace the copies of the method and kernel lists. The kernel tests probe the Lanczos table between entries against `max|L′|·(½·step + ulp)`, a column of ones gives every interpolation method a hand value through the row warp, and integer shifts copy the source to a bound derived from the table's residue at integers. `warp_into` is compared bit for bit with a fresh `warp` from NaN-filled and reused buffers, and an RGB warp with three mono warps. The drizzle tests are property sweeps over every kernel: a constant reads back to a rounding bound behind the `min_weight_fraction` gate, a magnified frame weighs `¼` (the Gaussian's closed form included), a linear ramp reads back at each preimage under a non-unit Jacobian, each kernel's footprint for one excluded pixel is counted exactly, and the dithered renders check flux, the star's position `s·p + (s − 1)/2` and the quality maps' closed form `Σw = 4`, `Σw²/(Σw)² = 9/64`.
- **RANSAC's adaptive stop counts uniform iterations only (W3).** The sampling phases were fractions of `max_iterations` — the top quarter by confidence for the first third, the top half for the second — while the adaptive bound, computed for uniform sampling of the whole set, stopped the run long before the uniform phase. A larger consensus outside the top quarter was then never drawn: twenty plausible pairs at the front of an equal-confidence list beat thirty consistent ones behind them. The two guided phases now last `adaptive_iterations(0.5, m, confidence)` each — enough for a pool at least half inliers — and the bound counts only the uniform iterations after them, so the stop's guarantee holds over every pair. `ransac_loop` derives `n` and the sample size from its inputs.
- **Registration tests grade against the truth that made the data (W3).** Star-catalog tests are scenario tables: the matched pairs are exactly the true ones — no spurious star, no star whose image left the frame — and the fit deviates from the truth over the box the stars span by no more than rounding (1e-10 px) on exact positions, or five least-squares sigmas, `5·σ·√(2h/n)` with leverage `h ≤ 7` (21 for a homography), under noise. Rendered-image tests draw both frames from one scene through the camera model and take σ from the detections against the render's own truth. Recovery, RANSAC, MAGSAC, triangle voting, the k-d tree (against a brute-force scan), SIP (one shared `testing::synthetic::distortion::RadialField`) and TPS assert exact counts and closed forms; every remaining tolerance states its derivation.
- **Detection reads one residual plane (W4).** `StarDetector::detect` marks saturation on the prepared plane, then `BackgroundEstimate::subtract_from` turns it into `pixels − background` in place and hands on a `SkyNoise { noise, floor }`; the background plane goes back to the pool. Threshold, labeling, both deblenders, FWHM estimation and measurement read only the residual. The residual reuses the prepared plane, so detection needs no extra plane: `DETECTION_WORKING_PLANES` is 7 (residual, sky noise, the matched filter's output and pass scratch, the label map, saturation and threshold masks), pinned as the maximum over every preset in `mem_budget`.
- **Saturation is a mask, not a peak value (W4).** `SATURATION_FRACTION` of `ImageMetadata::data_max` (1.0 when the format records none) marks pixels before the sky is removed; `Star::saturated` is that mask at the region's peak, and `Star::peak` is the peak above the sky.
- **The multi-threshold ladder starts at the detection threshold (W4).** Its floor is `sigma_threshold · σ` at the component's peak, SExtractor's `DETECT_THRESH`, not the component's faintest pixel: a matched-filter detection can include residual pixels at or below zero, which made the old floor degenerate. The root is the whole component with the flux above that floor; every level from the floor up may split it, so a component that falls apart at the floor is not one root per piece.
- **Peaks are kept brightest first (W4).** Local maxima collect every candidate above the prominence bar into pooled scratch, rank them, and keep each one at least `deblend_min_separation` from every brighter one kept (greedy non-maximum suppression, as photutils `find_peaks` and scikit-image `peak_local_max` do). The multi-threshold tree ranks a split's regions by flux before the separation check, and its significant leaves by flux. Both split from one `Component` view (component, residual, labels, peak); the region-connectivity walk follows `DetectionConfig::connectivity`.
- **Labeling collects the components from its runs (W4).** `Labeler` holds the union-find, the strip run lists and the relabel map between frames and fills each component's box and area from the runs it writes, so `collect_component_data` and its per-job dense arrays are gone. `ComponentData` moved to `labeling`. Dilation is row-parallel in place with one pooled scratch mask: a horizontal word pass, then each row ORs its `2r + 1` neighbours.
- **The Gaussian fit is elliptical (W4).** `Gaussian2D` fits `[x0, y0, A, a, b, c, B]`, the inverse covariance, held to `a, c ∈ [1/R², 4]` and `b² ≤ (1 − 10⁻⁴)·a·c`; a width or cross term pinned at a bound is rejected like a centre that wandered. FWHM and eccentricity come from `Cov2::fwhm`/`eccentricity` for the moments and the fit alike, the FWHM being `equal_area_fwhm(det) = sigma_to_fwhm(det^¼)`. The fits run to full convergence: `position_convergence_threshold` is gone. A round star converges in 4 iterations against 5 for a 3.5 × 2.0 one at 45°; the test asserts round ≤ elongated rather than equal.
- **A weighted fit reweights from its model once (W4).** With a noise model, `StampFit::fit` fits with weights from the observed pixels, then refits from that result with weights from the model's values: one pass of iteratively reweighted least squares, which removes the Neyman χ² bias of data weights.
- **FWHM estimation measures twice (W4).** `FwhmMode::Auto { fallback }` measures the bright stars at the fallback, then again at the first estimate; stars from 5 to 10 px against a 4 px seed come back within 0.02%. The measure stage takes the FWHM as an `Option` and measures an unknown one at `UNKNOWN_FWHM` (the smallest stamp and window).
- **Every convolution backend is unfused and takes every tap (W4).** AVX2 (no longer `avx2,fma`), SSE4.1, NEON and scalar accumulate in one order with separate multiply and add, so each is bit-identical to the scalar path; the NEON row kernel has the x86 structure. The separable path handles any kernel radius, so the direct 2D fallback is gone.
- **A deblend node splits once; labels follow raster order (W4).** Multi-threshold deblending records a node's children at the first level that splits it and never overwrites them, so a deeper split cannot orphan the first one's leaves. `Labeler` numbers components in raster order of their first run, so labels match for any strip count; tests inject the strip count.
- **A profile fit pinned at a bound is rejected (W4).** The amplitude floor is `1e-6` of the stamp's amplitude seed, not an absolute 0.01, and a fit that ends on any bound — amplitude, width, cross term — is rejected like one that leaves its stamp. `measure_star`'s moments phase is `moments_centroid`, and the sky annulus's outer radius is `annulus_outer_radius`, one definition each; the Moffat FWHM conversions live once, in `math::fwhm`.
- **A plane sky comes back exact (W4).** Tile centres are the mean index of the pixels a tile holds, `(start + end − 1)/2`. A tile past `MAX_TILE_SAMPLES` takes point-symmetric ordinals (`sample_ordinal`), so its samples centre on the tile centre. The 3×3 tile median reads the grid point-reflected through the edge tile (`2·v(edge) − v(mirror)`), so an edge tile on a gradient is not pulled toward the interior as SExtractor's cut window pulls it; one spoiled tile still loses the vote, 4 of 9 at a corner. The spline continues its end intervals past the outer tile centres on all four sides, as SEP does, and its linear part is a rise from `f0`, so a constant sky is bit-exact. The tile σ still counts the sky's own spread across a tile — an open issue in `ISSUES.md`.
- **Every component is deblended; the area bounds apply to its regions (W4).** `max_area` used to drop a whole connected component before deblending, and a component split into at most `MAX_PEAKS` = 8 regions, so a crowded group lost every star past its eighth, or all of them past 500 px. The bounds now apply to the regions a deblender makes, as `DetectionConfig::max_area` documents, and peaks are unbounded (SExtractor allows 1024 sub-objects, photutils no cap), in pooled `DeblendBuffers`. To keep a giant component affordable: the multi-threshold tree groups a level's regions by the node they grew from in one pass (O(pixels) per level, where each region used to rescan the level); a node's children are one run of the tree (`Range<u32>`); `Component::split_at` finds a pixel's nearest peak through a cell grid past 8 peaks; local maxima are suppressed through an occupancy window of the box. On the 6k globular-cluster bench, detection finds 21,686 candidates against 12,927, at 1.70 s against 0.31 s — the multi-threshold tree over the cluster core that was dropped before; the local-maxima deblend benches take 24–26% longer for the peaks past eight they now keep.
- **The pipeline tests hold the detector to what the truth decides (W4).** The aspirational per-scenario rates are gone. A star alone on an island — no other source within 3 FWHM, the noiseless image the threshold sees 2σ under it on that ring, clear of the edge margin — is found within 1 px when its peak is `2·(k + 5)`σ, and rejected when saturated; at 4σ or more nothing is found past 2 FWHM from every source. The stage tests count candidates exactly around isolated sources, the cosmic-ray test isolates the sharpness cut by opening the others, and `FrameTruth` records where a scenario's cosmic rays landed.
- **The 3×3 median's six-value network sorts (W4).** `median6` held a 13-comparator network that left the middle pair wrong in 416 of the 720 orders, so every non-corner border pixel of the CFA median filter could take a wrong value; it is the optimal 12-comparator six-sorter now, tested over every order and every 0/1 pattern, and the filter is held to a sorted reference at every pixel of fields whose rows all differ.
- **One star field (W4).** `Scenario::default()` renders exactly `fixtures::star_field` — flux (5, 14), sky 0.1, margin 16, FWHM 4, the realistic camera — through shared `STAR_FIELD_*` constants. The field's (6, 16) flux saturated its brightest star, which its documentation denied; the characterization pins moved with it, decode pins included, so `DECODE_VERSION` changed for a fixture and not a decoder.
- **Image-op numerics hold their exact forms (W5).** HDR takes its residual mean through `sum::mean_f32` (a sequential f32 fold over 262 144 samples drifts 1.6%). GHS evaluates `T` and `T′` through `ln_1p`/`expm1`, special-cased only at the exact limits `b = 0` and `b = −1`, and is held to an f64 textbook curve within 64ε through both. CLAHE maps a value linearly between its tile's CDF edges — a flat histogram is the identity, and distinct values stay distinct — spreads the clip remainder across the range at OpenCV's stride, and splits each axis evenly so no tile is empty, blending at the tiles' true centres.
- **One `asinh`, accurate at every magnitude (W5).** The vector backends compute `log1p(x + x²/(1 + √(1 + x²)))` with Goldberg's `ln(1 + u)·u/((1 + u) − 1)` correction, and `ln x + ln 2` past 2¹², where the textbook `ln(x + √(x² + 1))` lost all relative accuracy as `x → 0`. Mono and per-channel stretches run the same vector kernel through `ToneCurve::eval_block` instead of libm per sample. One `Rgb::with_intensity` moves a pixel to a new intensity hue-preservingly — black at no positive intensity, divided by its largest channel past white, negatives clamped — for the color-preserving stretch and `LinearImage::remap_intensity` alike.
- **One subsample, one spline segment, one median/MAD (W5).** `math::statistics::subsample::Subsample` takes `⌈len / cap⌉`-strided indices, never more than `MAX_STATISTIC_SAMPLES` (one cap for every image statistic, where four claimed to match and did not); `background_mesh::spline::spline_segment::SplineSegment` is the spline interval the mesh and its SIMD kernels both evaluate; denoise, frame statistics and the defect residuals take their median and MAD through `MedianMad::of_mut`, frame statistics with one copy per channel.

- **Image-op tests decided by exact values (W5).** Dyadic fixtures make the outputs exact where the arithmetic allows: a dyadic plane's tile skies are the plane at the tile centres, so gradient removal leaves exactly its mean level, and the outlier-tile rejection recovers the plane exactly; the hue tests compare `R = 2G` with `assert_eq!`. Where rounding is real, each bound states its roundings (`mtf_bound`, `ASINH_EVAL_BOUND`, `GHS_EVAL_BOUND`, the solver's Gershgorin conditioning). The rewrite found two wrong hand values that a 1e-4 tolerance had hidden (`f(0.25)` of the b = 0 GHS is 0.455054, `asinh(10)/asinh(100)` is 0.565879). The per-op `rejects_*` tests are rows of one table in `image_ops/error.rs`; a row sets the op's zero-strength shortcut, so validation must run before it.
- **Test fixtures in one place (W5).** `testing::real_data::{linear_master, display_master}` load the bundled stack and its display-domain form for every real-data test and bench (five copies of the load, three of the prep). `testing::synthetic::patterns::{horizontal_gradient, linear_rgb_master}` replace the gallery's private gradient, CLAHE's copy, and the two synthetic linear masters of the stretch bench and the memory probe.
- **The mesh's tile statistics without the splines (W5).** `MeshWorkspace::tile_stats` fills the tiles only; `compute` adds the y-spline derivatives. Gradient extraction reads tiles and never interpolates, so it no longer solves the splines, and it takes its clip passes from `DEFAULT_SIGMA_CLIP_ITERATIONS`, the detector's default, instead of a restated 3. `tile_starts` lost a `dedup` that could never fire (only the window that reaches the edge is clamped, and the loop ends there).
- **The pipeline checks what it is given before it works on it (W6).** `AlignStackConfig::validate` takes the light count: manual weights must be one per input light, and a run with the wrong count fails before any decode. After registration drops frames, `StackConfig::for_survivors` hands the combine the weights of the frames left, so a drop no longer ends the run in `ManualWeightCountMismatch` or pairs weights with the wrong frames. `align_and_stack` checks every light's dimensions and samples at entry; `calibrate_align_stack` checks each light's dimensions against the peeked header as it decodes, so a frame from another sensor fails before the rest are detected. `register_warp_and_stack` keeps the dimension rule as a `debug_assert!`. The already-decoded entry bounds its detectors by `MemoryPlan::decode_concurrency`, as the RAW entry does.
- **Cosmic-ray settings are validated, and the parametric scale comes from the frame (W6).** `CosmicRayConfig::validate` joins `AlignStackConfig::validate`: positive `sigclip`/`objlim`, `sigfrac` in (0, 1], `niter ≥ 1`, positive gain, non-negative read noise. `NoiseEstimation::Parametric` lost `full_scale`: the ADU one sample unit is worth is `(1/√12)/quantization_sigma`, which the decoder records; a frame without one (a float FITS) fails with `Error::CosmicRay { path, UnknownAdcStep }` rather than taking a hand-typed span. `CfaType` has no unlabeled variant since W1, so the "unlabeled is mono" branch no longer exists.
- **The ML tile stride is validated (W6).** `TiledOnnxConfig::run` refuses a stride outside `1..=512` with `MlError::InvalidConfig`: zero never advanced (a release `assert!` on a `pub` field), and past the window the uncovered bands came out `0/0`. The unused `MlDenoise::stride` / `RemoveStars::stride` setters are gone. The decode helper takes the cosmic-ray config alone, the register stage takes one `StagePlan` (tier and warp concurrency from one plan), and the detection funnel moves into the result instead of being cloned.
- **One Global estimator, its window keeping the stars (W7, D10).** `Normalization::Global` is the paired errors-in-variables fit for every frame set; coverage only decides where the statistics come from (every pixel, or the common domain). The fit is seeded by the median ratio `(y − m_y)/(x − m_x)` over pairs 5σ above their medians on both sides (16 or more; otherwise the ratio of sky spreads), and its window is each pair's expected residual `4·√(σ²_sky + (scatter·(x − m_x))²)` with `scatter` the lever-arm ratios' 1.4826·MAD, so a star stays in unless its ratio is out of line with the others'. Two Deming fits, the second re-centred on the first's gain. The old window was sky σ alone, which cut every star once the seed was a few percent off; the old unregistered estimator, the ratio of sky spreads, misreads the gain whenever two frames' noise differs, which a test with known gains on a star field now pins (and one blank pixel no longer switches estimators). Every plane is read once: the pass that gathers a frame's common-domain median takes its stratified samples. Normalization measures medians only.
- **The cache owns its norms (W7).** `FrameCache::frame_norms` is read by `process_chunked` itself, `FrameCache::normalization` and the assert that compared it with the config are gone, `FrameCacheParams` became `CacheCore`, and `CacheCore::process_chunks` takes `&[StoredFrame]` instead of a generic accessor. `run_stacking`'s mean arms share one reducer. Stored frames the pipeline produced have their geometry, samples and quality pair asserted in debug builds only; the facts their sources stated are still checked in release.
- **Masters per role, presets on the role (W7, D8).** `MasterRole::stack_config` is the one role → preset table; lens and the examples stack each role through `stack_cfa_master` and assemble with `from_images`. `CalibrationMasters::from_files`, `RoleStack`, `frames_fit_in_memory`, `RunMemory::share` and `CalibrationSet::from_roles` are gone. `from_images` and `DefectMap::detect_*` return `CalibrationError` (with a `Cancelled` variant), so the combine `Error` lost its `Calibration` variant.
- **`DefectMap` knows its sensor and keeps its mask (W7, D9).** `DefectMap::new(dimensions)`; the index lists are read through accessors; the mask of every defect is built when the lists change, not per light; `count` and `percentage` count a pixel both hot and dead once. A bundle's defect table needs its dimensions and in-range indices, no longer a sort order nothing read. A bundle that fails validation on load carries its `CalibrationError` inside the `io::Error` instead of its text.
- **Pipeline failures on one path (W7).** The pipeline `Error` has `Cancelled` and `FrameStore`, and `From<StackError>` maps the combine's cancel, empty set and frame-store failure onto them. RANSAC reports `TooFewMatches` when it never ran, and otherwise the iterations it ran and the most inliers it found, instead of "no inliers after the full budget" for both.
- **Public surface without test-only API (W7).** Gone: the image ops' builder setters (their fields are `pub`, and lens sets them), `Stretch::ghs`, `Transform::apply_inverse`, `InterpolationMethod::kernel_radius`, `RegistrationResult::{max_error, sip_fit}` (the result now carries the SIP polynomial alone; `SipFitResult` keeps its diagnostics as the fitter's own result), `TransformType`'s explicit discriminants, the `DMat3` `Default`, `Mul` and array conversions, `BitBuffer2`'s `(x, y)` index, `Vec<bool>` conversions and bit iterator, `Vec2us::to_index`, `LinearImage::mean`, `LinearPixels::mean`, `SubAssign<&LinearImage>`, `StarDetector::config`, `FwhmSource::was_estimated`. Accessors only tests read (`Coverage`'s indexing and `per_pixel`, `QualityMap::channel`, the SIP residual and grid-correction helpers, `BitBuffer2::iter`) moved behind `cfg(test)`.
- **Stored values that were derived, and per-pixel divisions (W7).** `BitBuffer2` no longer stores its length and keeps its stride private. Per-pixel null lookups walk rows with `(x, y)` instead of dividing a flat index (`NullMask::is_null_at`), the cosmic-ray primary mask clears the accumulated defects a word at a time (`and_not`), and the FITS writer marks nulls by visiting the set bits. `URect::new` checks `min ≤ max` in debug builds only; `URect::empty` is documented as `include`'s identity. `JobScratchLease` holds its value in a `ManuallyDrop` instead of an always-`Some` `Option`.
- **One mesh axis rule (W7).** `background_mesh::mesh_axis::MeshAxis` — SExtractor's `BACK_SIZE` tiles from the origin with a short last tile, centre `(start + end − 1)/2` — cuts both the background mesh and the dark-current model the hot-pixel detector subtracts, which had its own balanced partition and centres; its per-tile sample buffers are leased from a `JobScratchPool`. The real-data hot-pixel pins moved by a handful of pixels (51 082 → 51 074 on the first half of the darks).
- **Linear-fit clipping centres its positions (W7).** The slope is `Σ(x − x̄)·y / (n(n² − 1)/12)` in f64 and the fit is evaluated as `mean + b·(x − x̄)`: no position sums, no `n·Σx² − (Σx)²` cancellation, no guard that could never fire. A 4001-frame ramp with one outlier is pinned; on it, and on a 20 001-frame one, the old f32 sums rejected the same frames, so the change is conditioning rather than an observed result.
- **Helpers became methods (W7).** `FrameCheck { index, cancel }` holds the per-frame checks (`samples`, `stored`, `stored_quality`, `stored_samples`, `quality_pair`) that were free functions in `validation.rs`; `FrameNorm::measure`, `StoredPlane::write`, and `FrameCache`'s private `quality_chunks`, `weighted_layout` and `coverage_layout` replace their free-function forms.
- **A degenerate affine refit no longer panics (W7).** A minimal sample a hair off collinear passed the normal equations' determinant floor yet fit coefficients too large to normalize, and `Transform::compose` panicked; the real-data `weighted_fit_registration_rms` hit it. `Transform::try_compose` reports that as `None` and the affine estimate drops the sample.
- **The pipeline's memory plan counts what its passes hold (W7).** A warp worker loaded its source and measured its statistics beside the warp buffers it kept, a frame more than the plan charged, so ten 24 MP X-Trans lights at a 4096 MiB override peaked at 4274 MiB. Statistics are now measured in the preparing pass, on the frame as decoded and before it spills (`DetectedFrame::stats`), and the plan charges each stage what it holds: a decode its peak — the demosaic, the cosmic-ray pass (`cosmic_ray::heap_bytes`, pinned against each detector's allocations) or the statistics' copy of every channel, whichever is largest — plus the detector's scratch beside it (`RunShape::detection_bytes`); a spilled warp its source and warped output; a resident one only its source, since its warped output is its share of the resident set. The same run now peaks at 3174 MiB. The probes run the real stages: `pipeline_budget_probe` stacks masters with `stack_cfa_master` and lights with `calibrate_align_stack`, and `raw_lights_memory_probe` (the old `mem_probe` example) runs the dataset's RAW lights under a budget.
- **The lumos tests decide by exact values (W7).** The test review's lumos items are closed. Rejection tests assert the exact survivor set of each fixture, derived by hand from the median, MAD and fit (a float32 reference was the check on the derivations, not the oracle); the winsorized σ is pinned as the corrected standard deviation and as its Huber fixed point. Normalization tests moved out of the stack tests and normalize dyadic frames exactly; the Deming slope is pinned at two noise ratios; registered frames are normalized against frames that differ in gain and in noise. Weight, coverage, variance and quantization figures are exact where the arithmetic is, and otherwise carry a counted rounding bound. Fixtures share one validated cache builder (`FrameCache::from_images` over `CacheCore::plain`), one harness bit helper, one vignette, and `TestRng`. The GESD rows of the end-to-end tests stack 15 frames, so GESD runs instead of the median fallback.
- **A skipped producer keys its consumer as unbound (W9).** `RuntimeCache::node_digest` takes the run's node states and folds a `Bind` from a producer that does not run — disabled, or missing a required input — as `InputTag::Unbound`, the value the executor delivers there; it folded the producer's last stamp, so a consumer cached while the producer ran was served after it was disabled. `FORMAT_VERSION` is 10, so blobs stored under the stale keys miss.
- **The worker reports three things (W9).** `WorkerReport::{Activity(WorkerActivity), Progress { node_id, phase }, Completed(Arc<RunSummary>)}` replace the one `WorkerStatus` record whose fields meant different things per kind. Progress is by value; `RunPhase` is `Started`, `Succeeded` or `Failed`, so a failed node is never painted executed. The summary is published into one retained allocation, reused when the host dropped the last one. `NodeExecutionStatus::Running` is gone (live state is `RunPhase`), and the unread elapsed times on `Errored` and on the completed run went with the old record.
