# Structural redesign plan

Sources: `.notes/ISSUES.md`, `.notes/workspace-review.md`, `.notes/tests-review.md`.
Out of scope: `palantir`, and the findings in `lumos/.notes/*.md` (this plan only avoids conflicts with them).

Rules for the work:

- Delete a review item when its fix lands, as each review file asks. Delete a heading when its group is empty.
- Every bug noticed outside the current step goes to `.notes/ISSUES.md`.
- No commit until you say so. Each step ends with the verification chain for the crates it touched.
- Decisions are marked **D1**…**D13**. All are made (section 2).

---

## 0. Verification of the findings

The submodule update changed no reviewed code. `fits-well`, `imaginarium` and `quickbench` have no code commits after 2026-09-24, and the reviews are from 2026-10-02. `palantir` moved, but it is out of scope. `quickbench` has only lockfile changes in the working tree.

| Check | Scope | Result |
|---|---|---|
| Every cited path exists | 841 items, both reviews | All resolve. 22 short or generated paths (`port_row.rs`, `stacked_light.tiff`, proposed files) resolve by suffix or are outputs. |
| Every cited symbol exists | same | All exist. The 39 misses are external names (wcslib `celset`, LibRaw `color3_image`, `ManuallyDrop`) or name fragments (`_50_percent`). |
| Cited line is near its named symbol | 86 `path:line (name)` pairs | 71 within 8 lines. |
| Cited line is inside its file | 2663 line numbers | `tests-review.md` has wrong line numbers in some items, for example `math/fwhm.rs:78` (test is at 34), `wavelet/tests.rs:208` (at 63), `fwhm/tests.rs:240-339` (at 65-143, the file has 218 lines and never had more than 234). The content of these items is correct. Use the symbol names, not the line numbers. |
| Content, by reading code | all 6 `ISSUES.md` bugs, all 20 High groups, about 60 Medium items, 20 Low items, 20 test items | All confirmed, with the notes below. |
| Content, by running probes | fits-well WCS (5 claims), compressed table round trip | All confirmed. |

Notes from the content check:

- **Compressed tables (High):** the read fails only when a tile compresses to fewer bytes than the `TDIMn` product. Constant data fails with `KeywordOutOfRange { name: "TDIMn" }`. Incompressible data reads back. The bug is real, but it depends on the data.
- **`CalibrationMasters::from_files`:** the two examples `lumos/examples/full_pipeline.rs` and `mem_probe.rs` also call it. The finding lists only tests and benches.
- **WCS pole (`CelestialPole::from_fiducial`):** the probe gives the same pole for `LATPOLE = 90` and `-90`, which agrees with the finding. The probe does not show the pole value itself.
- **Stretch solver numbers** (`solve_asinh_beta`, `StfCurve::new`): the code shape is confirmed. The quoted output values were not re-run.

---

## 1. Phase 0 — build and lint baseline

These two items come first because every later step builds and lints under them.

### 1.1 One dev-profile rule for dependency optimization

Replace the 30 `[profile.dev.package.<name>]` blocks in the root `Cargo.toml` with:

```toml
[profile.dev.package."*"]
opt-level = 3
```

Cargo applies `"*"` to every package that is not a workspace member. It does not apply to build scripts and proc-macros, which use `[profile.dev.build-override]`, so `syn` and the derive crates keep their fast build. This is the form the Cargo book documents and the form Bevy recommends.

The excluded submodules (`palantir`, `imaginarium`, `fits-well`, `quickbench`) are not workspace members, so `"*"` also covers them. Today only `fits-well` is optimized. Per **D1**, `palantir`, `imaginarium` and `quickbench` stay at `opt-level = 0` through explicit `[profile.dev.package.<name>] opt-level = 0` entries, which take precedence over `"*"`. That keeps today's behaviour.

Verification: `cargo build` (darkroom), then `cargo build -p lumos --tests --features ml,internals`. Record the clean-build time before and after.

### 1.2 Lint set

Measured with clippy 0.1.99 on all eight crates (`--all-targets --all-features`, lints passed on the command line, no manifest change). Counts are distinct sites.

**Level policy:** every lint is `warn`, not `deny`. The verification chain runs clippy with `-D warnings`, so a warning fails the chain, but a build in the middle of an edit still works. The current workspace `deny` entries change to `warn`.

The submodules have their own `[lints]` tables and must not inherit from the workspace. The same set goes into `imaginarium`, `fits-well` and `quickbench` `Cargo.toml` by copy.

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
| `unsafe_code` | 323 | **not workspace-wide** | SIMD needs `unsafe` (`lumos` 169, `imaginarium` 122, `fits-well` 23). `darkroom` has 9, all in `alloc_audit.rs` (a `GlobalAlloc`). Put `#![forbid(unsafe_code)]` in `common`, `scenarium`, `lens` and `quickbench` (0 hits), and `#![deny(unsafe_code)]` in `darkroom` with an `allow` and a reason on `alloc_audit`. |
| `clippy::pedantic` (group) | ≈ 4 300 | adopt, with the allow-list below | |
| `clippy::print_stdout` / `print_stderr` | 325 / 53 | adopt | Every `lumos` hit is in test, bench, probe or example code. Set `allow-print-in-tests = true` in `clippy.toml`, and allow the lint at the top of each example and in `quickbench`'s report printer. |
| `clippy::absolute_paths` | 766 | adopt | Matches the rules "no inline paths" and "free functions stay namespace-qualified" (`use std::fs; fs::create_dir_all`). The one exception in the rules (a gated inline statement in place of a cfg'd import) gets a local `allow` with a reason. |
| `clippy::clone_on_ref_ptr` | 0 | keep | Already on. |
| `clippy::dbg_macro`, `clippy::todo` | 0 | adopt | Guards. |
| `clippy::let_underscore_must_use` | 36 | adopt | `let _ = fs::remove_file(..)` swallows an error. Deliberate best-effort cleanup in `Drop` gets `#[expect(.., reason = "..")]`. |
| `clippy::unused_result_ok` | 11 | adopt | Same class (`.ok();` to discard). |
| `clippy::map_err_ignore` | 59 | adopt | `map_err(\|_\| ..)` drops the source. Keep the source, or allow with a reason where the source carries nothing (`TryFromIntError`). |
| `clippy::self_named_module_files` | 0 | adopt | Guard for the rule "never `foo.rs` beside `foo/`". |
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

Kept from pedantic, with notes: `cast_lossless` (478, autofix to `f64::from`), `uninlined_format_args` (462, autofix), `doc_markdown` (296, with a `doc-valid-idents` list in `clippy.toml` for `LibRaw`, `PixInsight`, `wcslib`, …), `unreadable_literal` (280, autofix), `ignore_without_reason` (119, every `#[ignore]` states why), `return_self_not_must_use` (85, catches a dropped builder), `ptr_as_ptr` (70, `.cast()`), `wildcard_imports` (40, with `allowed-wildcard-imports` for `std::arch::*`), `manual_midpoint`, `stable_sort_primitive`, `trivially_copy_pass_by_ref`, `needless_pass_by_value` and the rest of the small ones.

Per **D2**, `cast_sign_loss` (308) and `cast_possible_wrap` (348) are on. Both flag `imaginarium/src/drawing.rs:49`, which is the `draw_circle` wrap bug in the workspace review.

Order of work for 1.2: (1) add the lints with every new one at `allow`; (2) enable them one at a time, autofix first, then hand fixes, one crate per chain run; (3) end with the full set at `warn`. A lint that needs a hand fix inside code that a later workstream rewrites waits for that workstream and is listed there.

---

## 2. Decisions

All decisions were made on 2026-10-02.

| ID | Question | Decision | Effect on the plan |
|---|---|---|---|
| D1 | Which submodules stay at `opt-level = 0` in dev? | `palantir`, `imaginarium`, `quickbench` | Same behaviour as today. 1.1 adds `[profile.dev.package.<name>] opt-level = 0` for those three. |
| D2 | `cast_sign_loss` and `cast_possible_wrap`? | Enable both | 1.2 enables them. About 650 sites get `try_from` or a local `allow` with a reason. |
| D3 | Thin-plate spline? | Keep as work in progress | `tps/` stays with its `cfg_attr(not(test), allow(dead_code))` and module note. W3 does not touch it. The TR item that asks to remove it is closed as "kept deliberately". |
| D4 | `imaginarium` GPU in this workspace? | Drop the `wgpu` feature from the workspace dependency | The GPU code stays in `imaginarium` with its fixes (W11). `scenarium`'s `ContextType` and the `&mut ContextStore` parameters go (W9). |
| D5 | `scenarium` wildcard outputs? | Keep, make cheap | The feature stays. Darkroom recomputes `OutputTypes` only when the graph changes. `OutputTypes::update` reads the const kind without a `ConstValue` clone (W9, W13). |
| D6 | Drizzle with a SIP registration? | Support it | Newton inversion of the SIP polynomial per point, with a stated tolerance (W3). |
| D7 | Palantir data types in `darkroom::core`? | Allow | The module doc names the allowed types. Core never imports `crate::gui` (W13). |
| D8 | Calibration-master construction? | lumos presets, lens per role | `MasterRole` owns the preset table. `from_files` and `RoleStack` go; the two examples call per role (W7, W10). |
| D9 | `lumos` public API with no non-test caller? | Keep public | `stack`, `stack_images`, `StackFrame`, `align_and_stack` and `DefectMap` stay `pub`. They get the same validation as the used paths: `FrameSet::validate` (W1), memory plan, finite-input and dimension checks (W6), and `DefectMap` without its `Option` placeholder (W7). |
| D10 | `Normalization::Global` estimator? | Paired photometric fit for every frame | W7 fixes the Deming inlier window first, then uses the paired fit over the common domain for all frames. |
| D11 | `lens` dev-dependencies `common/internals` and tokio `test-util`? | Approve both | W14 adds both to `lens/Cargo.toml` `[dev-dependencies]`. |
| D12 | `lens` processing-node port shape? | Preset picker plus optional `Config` | All six nodes take a preset and an optional `Config` port. A wired `Config` overrides the preset, and the node reports it (W10). |
| D13 | Graph panes? | Single pane | Delete the per-pane narration and the unreachable branch in `GraphUI::appearing` (W13). |

---|---|---|---|
| D1 | Which submodules stay at `opt-level = 0` in dev? | Only `palantir` (you edit and debug it most; `imaginarium` does pixel work where speed matters in dev). | 1.1 |
| D2 | `cast_sign_loss` and `cast_possible_wrap` on or off? | On. They caught a real bug in this tree. Cost: about 650 sites, most in SIMD index code. | 1.2 |
| D3 | Thin-plate spline: delete, integrate, or keep? | Delete (option A in `lumos/.notes/lumos-review.md`). SIP is the FITS standard and is not used yet with rotation either. | W3 |
| D4 | `imaginarium` GPU in this workspace: drop the `wgpu` feature from the workspace dependency, or wire the GPU stack into `lens`? | Drop it. Nothing uses it. The GPU code stays in `imaginarium` behind its own feature and gets its bug fixes there. `scenarium`'s `ContextType` then has no reason to exist and goes too. | W9, W12 |
| D5 | `scenarium` wildcard outputs: remove or keep? | Remove. No production func declares one, and darkroom pays for `OutputTypes::update` every frame. | W9, W14 |
| D6 | Drizzle with a SIP registration: support it, or refuse it with a typed error? | Support it, with a Newton inverse of the SIP polynomial per point (the method wcslib and astropy use when no inverse polynomial is stored). | W3 |
| D7 | `darkroom::core` and palantir: allow palantir data types (`DockState`, `DockOp`, `ImageFilter`) in core, or mirror them? | Allow data types, and say so in the module doc. Core still never imports `crate::gui`. | W14 |
| D8 | Calibration-master construction: who owns it? | `lumos` owns the role → preset table on `MasterRole`. `lens` keeps one call per role, because its cache is per role. Delete `CalibrationMasters::from_files` and `RoleStack`, and change the two examples. | W1, W11 |
| D9 | `lumos` public API with no non-test caller (`stack`, `stack_images`, `StackFrame`, `align_and_stack`, the `pub` methods of `DefectMap`): remove or keep? | Remove from the public surface. Tests reach them through `internals`. | W7 |
| D10 | `Normalization::Global`: which estimator for every frame? | The paired photometric fit, measured over the common domain, for all frames, after the Deming inlier window is fixed (`combine-review.md` §12). Siril and PixInsight ImageIntegration default to dispersion ratios; NSG uses photometry because dispersion is biased by gradients. Precision comes first in lumos. | W7 |
| D11 | Approve two `lens` dev-dependency changes: `common` with `internals`, and tokio `test-util`? | Approve. The first replaces three temp-dir schemes with `TempDir`; the second makes the debounce tests exact with paused time. | W15 |
| D12 | One port shape for `lens` processing nodes? | A preset picker plus an optional `Config` port for all six nodes. When `Config` is wired, the node reports the preset as overridden. | W11 |
| D13 | Graph panes: keep the single graph pane and delete the per-pane narration, or implement split view? | Single pane. Delete the narration and the unreachable `appearing` case. | W14 |

---

## 3. Workstreams

Each workstream lists the review groups it closes, the target design, the steps and the tests. "WR" is `workspace-review.md`, "TR" is `tests-review.md`.

### W1 — lumos: one sample domain, one frame validation

Root cause: the meaning of a sample (its domain, row order, CFA pattern) is a fact that several types restate, that the writer does not save, and that each entry point checks in its own way.

Closes: WR "A reloaded calibration master always fails the sample-domain check", "Frame validation differs by entry point…", "Four parallel enums describe one sensor fact…", "RAW and FITS loading misclassify or reject valid files", the `CfaImage`/`XTransImage`/`BitPix` items in "Placeholder values…".

Target design:

- `SampleDomain` is the persisted fact. The FITS writer saves `LUMSCALE` (the scale, f64) and `LUMDEC` (decoder kind). The reader restores `TransferProvenance::FitsNormalized { physical_scale: LUMSCALE }` when the keywords exist. A master that was stacked from RAW darks reloads with its RAW scale.
- One `FrameSet::validate(frames, dimensions, cancel)` runs in every `FrameCache` constructor, including `from_tiered_paths`. It checks geometry, sample domain, row order, CFA pattern and sample finiteness, in one order. Reused cache planes go through `validate_frame_quality` too. `CalibrationMasters::from_images` checks flat against flat-dark with the same function.
- `CfaType` is the only sensor-pattern type. `SensorType` becomes `Option<CfaType>` plus the LibRaw fallback for `filters == 0 && colors == 3`. `DemosaicKind` and `DemosaicProvenance` become methods of `CfaType`. `CfaImage` holds a `CfaType`, not `Option`. `BitPix` goes; `ImageMetadata` stores `fits_well::SampleType` where the source had one.
- Optional FITS keywords degrade to `None` on a type mismatch. Only `cfa_type`, row order and `QNTZSIG` can fail a load. `BAYERPAT = 'TRUE'` fails unless `FitsLoadOptions` gives a pattern override.
- `fits-well` exposes the shape and stored `Bitpix` of any image HDU, so lumos deletes `compressed_shape` and its HDU-selection copies (W13 first).

Tests: a RAW-sourced master saved, reloaded and used to calibrate a RAW light; a `uint16` and a `float32` FITS of the same ADU rejected by every entry point; a table over entry points × mismatch kinds.

### W2 — lumos: frame store, disk cache and memory plan

Root cause: the spill directory, the cache key, the sidecar names and the memory figure each have several owners.

Closes: WR "The frame spill directory deletes a directory the run did not create", "The disk frame cache keys, names and sidecars…", "Memory budgeting re-implemented at every entry point…", the null-mask half of "Frame validation…", "File-source identity is implemented three times…".

Target design:

- `SpillDirectory::create(root)` makes a new unique subdirectory `root/lumos-<pid>-<n>` with `create_dir` (not `_all`) and removes only that subdirectory. `keep_cache` keeps it. A user path is never removed.
- `CacheKey { source: FileIdentity, decoder: DecoderKind, decode_version: u32 }` names every cached plane. `FrameSpill` owns every file name, including sidecars, and uses `FramePlane`'s `Display` for `coverage`/`confidence`. One `cache_frame()` path serves frame 0 and the rest, so frame 0 gets its sidecars and the source-change check.
- `StoredImage` spills its null mask as a bit plane and restores it, so the spill tier warps with `MaskedWarp` like the RAM tier.
- `FileIdentity { len, mtime_ns: i128 }` moves to `common::file_utils`. `lumos`, `scenarium` and `lens` use it. `lens` computes `frame_set_key` only when its cache is on.
- `RunMemory { system: u64, user_override: Option<u64> }` is read once per run at the entry and passed down. `CacheConfig::available_memory` becomes `memory_override` and is never rewritten. `CacheCore` stores `chunk_memory: u64`, not a `OnceLock`. One tier rule (`MemoryPlan`) charges input frames and the resident output planes, for `load_tiered`, `frames_fit_in_memory` and the pipeline.

Tests: a `CacheConfig::with_cache_dir` pointed at a directory with a sentinel file, which survives the run; a RAW decoded as `LinearImage` then loaded as `CfaImage` with `keep_cache`, which must not reuse planes; a masked FITS light on the spill tier against the RAM tier, bit-exact.

### W3 — lumos: registration geometry

Root cause: `Transform` does not enforce its own normalization, the SIP model has no stated frame, and drizzle takes a bare transform with the opposite direction.

Closes: WR "SIP distortion is fitted in the target frame…", "Registration accepts a transform supported by 2–4 stars…", "`Transform` and drizzle disagree…", "Registration hot loops recompute work…", registration items in "Placeholder values…", "lumos stacking numerics…" and "The same constant or formula…"; `ISSUES.md` `recover_matches`.

Target design:

- `Transform::from_matrix` normalizes so that `m[8] = 1` and rejects a matrix with `m[8] ≈ 0`. Every constructor goes through it. The accessors (`rotation_angle`, `scale_factor`) then read a normalized matrix. The SIMD bilinear kernels keep `h·y + 1`, which is now correct by construction, and compute in f64 for the position, narrowing only the fraction.
- SIP follows the FITS convention: the correction applies in the reference pixel frame, before the linear part. The fit target becomes `L⁻¹·(t − T(r))`.
- `register` applies `min_matches` to the final inlier count and returns `RegistrationError::TooFewInliers` below it.
- RANSAC checks the adaptive bound on every iteration, not only on an improvement.
- `recover_matches` stops when the match *set* is unchanged, not the count, and always refits on the returned set.
- `DrizzleFrame` takes the registration's `WarpTransform` (reference → target) and maps input pixels with its inverse. Per **D6**, a SIP warp is inverted per point with Newton iteration to a stated tolerance. `quad_row_extent` uses the local Jacobian of the full transform.
- Non-linear warps evaluate the transform once per pixel into a row buffer and share it between channels, quality maps and validity. SIP evaluation uses incremental powers. Homographies use row stepping (numerators and denominator are affine in x).
- Per **D3**, the thin-plate spline stays as it is.

Tests: SIP fit and warp under a 10° and a 180° rotation, residual within the fit noise; a 3-star registration rejected; a homography stored with `m[8] = 2` against its normalized twin, bit-exact in SIMD and scalar; drizzle with a registration result, compared with the accumulator path bit for bit.

### W4 — lumos: star detection on a residual plane

Root cause: each stage decides for itself whether the sky is removed, and the configuration encodes modes as numeric sentinels.

Closes: WR "Star measurement rectifies sky noise into signal", "Deblending runs on sky-included pixel values", "FWHM auto-estimation measures on the minimum stamp", "The Gaussian profile fit is axis-aligned…", "Star-measurement fits and peak selection…", "Star-detection config holds variant fields…", "lumos star-detection deblender structure…", star-detection items in "SIMD backends…" and "Placeholder values…".

Target design:

- The detect stage computes the residual plane `pixels − background` once, in a pooled buffer. Threshold, labeling, both deblenders and measurement read the residual. No stage sees sky-included values.
- Flux, core flux, peak and SNR use the signed residual sum. Only the moment seed and centroid weights clip at zero.
- FWHM estimation seeds the stamp from `fwhm.expected` and repeats the measurement once at the radius the first estimate implies.
- The Gaussian fit gets a rotation parameter (`[x0, y0, amp, σ_major, σ_minor, θ, bg]`, the astropy `Gaussian2D` form). One FWHM definition lives in `math::fwhm`, used by the moments path and both fits.
- Config: `Deblend::{LocalMaxima { min_prominence }, MultiThreshold { n_thresholds, min_contrast }}`; `FwhmMode::{Fixed(f32), Auto { fallback: f32 }}`; `BackgroundRefinement::Iterative { mask_dilation }`; `Option` instead of `0` sentinels. `validate` names match the field names.
- Peak lists keep the brightest `MAX_PEAKS`, not the first in raster order. All per-frame buffers come from `JobScratchPool`.
- `SATURATION_PEAK` derives from `ImageMetadata::data_max`.

Tests: an empty-sky stamp with SNR ≈ 0 at r = 7, 13, 15; a star on a 0.1 sky against the same star on a 0 sky, same peaks and same split; FWHM 5…10 recovered within the fit noise; a 45° elongated star with eccentricity above the cut under `GaussianFit`.

### W5 — lumos: image-op numerics

Closes: WR "The default gradient removal puts both auto stretches on their degenerate branch", "Normalization and image ops lose precision…", image-op items in "lumos stacking numerics…", "SIMD backends…", "The same constant or formula…".

Target design:

- `BackgroundMode::Subtract` writes `p − m + mean(m)`, as Siril does (`remove_gradient` in `src/algos/background_extraction.c`). `Divide` already keeps the level.
- `solve_asinh_beta` and `StfCurve::new` return an error when the target is out of reach instead of the range limit.
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
- Lights are checked for non-finite samples at entry. Dimensions are checked at decode time. The detector pool uses `MemoryPlan`.
- The `cfa_type == None` policy for cosmic rays is one place: skip with a warning.

### W7 — lumos: combine and normalization ownership

Closes: WR "Normalization and combine: measured statistics thrown away…", "lumos error types route through each other…", "Calibration-master construction is duplicated…", lumos items in "Public API… with no production user" (the `lumos::lib.rs` item is closed as "kept" per **D9**).

Target design:

- Per **D10**, the paired photometric fit over the common domain is the only estimator. First fix the Deming inlier window (`combine-review.md` §12) so the stars with a photometric lever arm stay in the fit. Partial coverage then changes only where statistics are measured.
- Per **D8**, `MasterRole` owns the role → preset table. `CalibrationMasters::from_files` and `RoleStack` go.
- Per **D9**, `DefectMap` stays public: `dimensions` stops being an `Option`, and `correct` keeps its mask instead of rebuilding it per light.
- `FrameCache` owns `frame_norms`. `process_chunked` reads them from `self`. `run_stacking` takes the normalization from the config only.
- Normalization measures medians only (no unread MAD).
- Pipeline `Error` gets `Cancelled`, one `NoFrames`, `From<FrameStoreError>`. Calibration errors leave the combine `Error`.
- Code-contract checks on frames the pipeline produced become `debug_assert!`.

### W8 — common: typed ids and introspection

Closes: WR "Typed ids are parsed from strings at run time…", "File-source identity…" (with W2), common items in "Public API…" and "common, quickbench: small API…", "Missing `const fn`…" for `id_type!`.

Target design:

- `id_type!` gets `pub const fn literal(s: &str) -> Self`, built on `uuid::Uuid::try_parse`, which is `const fn` in uuid 1.26. A bad literal in a `const` item fails at compile time. `From<&str>` and `From<String>` go. All 57 string-literal ids in production become `const` items. `nil`, `is_nil`, `as_u128`, `as_uuid` become `const fn`.
- The two hand-typed ids in `lens/src/image/nodes/processing.rs` get new `uuidgen` values. This changes saved graphs that use those two nodes; the workspace rules allow it.
- `#[derive(Introspect)]` takes `#[config(type_id = "…")]` like `IntrospectEnum`. `lens`'s `NodeConfig` trait goes. Introspection metadata is `&'static str` and `&'static [..]`.
- `serde.rs`: one `serialize_into(&T, ..)` signature, a real scratch for Bitcode, `SerdeFormat::Lz4` and `deserialize_from` removed.

### W9 — scenarium: worker protocol and engine state

Closes: WR "A disabled producer's stale digest keys its consumer's cache", "scenarium's `WorkerStatus` folds three messages into one record", "scenarium threads and stores state for features nothing uses", "scenarium flattens errors to strings…", "scenarium's graph API makes hosts mirror its private rules", scenarium items in "Placeholder values…" and "Public API…".

Target design:

- The `Bind` arm of `node_digest` folds `InputTag::Unbound` for a producer that is not runnable, with the predicate `collect_inputs` uses.
- `WorkerReport::{Activity(WorkerActivity), Progress { node_id, phase }, Completed(RunSummary)}`. `RunPhase::Finished` carries the outcome, so a failed node is never painted executed. Progress events reuse a buffer; no `Arc` per event.
- Per **D4**, `ContextType`, `ContextStore` and the `&mut ContextStore` parameters go.
- Per **D5**, wildcard outputs stay. `OutputTypes::update` reads the const kind without a `ConstValue` clone and reuses its tables.
- `FuncLambda` and `EventLambda` are required constructor arguments. `NodeState::MissingLambda` and `RunError::MissingLambda` go.
- `DiskStore` holds the root only. Codecs move into `CompiledGraph`. Codec coverage is checked before any I/O, and the verdict is stored.
- `CompileError`, `RunError::Invoke` and `InvokeError::External` keep typed sources. `StampError::Io` carries the path.
- `Graph::find` and `Library::by_id` return `None` for a nil id. `DetachedNode` gets a fallible constructor that owns the rules `attach_node` checks. `Library::types` becomes private with an idempotent `register_type`.
- `ExecutionEngine::compiled` is a `CompiledGraph` (default empty), not an `Option`.
- `Invocation` gets a typed accessor for required inputs, so `lens` drops 37 `.expect` calls.

### W10 — lens: adapters

Closes: WR "lens re-states lumos's presets and defaults", "lens node lambdas repeat boilerplate…", "Pixel layout is converted and copied at crate boundaries", lens items in "Untrusted data…", "Public API…".

Target design:

- Per **D12**, all six processing nodes take a preset picker and an optional `Config` port. A wired `Config` overrides the preset, and the node reports that.
- `lens` uses the `lumos` enums directly (`StretchMethod`, `ScnrMethod`, `BackgroundMode`, `CombineMethod`). The four mirror enums, `preset_enum!` and the copied defaults go. Preset becomes a trait with an associated knob type and default methods.
- `register_blend` compares descriptors and returns `InvokeError`. `register_transform` rejects a zero scale.
- Stacking outputs leave planar (`Image::from(LinearImage)`). `AlignStackConfig.stack.quality` follows `demand`. The codec stores the layout it was given. `Image::take_or_copy` replaces the three copies of that match.
- The Build Masters "Sigma" description names the defect threshold. "Reference" becomes an optional input.

### W11 — imaginarium: one format type, one rounding rule

Closes: WR "Image conversion and SIMD kernels return wrong or CPU-dependent pixels", "GPU blend and contrast dispatch fails…", "imaginarium integer outputs truncate or round differently…", "imaginarium `ColorFormat` admits 18 combinations…", "imaginarium's GPU stack has no consumer…", "imaginarium plumbing…", imaginarium items in "Untrusted data…", "Public API…".

Target design:

- `ColorFormat` is a 9-variant enum with `channel_count()`, `channel_size()`, `channel_type()`. `validate()`, `ALL_FORMATS.contains` and the tuple `From` go. The dispatch tables match exhaustively, with no fallback arm.
- One `Sample` trait states full scale, widening and narrowing. Every narrowing rounds half to even, on CPU, SIMD and WGSL (WGSL `round` is half-to-even). u16 → u8 is `round(v·255/65535)`. Every SIMD tail calls the scalar reference. Float → int multiplies in f64, or in an f32 form proven equal by an exhaustive sweep.
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
- The `Header` forwarders go; `Wcs::from_header` and `FitsTime::from_header` are the one path. The writer has five operations, each with `header: Option<&Header>`.
- The ASCII float parser builds `"{mantissa}e{exp}"` and parses once (strtod parity).
- The examples take their file from an argument.

### W13 — darkroom: layering and editing

Closes: WR "Raising a node is never a no-op…", "Esc commits the text a user tried to cancel", "Undo coalescing has no gesture identity…", "`darkroom::core` claims to be frontend-free…", "Loaded documents and preferences…", "Canvas gestures…", "Canvas per-frame work…", "darkroom core keeps containers…", "darkroom GUI repeats per-frame lookups…"; `ISSUES.md` format carry.

Target design:

- Undo has gesture sessions. A pointer press opens a gesture with a new `GestureId`. The open entry stays decoded on `ActionStack`. Release, Esc or any other step seals it: a no-op entry is dropped, otherwise it is encoded once. Coalescing zips the latched member lists.
- `front_z` excludes the raised item, and a raise of the frontmost item emits no step.
- One `DraftEdit` widget helper returns `Commit(text)`, `Cancel` or `Editing`. The value editor, the preferences path field and the inline rename use it. `EditBuffer::blur_edge` becomes `blurred = latch && !focused; latch = focused`.
- Esc deselects only when no gesture was in flight. Group drag and preview drag honour cancel.
- `core::edit` owns `DocumentRequest` and the layout-impact flag. `gui::Requests` wraps the document queue and the app queue. `core` never imports `gui`.
- `Document::validate` checks that `Graph` is present and pinned and that the seed is `DOCK_SEED`. `Preferences::load` reports a parse error and does not overwrite the file.
- Per-frame costs: the palette keeps its row buffer and re-filters on a query change; the breaker keeps a scribble bounding box; `GraphCtx` recomputes `OutputTypes` only when the graph changes (**D5**).
- Per **D7**, the `core` module doc names the palantir data types it may use. Per **D13**, the per-pane narration and the unreachable branch in `GraphUI::appearing` go.
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
- **Golden bytes:** one node digest and one blob layout in scenarium.

The individual test items in TR (cannot-fail tests, loose tolerances, duplicates, missing tests) are done inside the workstream that owns the code under test, because they verify that workstream's changes.

### W15 — comments and docs

Closes: WR "Docs that describe code that no longer exists", TR "Stale, wrong and change-narrating comments". Done when a workstream touches the file, per the comment rules. A final pass covers the files no workstream touched.

---

## 4. Order

| Phase | Content | Depends on |
|---|---|---|
| 0 | 1.1 dev profile, 1.2 lints | — |
| 1 | W14 core (bench feature, real-data gate, `TempDir`, seeding, SIMD tier harness) | — |
| 2 | Local High fixes that need no redesign, each with its test: spill directory (W2 first step), raise no-op, `DraftEdit`, RCD step 4.0 start, signed flux, background pedestal, dedup 0, `draw_circle` clamp, `convert_to` copy, SIMD tails, the six `ISSUES.md` bugs | — |
| 3 | W8 (ids) — touches every crate, so it goes before the crate rewrites | — |
| 4 | W12 (fits-well), W11 (imaginarium) — independent submodules, can run beside phase 5 | — |
| 5 | W1 → W2 → W3 → W4 → W5 → W6 → W7 (lumos, in this order: domain before cache, cache before registration, geometry before detection) | — |
| 6 | W9 (scenarium) → W10 (lens) → W13 (darkroom) | — |
| 7 | W15 final pass; check every review file is empty | — |

Verification after each step: the chain for the touched crates (`cargo fmt -p <crate> && cargo clippy -p <crate> --all-targets --all-features -- -D warnings && cargo test -p <crate> --tests --all-features`, with `--features ml,internals` for `lumos` tests). Submodules run their own chain from their own directory. A change to a public type that other crates use runs `--workspace`.

---

## 5. Coverage

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
| Style rules not applied | 1.2 and the owning workstream |

TR sections map to W14 (harness, determinism, SIMD/GPU, placement, fixtures, test-only code) and to the owning workstream (cannot-fail tests, loose tolerances, missing tests, second sources of truth, duplicates). The six `ISSUES.md` bugs are in phase 2. The feature list in `.notes/todo.txt` is not part of this plan.
