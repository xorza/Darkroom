# Lumos stacking and RAW decode review

Scope: `lumos` RAW decode (`io/raw`, `libraw-sys`), demosaic, calibration masters, the combine
engine (rejection, normalization, weights, quality planes), the pipeline and frame storage, and
drizzle plus the warp that feeds the combine. Reference projects (shallow clones in `.tmp/`):
Siril, DeepSkyStacker, RawTherapee, librtprocess, darktable, ccdproc. PixInsight is compared
through its documented behavior.

The detailed per-area reports, with full evidence and reference citations, are in
`.notes/lumos-stacking-review/{raw_decode,demosaic,calibration,combine_math,pipeline,drizzle_warp}.md`.
Each ID below (RAW-n, DMS-n, CAL-n, CMB-n, PIP-n, DRZ-n) points to an entry there.

Paths are relative to `lumos/src/` unless stated otherwise.

## Summary

The basics are strong. These parts match or beat the reference tools:

- The operation order: calibrate on CFA, then cosmic rays, then demosaic, register, and stack.
- Spills are lossless f32, and results are deterministic. The RAM tier and the spill tier give
  bit-identical results.
- The GESD, winsorized, MAD and linear-fit math is correct.
- The STScI `boxer`/`sgarea` port is faithful.
- The RCD and Markesteijn ports fix real librtprocess bugs.

The defects that matter most are:

1. **Calibration gives silently wrong lights** in three reproduced cases: a double pedestal, a
   master that keeps frame 0's exposure, and a flat that keeps its bias (Batch 1).
2. **The `light()` preset can run out of memory.** Normalization builds a full-plane buffer for
   each thread, and no memory plan counts it (Batch 2). **The editor never uses that preset**,
   so its light stacks have no normalization and no noise weighting (Batch 2b).
3. **The exact masked-area black level never applies**, so Canon frames keep LibRaw's truncated
   black (Batch 3). **Corrupt RAW files decode "successfully"** (Batch 4).
4. **Sigma clip and winsorized over-reject clean data** on stars and nebulae, up to about 4× the
   nominal rate at small N (Batch 5).
5. **The warp-to-combine handoff costs samples and weight.** Flag dilation drops about 3.6 % of
   the samples. The confidence weighting gives the reference frame less weight (Batch 6).
6. **The variance plane ignores flat-field amplification.** In vignetted corners it reports 2–4×
   too little variance (Batch 7).
7. **The combine on the spill tier reads about 3× the bytes it needs** (Batch 9).

Reading guide: batches are sorted by impact. Each batch is one change that should land in one
go: it touches one area, and its parts depend on each other or share tests. "Confidence" is
*confirmed* (reproduced, or proved from the code), *likely* (argued from the code but not
measured), or *speculative*.

---

## Batch 1: Calibration correctness — **high**

Silently wrong calibrated lights. CAL-1, CAL-2 and CAL-3 were reproduced with probes that use
only the public API, and no existing test catches them.

| ID | Finding | Where | Conf. |
|---|---|---|---|
| CAL-1 | `calibrate` computes the dark's domain map *before* the bias subtraction changes the light's pedestal from `Kept` to `Removed`. The pedestal is then removed twice: the probe gave 0.21875 where 0.25 was expected, a loss of exactly 2048/65536. `from_images` does it correctly because it recomputes the map after each subtraction. | `calibration_masters/mod.rs:391-430` | confirmed |
| CAL-2 | `stack_cfa_master` takes the master's exposure and temperature from frame 0 and never checks that the set agrees. Darks of 120, 300, 300, 300 and 300 s give a 300 s master labeled 120 s, and that dark is then scaled ×2.5. | `calibration_masters/mod.rs:98-126`, `combine/cache/set_facts.rs` | confirmed |
| CAL-3 | `MasterSubtraction` accepts a subtractor that is already calibrated and still marks the flat as calibrated. `from_images` then skips the flat's bias, and the light is left 18 % non-uniform. Root cause: `ImageMetadata::calibrated: bool` cannot say *what* was removed. | `calibration_masters/master_subtraction.rs:16-38`, `mod.rs:245-249` | confirmed |
| CAL-4 | The exposure and temperature of a flat-dark are never matched to the flats, although lights get this check through `dark_scale`. | `calibration_masters/mod.rs:245-271` | confirmed |

**Direction:** Replace `calibrated: bool` with a typed record of what each frame lost (bias,
thermal, flat). Derive every subtraction map from the domain that the light will have when that
master is applied, or fold bias and dark into one subtraction. Admit calibration frames only when
their exposure and temperature agree within the existing tolerances, and use a typed error that
names the frame. Add exact dyadic tests for each reproduced case.

---

## Batch 2: Memory budget correctness — **high**

The memory plan is the reason the spill tier exists. These allocations escape the plan.

| ID | Finding | Where | Conf. |
|---|---|---|---|
| PIP-1 | Global normalization (the `StackConfig::light()` preset) grows a full-plane f32 buffer on each rayon worker (`map_init(Vec::new)` + `reserve_exact(sample_count)`). Example: 61 MP on 32 threads needs about 7.8 GB on top of a plan that already uses 75 % of memory. Siril limits normalization threads by memory per image. | `combine/normalization/mod.rs:289,353,413` | confirmed |
| CMB-5 | The linear-fit `NormalScores` table is N(N+1)/2 f64 per thread: 100 MB at N = 5000, ×32 threads. It grows lazily on the hot path and is not budgeted. | `combine/rejection/normal_scores.rs:334-350`, `scratch_buffers.rs:79-85` | confirmed |
| PIP-23 | `.max(MIN_CHUNK_ROWS)` overcommits without a report. The output flags plane is not counted in `resident_bytes`. | `memory/chunk_memory_layout.rs:36-37`, `stack_product/quality_planes.rs:58-62` | confirmed |
| PIP-25 | The tier-equivalence test is mono with `Normalization::None`, and the memory probes use the default config. Neither test reaches PIP-1 or PIP-2. | `pipeline/tests/mod.rs:784-916`, `pipeline/tests/mem_budget_probe.rs:183` | confirmed |

**Direction:** First extend the tier sweep to {mono, RGB} × {None, Global} and add a
`light()` memory probe. Then make the common-domain median exact without a copy: a radix or
histogram select over the f32 bits, masked by the domain, with bounded scratch. The alternative
is to limit the parallel width to `budget / plane_bytes` and charge that width in the plan. Cache
the normal scores only for the counts actually seen, and reserve the cache for `frame_count`.

---

## Batch 2b: Light stacking policy — **high (do after Batch 2)**

The editor never normalizes or weights lights. The lens method presets
(`lens/src/astro/config/stacking.rs:219-226`: `sigma_clipped`, `winsorized`, `median`, `mean`)
take everything except the method from `StackConfig::default()`. That is
`Normalization::None` + `Weighting::Equal`, and `AlignStackConfig::default()` does the same. Only
`StackConfig::light()` has Global + Noise, and nothing in the editor uses it. Frames with different
sky levels are then clipped as if the difference were outliers. Siril, PixInsight and DSS
normalize and weight lights by default.

| ID | Finding | Where | Conf. |
|---|---|---|---|
| PIP-21 | The method presets carry the run policy (normalization, weighting, ingest). The default lights pipeline is not normalized and not weighted. `IngestConfig` is inside the combine's `StackConfig`. | `combine/config/mod.rs:156,171-245,302-309`, `pipeline/config.rs`, `lens/src/astro/config/stacking.rs:219-226` | confirmed |

**Direction (decided):** The method presets set only the method. The run sets normalization and
weighting by role: lights get Global + Noise, and calibration masters keep their own presets.
`AlignStackConfig` defaults to the light policy. Lens adds knobs for normalization and weighting
with those defaults. Hoist `IngestConfig` to the run level. Batch 2 must land first, because
this turns on the normalization memory path (PIP-1) for every editor stack.

---

## Batch 3: RAW black level precision — **high (small change)**

| ID | Finding | Where | Conf. |
|---|---|---|---|
| RAW-1 | The exact masked-area black level never applies. The check requires `libraw.black == 0`, but `unpack()` moves the lowest channel mean into `black` (`LibRaw/src/decoders/unpack.cpp:494-502`). A probe against `libraw.a` printed `black 2048 cblack 0 0 1 1`, and the check failed on every channel. Canon CR2/CR3 and other masked-area cameras keep a truncation error of 0–1 ADU per channel per frame. The unit test passes only because it builds a state that cannot occur after unpack. | `io/raw/black_level/mod.rs:100-111`, `black_level/tests.rs:116-155` | confirmed |
| RAW-19 | DNG `BlackLevelDeltaH/V` collapse to a scalar mean inside LibRaw. Document the limit. | `io/raw/black_level/mod.rs:91-99` | confirmed |

**Direction:** Test `black + cblack[c] == sums[c] / counts[c]`, then rebuild each channel from
`sums[c] / counts[c]`. Build the test fixture through LibRaw itself (`open_bayer`, a mask, then
`unpack`), not by hand.

---

## Batch 4: LibRaw boundary rewrite — **high**

These items all land in one new `Libraw` wrapper file: the data-error counter and the cancel hook
need its user-data pointer, and the typed errors come from its methods.

| ID | Finding | Where | Conf. |
|---|---|---|---|
| RAW-2 | A corrupt (not truncated) file decodes with garbage pixels and no flag. LibRaw's `derror()` throws only at EOF. Otherwise it counts the error and prints to stderr through the default callback of `libraw_init(0)`. No `libraw_set_dataerror_handler` is installed. | `io/raw/mod.rs:68, 522-529` | confirmed |
| RAW-9 | The FFI boundary is spread over about 25 `unsafe` blocks. `extract_iso` and `open_libraw_input` are safe fns that dereference a raw pointer. The `data.as_ptr()` read of the flexible array at `:448/:480` is UB under Stacked Borrows. | `io/raw/mod.rs` | confirmed |
| RAW-10 | Platform `#[cfg]`s with two different LibRaw input paths (`open_file` on Unix, `fs::read` + `open_buffer` elsewhere). Its premise is outdated: `libraw_open_wfile` exists, and Siril and darktable use it. | `io/raw/mod.rs:10-14, 51-61, 705-735` | confirmed |
| RAW-11 | Errors are strings. LibRaw codes print as bare integers, and `BlackLevelError` and `XTransPatternError` are flattened with `to_string()`. | `io/raw/mod.rs:154-159` + call sites, `io/image/error.rs:28` | confirmed |
| RAW-20 | A load cannot be cancelled while LibRaw runs. The fallback `dcraw_process` takes seconds on 50 MP+. | `io/raw/mod.rs:743-788` | confirmed |
| RAW-21 | Small items: `CBLACK_LEN` and the X-Trans marker `9` duplicate the `sys::` constants, `raw_err` is too visible, metadata construction is duplicated, there is a dead null check in `ProcessedImageGuard`, `Debug` prints the whole file buffer, and the fallback deinterleave makes a second copy. | see report | confirmed |

**Direction:** Use one `struct Libraw` in its own file. It has one `unsafe` accessor for
`&libraw_data_t`, plus `unpack()`, `raw_image() -> Option<&[u16]>`, and an RAII
`ProcessedImage` type that gives a typed slice through `&raw const`. Use one input path on every
OS (decided): `fs::read` + `open_buffer`. Count the compressed file in the memory plan for each
decode slot, and measure unpack time against `open_file` first. Install a data-error
handler that counts into the wrapper and fails with a typed `CorruptData` error. Add a shim for
`setCancelFlag`. Add a `RawError` enum with a `LibrawCode` mapping.

---

## Batch 5: Rejection scale precision — **medium-high**

These items share the `Spread`/`Pass` machinery, and the tests must land with the fix.

| ID | Finding | Where | Conf. |
|---|---|---|---|
| CMB-1 | Iterated sigma clip and winsorized measure the MAD (or the winsorized SD) of the window that the last pass already trimmed. They then scale it with constants for a complete Gaussian sample, so σ shrinks with each pass. Clean-data rejection at 2.5σ (nominal 1.24 %): 5.5 % at N = 10, 3.9 % at N = 20, 2.4 % at N = 50. Winsorized at k = 3, N = 10 rejects 2.3 % against a nominal 0.27 %. Linear fit avoids this with full-count normal scores. Siril has the same bias. | `combine/rejection/pass.rs:58`, `math/statistics/spread.rs:28-39`, `winsorized_clip_config.rs:137-171` | confirmed (rates from simulation) |
| CMB-2 | The σ floor is the background RMS only. On the sky it hides CMB-1 (0.8–0.9 %). On stars, nebulae and flats the true σ is higher than the floor, so the CMB-1 rates come back exactly where photometry is done. | `combine/rejection/mod.rs:161-164`, `spread.rs:82-86` | confirmed |
| CMB-16 | After a ±kσ clip, the dispersion plane reads 3–9 % low (truncated-normal factor 0.911 at k = 2.5). | `combine/cache/sample.rs:142-147,179-183` | confirmed (analytic) |
| CMB-14 | The clean-frames test allows a 5 % RMS error and lets CMB-1 through. No clean-data rate test exists for sigma clip or winsorized. The bench has no large-N or linear-fit row. | `combine/tests/mod.rs:255-286`, `combine/bench.rs` | confirmed |
| CMB-15 | The `min_survivors` doc and the claim that winsorized is stable at small N are stale or wrong. | `combine/config/mod.rs:23-27,161-165` | confirmed |

**Direction:** After pass 0, measure the scale with censoring awareness: a rank-based spread on
the full-count Blom scores, as `NormalScores` already provides. For `RejectionScale::Robust`,
use the noise-model σ at the pass centre as the floor (the value `clip_by_model` already
computes). Correct the dispersion by the truncated-normal factor where the band is known. Add
exact clean-data rate tests, with and without the floor.

**Also decided (CMB-13):** for the `CcdModel` rejection scale only, test each sample against its
own band `|x−c|/σᵢ`. The robust scales keep the sorted window. Today every method uses one RMS
band for samples whose variance differs (`combine/rejection/pass.rs:44-90`).

---

## Batch 6: Warp → combine weights and flags — **medium-high**

These items affect every registered stack.

| ID | Finding | Where | Conf. |
|---|---|---|---|
| DRZ-4 | Warp flags grow over the whole kernel window, whatever the weight of each tap. With Lanczos3, one CR, defect, repaired or saturated pixel removes 36 output samples (64 when the stretch is a little above 1). This happens even at integer shift. A frame with 0.1 % flagged pixels loses about 3.6 % of its samples. The reference frame keeps its flags without dilation, so it is treated differently from the warped frames. | `registration/resample/mod.rs:178-211` | confirmed |
| DRZ-3 | The combine multiplies the frame weight by the warp confidence `q` (Kish ESS: 1–1.62 for Lanczos3, 1–4 for bilinear). The unwarped reference (the sharpest frame, chosen by `Auto`) always has `q = 1`, so it enters at about 0.79× the weight of an average warped frame with Lanczos3, or 0.41× with bilinear. Weights also change with the sub-pixel phase, which makes a moiré pattern on rotated fields. | `combine/cache/mod.rs:481-496` | confirmed |
| DRZ-10 | The kernel stretch has no tolerance band. A stretch of 1 + 1e-12 changes the Lanczos3 reach from 3 to 4, which makes DRZ-4 worse. | `registration/resample/kernel/warp_kernel/mod.rs:160-191` | confirmed |
| DRZ-9 | The Lanczos LUT is read at the nearest entry, so each tap weight is off by up to 1.7e-4. Linear interpolation between entries gives about 1e-8. | `registration/resample/kernel/warp_kernel/mod.rs:233-237` | confirmed |

**DRZ-3 (decided):** keep `q` in the per-sample noise model (rejection and the variance plane),
and weight the mean with frame weights only, as Siril, PixInsight and DSS do. The combine review
had accepted the `q` weight because `wᵢvᵢ = 1` holds with noise weights. That consistency is
given up deliberately: the smaller variance comes from smoothing, the noise is correlated, and it
discounts the reference frame. State the trade-off on `WarpResult::confidence`.

**Direction:** Send the fill-type flags (`COSMIC_RAY`, `REPAIRED`, `DEFECT`) through the existing
masked `NO_DATA` interpolation path. Propagate the bound-type flags (`SATURATED`, `FLAT_FLOOR`)
only where the flagged taps carry a non-negligible share of `Σ|L|`. Snap the stretch to 1 within
a stated tolerance. Interpolate the LUT.

---

## Batch 7: Noise model and science planes — **medium-high (science product)**

| ID | Finding | Where | Conf. |
|---|---|---|---|
| CMB-3 | The variance plane does not include flat-field amplification. After division by `f`, the read variance is `σ²/f²` and the photon variance is `x/(e·f)`. The model uses one global σ and one global e⁻/unit, so corners at `f = 0.5` report 2–4× too little variance. ccdproc propagates uncertainty through `flat_correct`. | `math/noise/ccd_noise.rs`, `combine/cache/sample_noise.rs:229-266` | likely (absence confirmed) |
| CAL-12 | The `NoiseEstimation::Gain` cosmic-ray model has the same gap. Cosmic-ray detection runs after the flat, so the source term is low by `1/f` in the corners, and the corners get too many flags. | `calibration_masters/cosmic_ray/noise_model.rs:43-51` | likely |
| CAL-11 | `quantization_sigma` is not updated after the dark and flat steps, but it is used as a noise floor downstream. | `calibration_masters/mod.rs:423-438` | speculative |
| CMB-9 | A pixel with no weight reports variance 0, which reads as "exact". Manual weights of 0 are accepted, so a covered pixel whose survivors all have zero weight gets the value 0 without a flag. | `combine/cache/sample.rs:174-196`, `combine/config/mod.rs:330-341` | confirmed |
| CMB-10 | The median path computes a dispersion on value/weight pairs that `median_mut` permuted apart, and then discards it. This is a latent bug and an extra pass. | `combine/stack/mod.rs:311-325` | confirmed |
| CMB-7 | The master quantization σ costs an extra loop over the survivors for each pixel to give one worst-pixel scalar. | `combine/stack/mod.rs:341-392` | confirmed |

**Direction:** Carry a per-pixel flat variance factor with the frame, as confidence is carried,
and warp it with the frame. Use it in `CcdNoise`, in the rejection floor (Batch 5) and in the
cosmic-ray model. For a median, write only `Σw`.

Decided:
- **CMB-9:** publish an inverse-variance plane in place of the variance plane, so 0 means "no
  information". Refuse `Manual` weights of 0 at validation. A caller drops a frame by removing
  it from the set.
- **CAL-11 / CMB-7:** `quantization_sigma` means the source's ADC step noise before flat
  division. It stays unchanged through calibration, and its doc says so. Gain-mode cosmic rays
  (`cosmic_ray/noise_model.rs:31`), the `FrameStats` noise floor (`frame_stats.rs:148,164`) and
  the defect residual floor (`defect_map/mod.rs:360`) keep working. Remove the master's
  worst-pixel computation: a master states the step of its inputs. The 1/f part comes from the
  flat variance factor above.

---

## Batch 8: CFA-aware normalization — **medium**

| ID | Finding | Where | Conf. |
|---|---|---|---|
| CMB-4 / CAL-20 | A CFA mosaic gets one normalization affine for all colours, but noise and weights are per colour. Twilight flats drift in colour, so rejection clips real frames. The stratified sample also aliases with the CFA: a 4096-wide mosaic sees only R and G2. | `combine/normalization/mod.rs:222-277,312-397,458-460`, `combine/cache/slots.rs:36-38` | confirmed |

**Direction:** Index `FrameNorm` by slot (colour), as noise and weights already are. Measure
medians and gains for each colour, and stratify the sample for each colour.

---

## Batch 9: Combine I/O passes — **medium-high (performance, bit-identical)**

The combine phase reads an RGB warped spilled light about 15.75 plane-equivalents, against 5.25
needed (mono: 7.25 against 3.25). Every item gives bit-identical output, and the extended tier
sweep from Batch 2 guards them.

| ID | Finding | Where | Conf. |
|---|---|---|---|
| PIP-2 | The loop runs channel-outer, so the coverage, confidence and flag planes are re-read once for each channel (about 1.8× the bytes). | `combine/cache/core.rs:119-120`, `combine/cache/mod.rs:378-394` | confirmed |
| PIP-3 / CMB-8 | The coverage plane is a second full pass that recomputes the `covered` count that the gather already makes. The `CommonDomain` build is serial I/O over N mapped files. | `combine/cache/mod.rs:254-302`, `combine/normalization/common_domain.rs:41-63` | confirmed |
| PIP-27 | Normalization of an unwarped spilled set reads every page to gather 65 536 samples. | `combine/normalization/mod.rs:401-455` | confirmed |
| PIP-7 | An unwarped masked frame stores two identical f32 planes that come from a 1-bit mask. | `frame_store/frame_quality.rs:98-108`, `stored_image.rs:73-90` | confirmed |

**Direction:** Run the loop chunk-outer and channel-inner, and price
`ChunkMemoryLayout::input_bytes` to match. Write coverage from the gather on the first channel.
Build the domain in parallel, or AND it into a shared bitset during the warp. Take the
normalization samples in `FrameStats::measure` while the frame is in RAM. Add a
`FrameQuality::Mask`.

---

## Batch 10: X-Trans pattern contract — **medium**

| ID | Finding | Where | Conf. |
|---|---|---|---|
| DMS-2 | `XTransPattern::new` accepts layouts that the hex table and `is_green` cannot handle. It never checks that greens repeat every 3 rows and columns, or that the hex table fills. Patterns come from untrusted FITS `XTRNROW*` keys, serde and LibRaw. A random search found 294 such layouts. 257 of them hit an `assert!` in `hex_lookup.rs:127` (a panic on untrusted data), and the rest demosaic wrongly. | `io/raw/demosaic/xtrans/xtrans_pattern.rs:43-88`, `hex_lookup.rs:127-135` | confirmed |
| DMS-14 | `HexLookup` exists only so that `HexTable` can flatten it. Its comments narrate the code. `xtrans::demosaic` is a wrapper that only logs time. `XTransImage::new` has no finite-data `debug_assert`. | `io/raw/demosaic/xtrans/hex_lookup.rs`, `xtrans/mod.rs:27-44` | confirmed |

**Direction:** Validate period-3 greens, exactly one solitary green, and a filled hex table, and
use a typed error for each. Build the hex table once inside `XTransPattern::new`, and delete
`HexLookup`.

---

## Batch 11: Demosaic on signed, faint data — **medium**

| ID | Finding | Where | Conf. |
|---|---|---|---|
| DMS-1 | Markesteijn's green bounds start `maxval` at 0, copied from librtprocess, which sees only unsigned data. Where all six hex greens are negative, the clamp is one-sided and adds a positive bias to calibrated backgrounds near zero. | `io/raw/demosaic/xtrans/markesteijn/tile.rs:159-160` | confirmed |
| DMS-3 | RCD's absolute `EPS = 1e-5` in the ratio step moves the plane means by ±EPS/16: G low, R and B high. That is ±0.6 % at a level of 1e-4, and the step is not invariant to offset. | `io/raw/demosaic/bayer/rcd/mod.rs:27, 83-104` | confirmed (analytic) |
| DMS-12 | No test uses signed or low-SNR input, none checks the mean of each colour, and several assert thresholds instead of exact values. | `bayer/tests.rs:393-488`, `markesteijn/tests.rs:139-156`, `hex_lookup.rs:155-249` | confirmed |

**Direction:** Start `maxval` at `f32::MIN`. Remove `EPS` from the well-conditioned branch, and
send the conditioning test through the existing signed blend. Add a table-driven flat-field test
over the levels {−1e-4, 0, 1e-5, 1e-4, 1e-2}, with symmetric noise and the exact mean of each
plane. The librtprocess digests stay valid.

---

## Batch 12: Fused per-light calibration pass — **medium (performance + precision)**

| ID | Finding | Where | Conf. |
|---|---|---|---|
| CAL-8 | Per-light calibration sweeps the whole frame 4–6 times: saturation, bias, dark, flat, and the `FLAT_FLOOR`/defect `add_where` calls. Each flag update ends in a full recount of the flag plane on one thread. 60 MP: about 2.6 GB of traffic where 1.2 GB is enough, and three roundings where one is enough. | `calibration_masters/mod.rs:423-437`, `io/image/cfa/mod.rs:444-506`, `io/image/pixel_flags.rs:125-145,340` | likely |
| DMS-10 | Nulls are repaired twice for each light: at the end of `calibrate` and again at the start of `demosaic`. | `calibration_masters/mod.rs:437`, `io/image/cfa/mod.rs:326` | confirmed |

**Direction:** Use one row-parallel kernel, `(L − B·g_b − o_b − s·(D·g_d + o_d)) / F`, with
saturation and master flags in the same pass. Precompute `FLAT_FLOOR` into the divisor's flags.
Update flag counts incrementally. Give null repair one owner. Do this after Batch 1, because it
rewrites the same function.

---

## Batch 13: Cosmic-ray quality — **medium**

| ID | Finding | Where | Conf. |
|---|---|---|---|
| CAL-6 | Detection ignores `SATURATED` and `NO_DATA`. Saturated star cores (the classic L.A.Cosmic false positive) get in-painted, and the in-paints read from saturated or filled neighbours. astroscrappy masks saturated stars. | `calibration_masters/cosmic_ray/mod.rs:59-101`, `mono.rs:346-380`, `xtrans.rs:313-351` | confirmed |
| CAL-7 | The X-Trans detector applies astroscrappy's `sigclip`/`objlim` to a different statistic, `(v − median₈)/N`, so its false-positive rate is not calibrated. It has no noise-only test and no star-preservation test. | `cosmic_ray/xtrans.rs:187-288`, `tests.rs:272-319` | likely |
| CAL-9 | Iterations 2..n recompute the whole frame, but only pixels within about 6 px of a repair can change. A local recompute gives a bit-identical result. | `cosmic_ray/mono.rs:100-144`, `xtrans.rs:203-228` | likely |
| CAL-17 | The X-Trans detector takes `&CfaType` and panics on other types. Take `XTransPattern` instead. | `cosmic_ray/xtrans.rs:159-170` | confirmed |

---

## Batch 14: Drizzle as a science producer — **medium (no production caller yet)**

| ID | Finding | Where | Conf. |
|---|---|---|---|
| DRZ-1 | Drizzle has no outlier rejection and no frame normalization. Satellite trails and sky-level differences go in at full weight. Siril, PixInsight and DrizzlePac reject before or during drizzle. | `drizzle/accumulator/frame_source.rs:214,418`, `drizzle/stack.rs` | confirmed |
| DRZ-7 | It accepts non-finite samples, and one NaN spreads to every output pixel its drop reaches. The combine refuses the same input. | `drizzle/accumulator/mod.rs:379-418` | confirmed |
| DRZ-8 | The flag policy is different from the combine's: drizzle deposits `FLAT_FLOOR` and `DEFECT`-only samples. The output has no flags and an empty `RunReport`. | `drizzle/accumulator/frame_source.rs:214-216`, `combine/cache/sample.rs:10-14` | confirmed |
| DRZ-6 | The fixed-footprint kernels divide by local magnification, but STScI does this only for the square kernel. When plate scales differ, the relative frame weights then depend on the kernel. | `drizzle/accumulator/frame_source.rs:263-278` | confirmed |
| DRZ-13 | The output unit (surface brightness: total = s² × input flux) is not stated on the API. | `drizzle/config.rs`, `drizzle_result.rs` | confirmed |

**Direction (decided, Siril's design):** Drizzle each frame onto the output grid as its own frame.
Its drop weight becomes the coverage/confidence planes. Then run the normal combine, so
rejection, normalization, weights, flags and quality planes come from one engine. The spill tier
holds the s²× larger frames. Define the exclusion policy once in `pixel_flags`. For DRZ-6, follow
STScI: divide by the magnification only in the square kernel, and extend
`a_magnified_frame_weighs_less_per_output_pixel` to the mean weight over one lattice period.

---

## Batch 15: CFA (Bayer / X-Trans) drizzle — **medium (feature)**

| ID | Finding | Where | Conf. |
|---|---|---|---|
| DMS-5 / DRZ-5 | No CFA drizzle path exists: every OSC frame is interpolated twice (demosaic, then warp). The variance plane also treats demosaic-interpolated samples as independent. PixInsight, Siril and DSS all offer CFA drizzle as the best path for dithered OSC data. | `pipeline/light_source.rs:381-406`, `drizzle/accumulator/mod.rs:40,368` | confirmed |

**Direction:** Accept a calibrated `CfaImage`. Deposit each photosite only into the plane of its
own colour, with weight and coverage planes for each channel (Siril `cdrizzlebox.c:448`). With
the Batch 14 design, these per-channel planes go straight into the combine.
Register on a quick demosaic or a green-only proxy. Do this after Batch 14.

---

## Batch 16: Drizzle scatter performance — **medium**

| ID | Finding | Where | Conf. |
|---|---|---|---|
| DRZ-2 | Each output band scans every column of the input rows that it maps back to. On a rotated field, each band scans most of the frame, and the extra work grows with the thread count (4 bands per thread). For square + SIP, each visit costs 4 Newton solves before the band test. The bench covers only a 1° rotation. | `drizzle/accumulator/output_band.rs:116-145`, `frame_source.rs:282-347` | confirmed (cost not measured) |
| DRZ-11 | Magnification is computed before the band test. Drizzle Lanczos calls `sin` for each tap. | `frame_source.rs:141-159`, `output_band.rs:102` | confirmed |

**Direction:** Limit each input row to the closed-form column interval that can reach the band.
Test the landing row first. Share the corner lattice for each row. Reuse the (interpolated)
Lanczos LUT. Add bench legs for 45°, 90° and SIP first.

---

## Batch 17: RAW sensor classification and refusals — **medium (rare cameras, silent)**

Do this after Batch 4. Most items need a shim accessor or a `colors`/`cdesc` check.

| ID | Finding | Where | Conf. |
|---|---|---|---|
| RAW-3 | Four-colour sensors (Sony F828 RGBE, Nikon CMYG) are accepted as Bayer RGB, because `colors` and `cdesc` are never read. | `io/image/cfa/mod.rs:87-101` | confirmed |
| RAW-6 | Phase One compressed IIQ: the black is never subtracted, but the frame declares `Pedestal::Removed`. | `io/raw/mod.rs:256-305` | confirmed |
| RAW-4 | The quantization σ claims a 1-ADU step for Canon C-RAW and Fuji lossy RAF, which bypass `curve`. Phase One has a 4-ADU step. The doc comment is wrong. | `io/raw/mod.rs:184-187, 640-646` | confirmed |
| RAW-15 | Float DNG is silently converted to `u16` (clamped and truncated) by `CONVERTFLOAT_TO_INT`. | `io/raw/mod.rs:513-667` | confirmed |
| RAW-7 | `linear_max` is trusted with no bounds check when the saturation level is chosen. | `io/raw/mod.rs:630-639` | likely |
| RAW-17 | Peek can disagree with the load. Example: a Fuji SuperCCD file passes peek, and then the load refuses it. | `io/raw/mod.rs:751-778` | confirmed |
| RAW-14 | The build cannot decode deflate DNG, lossy DNG, X3F or GPR, but `RAW_EXTENSIONS` lists `x3f` and `gpr`. | `libraw-sys/build.rs:8`, `io/raw/mod.rs:41-49` | confirmed |
| RAW-13 | LibRaw is built without OpenMP, so CR3, Fuji-compressed and Panasonic v8 decode on one core. | `libraw-sys/build.rs:35-46` | confirmed |
| RAW-5 | Canon writes `CameraTemperature`, not `SensorTemperature`, so Canon frames never carry a temperature and the dark temperature check never runs. | `io/raw/mod.rs:624-627`, `calibration_masters/mod.rs:450-455` | confirmed |
| RAW-8 | The 95 % saturation threshold also applies where the file states the true clip (`linear_max`), so good samples between 95 % and 100 % are flagged. | `io/image/pixel_flags.rs:8-12`, `io/raw/mod.rs:632-639` | likely |
| RAW-16 | RAW frames leave `instrument`, `date_obs` and `focal_length` empty, so nothing can check that masters come from the same camera. | `io/raw/mod.rs:279-297, 338-355` | confirmed |

**Direction (decided):**
- **RAW-14:** add `libz-sys` (approved new dependency) and define `USE_ZLIB`, so deflate DNG
  decodes. Remove `x3f` and `gpr` from `RAW_EXTENSIONS` and the Foveon claim. Lossy DNG stays
  refused.
- **RAW-13:** enable OpenMP (approved system dependency). Bound the OpenMP thread count so that
  rayon decode slots × OpenMP threads do not oversubscribe the cores. The build must stay valid
  on macOS (libomp) and Windows.
- **RAW-5:** add a separate `camera_temperature` field. Calibration uses it only to match darks
  when no sensor temperature exists, and the outcome says which one it used.
- **RAW-8:** with RAW-7, use `linear_max[c]` only when `black_c < linear_max[c] ≤ maximum`. Then
  the saturation level is that value minus a small stated ADU margin. Keep 95 % only for
  `maximum`.
- **RAW-16:** fill `instrument` (normalized make/model), `date_obs` and `focal_length`. Leave the
  pixel size empty. Calibration refuses masters from another camera model.
- The other items: read `colors`/`cdesc`, and add shim accessors for `fuji_lossless`, the CRX
  header and `is_phaseone_compressed`. Refuse float DNG with a typed error or read
  `float_image`. Share one identify-stage validator between peek and load.

---

## Batch 18: Shared demosaic driver, gains inside the kernel — **medium (performance + design)**

| ID | Finding | Where | Conf. |
|---|---|---|---|
| DMS-11 | The tiled-driver code is duplicated between RCD and Markesteijn: `OutputPlanes`, memory accounting, `tile_starts`, the parallel/cancel/border block, and two border fills that compute nearly the same thing on Bayer. | `bayer/rcd/{mod,tile}.rs`, `xtrans/markesteijn/{mod,border}.rs` | confirmed |
| DMS-8 | The white-balance and unbalance steps are 4 extra full-frame passes (about 768 MB of memory traffic at 24 MP, roughly 25–40 ms against a 125 ms RCD). | `io/image/cfa/mod.rs:357-383` | likely |
| DMS-4 | The same round trip changes native samples by up to 1 ulp. | `io/image/cfa/mod.rs:353-383` | confirmed |

**Direction:** Use one generic tiled driver and one CFA-generic border fill. Apply the gains when
each tile reads its input, and write native samples from the unbalanced input.

---

## Batch 19: RCD vectorization — **medium (performance, measure first)**

| ID | Finding | Where | Conf. |
|---|---|---|---|
| DMS-7 | RCD's main loops are scalar, step two columns at a time, branch, and sit outside `simd::Isa`. They vectorize only with the x86-64-v3 flag, which AGENTS.md says can be turned off. The low-pass filter is computed at every pixel, but only R/B sites use it. Tiles recompute 40 % overlap. | `io/raw/demosaic/bayer/rcd/tile.rs:133-375` | likely |
| DMS-9 | Three-pass Markesteijn recomputes 2.1× its output, and its 8-direction RGB buffer (884 KB) is larger than L2. | `xtrans/markesteijn/mod.rs:46,75-80`, `tile.rs:57` | speculative |

**Direction:** Split the frame into per-phase half-resolution planes at copy-in, so the loops
become contiguous `simd::Isa` kernels. Compute the low-pass filter only at R/B sites, keep the
high-pass filter in ring rows, and retune `TILE`. The librtprocess digest stays the oracle.

---

## Batch 20: One ingest stage — **medium (design, simplification)**

| ID | Finding | Where | Conf. |
|---|---|---|---|
| PIP-11 | Ingestion (peek, plan, admit, bounded decode, tier store) is implemented twice, in `combine/cache/loader` and `pipeline/light_source`. `ingest` and `combine` import each other. | `combine/cache/loader/mod.rs`, `pipeline/light_source.rs:101-406`, `ingest/frame_admission.rs:7-9` | confirmed |
| PIP-12 | The tier decision has four representations: `fits_in_ram`, `FrameTier`, `CacheTier::of(bool)` and `LoadedTier.spilled`. | `memory/memory_plan.rs:84-89`, `pipeline/frame_tier.rs`, `combine/cache/core.rs:38-56` | confirmed |
| PIP-13 | Frame-carrier types overlap: `DetectedFrame`/`FrameToPark`, `StoredImage`/`StoredFrame`, `Lights`/`LightSource`, and nine private loader types. | see report | confirmed |
| PIP-10 | Lights that are already calibrated on disk have no tiered entry, so RGB FITS lights must be fully resident and can never spill. | `pipeline/align.rs:42-55`, `pipeline/calibrate.rs:37-69` | confirmed |
| PIP-4 | `keep_cache` works only on the disk tier, and the registered runs never use it, which is not what the doc promises. | `combine/cache/loader/mod.rs:121-125`, `ingest/ingest_config.rs:22-26` | confirmed |
| CAL-14 | The master-building policy is written twice, in lumos internals and in `lens`. The public `stack_cfa_master` has no role, which is why CAL-3 and CAL-4 are possible. | `calibration_masters/mod.rs:98,572-621`, `lens/src/astro/nodes/calibration.rs:127-240` | confirmed |

**Direction:** Use one ingest type in `ingest/`, with the source as a parameter (paths + optional
`FrameStep`, held images, or RAW + masters) and the sink as a parameter (park or store). Move
`FrameCheck` and `SetFacts` there with an admission error enum. Use one `Tier` enum. Add a
`LightSource::Decoded(&[P])`. Make the kept cache a decode-level memo. Make a lumos-owned
`CalibrationMasters::build` the only master builder.

---

## Batch 21: Platform entity, statics, disk space — **low-medium (your rules)**

| ID | Finding | Where | Conf. |
|---|---|---|---|
| PIP-14 | Platform `cfg`s are spread over `stored_plane.rs`, `run_scratch.rs`, `ingest_config.rs` (`cfg!`), `memory/mod.rs` (a macOS workaround) and `io/raw` (Batch 4). Windows Job-object limits are never read. | `frame_store/stored_plane.rs:27-65`, `run_scratch.rs:68-93`, `ingest/ingest_config.rs:57-68`, `memory/mod.rs:48-53` | confirmed |
| PIP-15 | Statics: `SYSTEM: LazyLock<Mutex<System>>`, `GROUPS`, and `COUNTER`. The outside world does not force any of them. | `memory/mod.rs:32,40`, `frame_store/run_scratch.rs:53` | confirmed |
| PIP-16 | There is no free-space check before a spill, so `ENOSPC` comes only after hours of work. Siril tests the free space first. | `pipeline/frame_tier.rs:67-82` | confirmed |
| PIP-22 | A `Display` string is used as an on-disk file name. | `frame_store/frame_spill.rs:66-73` | confirmed |
| PIP-24 | Free `pub(crate)` fns that should be methods of an owner. Two files have more test code than the 40 % rule allows. | see report | confirmed |

**Direction:** Use `frame_store/platform/{unix,windows}.rs` behind one `ScratchFile` type, and
`memory/platform/{linux,macos,windows}.rs` behind one `SystemMemory` type, which also holds the
disk-space probe. Fail with a typed `InsufficientSpace { needed, available }`.

---

## Batch 22: Combine hot loop — **medium (performance, measure first)**

| ID | Finding | Where | Conf. |
|---|---|---|---|
| CMB-6 | The gather tests six unchanging `Option`s for each sample, divides for each sample, and walks up to 4N plane streams for each pixel. Above about 32 streams the prefetcher loses track, and nothing is vectorized except the f64 sum. | `combine/cache/mod.rs:469-511` | likely |
| CMB-17 | Winsorized can cost up to about 50·N² per pixel in the worst case, with two passes for each clamp step. | `combine/rejection/winsorized_clip_config.rs:144-169` | confirmed |

**Direction:** For each row tile of 64–256 px, loop over the frames on the outside, and transpose
into a `[tile][frame]` buffer with `simd::Isa` gain/offset, mask bits, weights and noise terms.
Remove the `Option`s with monomorphization. Precompute `1/electrons`. Fuse each winsorized step
into one f64 pass. Add the large-N and linear-fit bench rows from CMB-14 first.

---

## Batch 23: Defect-map policy — **medium (needs real data)**

| ID | Finding | Where | Conf. |
|---|---|---|---|
| CAL-5 | The hot-pixel cut is 5× the robust spread of the master dark. That is much more aggressive than Siril (median + k·stddev) or DSS (median + 16·stddev). It probably flags warm pixels that the dark already subtracts correctly, and replaces them with interpolated values. | `calibration_masters/defect_map/mod.rs:301-355` | likely |
| CAL-16 | Bayer green repairs use the four diagonal greens, which are the *other* green phase (Gr ↔ Gb). | `io/image/cfa/cfa_lattice.rs:122-147` | likely |

**Direction (decided):** A pixel is hot when its dark-current shot noise in the light (or its
non-linearity or saturation) is larger than the light's own noise there. A warm pixel that the
dark subtracts correctly stays measured. The threshold depends on the gain or measured noise and
the light exposure, so the map is built per run, not once per master. Measure the flagged
fraction for each colour, and Gr/Gb (CAL-16), on the real-data set.

---

## Batch 27: Dark scaling for unregulated sensors — **medium (feature)**

| ID | Finding | Where | Conf. |
|---|---|---|---|
| CAL-19 | A DSLR has no regulated temperature, so its dark is subtracted at scale 1, and the run is marked "unverified temperature". The 1 °C tolerance also accepts up to about 12 % thermal error with no record. Siril and DSS offer dark optimization. | `calibration_masters/mod.rs:444-475` | confirmed |

**Direction (decided):** For each light, fit the dark scale k by least squares on the hot-pixel
residuals. Do this only for bias-removed darks, because only the thermal part scales. Record k
in the outcome. Do this after Batch 1 and with Batch 23, because both use the hot-pixel set.

---

## Batch 24: Diagnostics — **low-medium**

| ID | Finding | Where | Conf. |
|---|---|---|---|
| PIP-8 | Combine errors in registered runs give the frame's index among the survivors, not its input index, so they can name the wrong light. | `pipeline/registered_set.rs:44-69`, `combine/cache/mod.rs:117-119` | confirmed |
| PIP-9 / CAL-13 | Calibration errors in `calibrate_align_stack` do not say which light failed. | `pipeline/light_source.rs:393`, `pipeline/error.rs:39-40` | confirmed |
| PIP-26 | `lens` describes `Auto` as "the richest frame", but `Auto` picks the frame with the lowest FWHM. The test name has the same error. | `lens/src/astro/nodes/stacking.rs:39,131`, `pipeline/tests/mod.rs:490` | confirmed |
| RAW-18 | One broken symlink of any name makes `raw_files` fail for the whole directory. | `io/raw/raw_files/mod.rs:315-339` | confirmed |

---

## Batch 25: Kept-cache integrity — **low-medium**

| ID | Finding | Where | Conf. |
|---|---|---|---|
| PIP-17 | The kept cache trusts only file length and mtime. The commit has no path and no digest, and it is written without fsync. A same-size rewrite that keeps the mtime (`rsync -t`, `cp -p`), or a power loss, can reuse stale or zero planes. The doc claim that "a clash never reads another's" is not strictly true. | `frame_store/frame_spill.rs:43-46`, `frame_store/cache_key.rs:110-116`, `common/src/file_utils/file_identity.rs` | likely |

**Direction:** Record the canonical path and a blake3 digest for each plane, and verify the
digest in the full-plane pass that reuse already makes. Do this after Batch 20, because the cache
moves there.

---

## Batch 26: Layout, visibility, API cleanup — **low**

| ID | Finding |
|---|---|
| DMS-13 | `CfaPattern` and `XTransPattern` live inside the demosaic modules, but 32 files in I/O and calibration use them. Colour indices are bare integers of two widths. Move the patterns beside `CfaType` and add a `CfaColour` enum. |
| DRZ-12 | `InputMap`, `SipMap` and `DrizzleFrame` need their own files. `sgarea`, `boxer` and `nearest_index` are free fns. The floor helper is written twice. `x2()` is the same as `default()`. Two quality claims in the docs contradict each other. |
| CAL-15 | `DefectMap` is `pub` but has no caller outside the crate. |
| CAL-10 | The master bundle load reads the whole file into memory (about 720 MB at 60 MP) and reports errors as strings. |
| CAL-18 | Comments that narrate history (`fits.rs:40-41`) and doc comments joined to import blocks. |
| CMB-11 | GESD records per-pixel statistics on the hot path, and only one test reads them. |
| CMB-12 | The rejection configs are inconsistent: the constructor shapes differ, and `max_iterations` means different things. GESD accepts `alpha = 0` and `max_outliers = Some(0)`, which turn the method off silently. |

---

## Measure before you change

These items need a benchmark or a real-world trigger before work starts.

| ID | Question |
|---|---|
| PIP-5 | A parked frame is warped directly from an `MADV_SEQUENTIAL` map. Rotated reads go against the readahead. |
| PIP-6 | Is it faster to park the 1-plane calibrated mosaic and demosaic again, than to park 3 planes? |
| PIP-18 | Statistics copy every channel, so the decode transient factor is 2. An exact radix select without a copy would remove the copy. |
| PIP-19 | Disk I/O and compute do not overlap (no `WILLNEED` prefetch of the next chunk). DSS measured reads at about 7 % of combine time. |
| PIP-20 | The progress callback runs under a non-reentrant `Mutex` on rayon workers. A callback that uses rayon can deadlock. |
| RAW-12 | Decode makes two passes over the raw buffer, with a `%`/`/` for each pixel and a flag plane that is always allocated. Fuse it into one row pass. |
| DMS-6 | Derive per-colour gains from the background for third-party OSC FITS (no WB). Compare star colour and zipper artifacts on real data with and without them, and adopt only if it helps. |

---

## Decisions taken

Each decision is also written into the direction of its batch.

| ID | Decision | Batch |
|---|---|---|
| DRZ-3 | Warp confidence `q` stays in the per-sample noise model only (rejection, variance plane). The mean uses frame weights only. | 6 |
| PIP-21 | Split the method from the policy. Method presets set only the method. The run sets normalization and weighting by role: lights get Global + Noise, and masters keep their presets. `AlignStackConfig` defaults to the light policy, and lens adds knobs with those defaults. | 2b |
| CMB-9 | Publish an inverse-variance plane in place of the variance plane, so 0 means "no information". Refuse `Manual` weights of 0 at validation. | 7 |
| CAL-11 / CMB-7 | `quantization_sigma` means the source's ADC step noise before flat division. It stays unchanged through calibration, and that is documented. Remove the master's worst-pixel computation. The 1/f part goes into the flat-aware noise model. | 7 |
| CMB-13 | Use per-sample bands `|x−c|/σᵢ` for the `CcdModel` rejection scale only. The robust scales keep the sorted window. | 5 |
| RAW-10 | Use `fs::read` + `libraw_open_buffer` on every OS. Count the compressed file in the memory plan for each decode slot. Measure unpack time against `open_file` first. | 4 |
| RAW-5 | Add a separate `camera_temperature` field. Calibration uses it only to match darks when no sensor temperature exists, and reports which one it used. | 17 |
| RAW-8 | Use the bounds-checked `linear_max` (RAW-7), minus a small stated ADU margin, as the saturation level when present. Keep 95 % only for `maximum`. | 17 |
| RAW-16 | Fill `instrument`, `date_obs` and `focal_length` from LibRaw. Leave the pixel size empty. Calibration refuses masters from another camera model. | 17 |
| RAW-14 | Add `libz-sys` (approved new dependency) and define `USE_ZLIB` so deflate DNG decodes. Remove `x3f` and `gpr` from `RAW_EXTENSIONS` and the Foveon claim. Lossy DNG stays refused. | 17 |
| RAW-13 | Enable OpenMP in the LibRaw build (approved system dependency). Bound its thread count so it does not oversubscribe the cores together with the rayon decode slots. | 17 |
| CAL-19 | Fit the dark scale k for each light by least squares on the hot-pixel residuals (bias-removed darks only), and record k in the outcome. | 27 |
| CAL-5 | A pixel is hot when its dark-current shot noise in the light (or its non-linearity or saturation) is larger than the light's own noise there. The map is built per run from the gain or measured noise and the light exposure. | 23 |
| DRZ-1 | Siril's design: drizzle each frame onto the output grid as its own frame, with its weight as the coverage/confidence planes, then run the normal combine. | 14 |
| DRZ-6 | Follow STScI: divide by the magnification only in the square kernel. | 14 |
| DMS-6 | Measure first. Derive per-colour gains from the background, compare star colour and zipper artifacts on real OSC data, and adopt only if it helps. | measure |
