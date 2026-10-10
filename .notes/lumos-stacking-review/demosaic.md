# Demosaic review — lumos `io/raw/demosaic/` and how the pipeline uses it

Paths are relative to `/home/xxorza/Projects/darkroom/lumos/src/` unless absolute. References are in
`/home/xxorza/Projects/darkroom/.tmp/`. `cargo test -p lumos --tests --features ml demosaic`: 51 passed, 1.59 s.

Short version: both kernels follow librtprocess closely and are bit-checked against it. The
pipeline order is right: calibrate on CFA, then cosmic rays on CFA, then demosaic, then
register/stack. The main defects:
1. Markesteijn's green bounds assume unsigned data. This biases calibrated near-zero backgrounds.
2. The X-Trans pattern check accepts layouts the hex table cannot handle. On an untrusted FITS
   header that means a panic or a silently wrong demosaic.
3. RCD's absolute `EPS` in the ratio step skews faint signal by colour.
4. There is no CFA (Bayer) drizzle path.
5. The rest are performance and structure items: extra full-frame passes, scalar stride-2 loops,
   and the tiled-driver code duplicated between the two kernels.

---

## Correctness / precision

### DMS-1 — Markesteijn green bounds start `maxval` at 0, so negative backgrounds are clamped on one side only
- **Where:** `io/raw/demosaic/xtrans/markesteijn/tile.rs:159-160` (`let mut minval = f32::MAX; let mut maxval = 0.0f32;`). It is used by `interpolate_green` (`tile.rs:268-270, 282-284`) and `refine_green` (`tile.rs:312-313`).
- **Category:** correctness / precision
- **Impact:** medium. Calibrated frames have backgrounds near zero. Wherever all six hex greens are negative, the upper bound becomes 0 instead of the largest neighbour. Upward overshoots then survive and downward ones are clipped, which adds a positive bias to faint background. It is small per pixel but systematic, and it lands in the low-SNR regime lumos targets.
- **Confidence:** confirmed by reading the code. No test feeds negative data to Markesteijn; `markesteijn_ramp_is_finite_and_zeros_stay_zero` uses ≥ 0 only.
- **Evidence:** librtprocess `src/demosaic/markesteijn.cc:268-279` has the same `float maxval = 0.f;`. That is safe there because RawTherapee and dcraw feed unsigned, black-subtracted and clipped data. lumos explicitly keeps negatives: see the `XTransImage`/`xtrans::demosaic` docs ("samples may lie outside `[0, 1]`") and `CfaImage::subtract` ("the f32 pipeline preserves negatives… clamping would bias").
- **Direction:** Start `maxval` at `f32::MIN` (or `-INFINITY`) so the clamp is symmetric for signed data. The librtprocess digests are unaffected because those scenes are all positive. Add a signed, zero-mean noise case to the Markesteijn tests (see DMS-12).

### DMS-2 — `XTransPattern::new` accepts layouts the hex table and `is_green` cannot handle: panic or a silently wrong demosaic on untrusted input
- **Where:** `io/raw/demosaic/xtrans/xtrans_pattern.rs:43-88` (validation), `io/raw/demosaic/xtrans/hex_lookup.rs:127-135` (`assert!(entry.dy != isize::MAX, "Unfilled hex entry…")`), and `io/raw/demosaic/xtrans/markesteijn/hex_table.rs:76-78` (`is_green` reads `color_at(col % 3, row % 3)`). The pattern reaches here from untrusted input: the FITS header in `io/image/fits/metadata/mod.rs:423-455` (`XTRNROW0..5`), serde `Deserialize`, and LibRaw.
- **Category:** correctness
- **Impact:** medium. The type's doc says "Only a layout the demosaic can work on exists", and that is false. Two things the demosaic relies on are never checked: greens repeating with period 3 (`is_green`, `right_shift`, `sgrow`/`sgcol`), and dcraw's hex construction finding a trigger in every 3×3 cell. A crafted or unusual header either panics in `HexLookup::new`, which is an `assert!` on untrusted data and against the error rules, or demosaics with wrong green positions without any error.
- **Confidence:** confirmed. A random local search (scratchpad `scripts/xt2.py`, which mirrors the `new()` rules and dcraw's `allhex` trigger) found 294 layouts that pass `XTransPattern::new`. None had period-3 greens, and 257 of them leave hex entries unfilled, so they hit the assert. Example that passes validation and panics: `[[0,2,1,0,1,0],[1,2,1,0,2,1],[1,0,1,1,1,1],[2,1,2,2,0,1],[1,0,1,1,0,1],[1,1,1,1,2,2]]`.
- **Direction:** Make the validation cover what the demosaic assumes: greens with period 3 (`rows[r][c]==1 ⇔ rows[r%3][c%3]==1`), exactly one solitary green, and a filled hex table. Return a new `XTransPatternError` variant for anything else. Then `HexLookup` can only fail on a logic error. Ideally the hex table is built once inside `XTransPattern::new`, so the check and the table cannot drift apart.

### DMS-3 — RCD's absolute `EPS = 1e-5` in the ratio step skews faint signal by colour
- **Where:** `io/raw/demosaic/bayer/rcd/mod.rs:27` and `:83-104` (`estimate_green`, numerator / (`EPS + c + s`)).
- **Category:** precision
- **Impact:** low to medium. On a flat field of value v, every ratio estimate is `v·8v/(EPS+8v)`.
  - Green at R/B sites comes out low by about `EPS/8` absolute. The colour-difference steps then put R and B at green sites high by the same amount.
  - Plane means: G ≈ −EPS/16, R and B ≈ +EPS/16, i.e. about ±6e-7, or 0.04 ADU at 16 bit.
  - Relative to signal that is about ±EPS/(16v): ±0.6 % at v = 1e-4 (6.5 ADU), and rising sharply below that.
  - The result is a level-dependent magenta/green offset in exactly the faint, dark-subtracted backgrounds (narrowband, short subs) where a science product needs linearity. It also makes the output depend on whether a pedestal is kept, because the step is not offset-invariant.
- **Confidence:** confirmed analytically from the code. The existing test `constant_colour_reconstructs_on_every_phase` hides it by keeping every LPF ≥ 2 (its doc names the `EPS/(2·lpf)` error).
- **Evidence:** librtprocess `rcd.cc:72,183-186` and darktable `rcd.c:76,248-251` use the same `eps`. It is harmless there because inputs are photo-scaled and clipped to [0, 1] (`LIM01`), so the shadows that matter are far above 1e-5 relative. Siril sidesteps it by shifting the data to [0, 65535] by its global minimum before RCD (`siril/src/algos/demosaicing_rtp.cpp:247-265`). That shift is a different offset dependence, not a cure. lumos already has a well-defined fallback for ill-conditioned denominators (the signed blend, `mod.rs:89-103`).
- **Direction:** Drop `EPS` from the well-conditioned branch: pure ratio when `c + s` is far from zero, and route the conditioning test through the existing additive blend, which is continuous and has no 0/0. Alternatively, scale the epsilon to the frame's noise or level. Pin the result with a low-level, flat-field exactness test (v = 1e-5…1e-3, exact per-plane means).

### DMS-4 — The white-balance round trip through demosaic does not return native samples bit for bit
- **Where:** `io/image/cfa/mod.rs:353-383`. The CFA is multiplied by `gains` (R/G, 1, B/G as f32), and the output planes are divided by the same gains.
- **Category:** precision
- **Impact:** low. In f32, `(x·g)/g ≠ x` for some x (an error of 1 ulp). The demosaic otherwise guarantees exact native samples (`rcd_all_patterns_preserve_native_samples…`), but the production path through `CfaImage::demosaic` does not, and every interpolated sample picks up two extra roundings.
- **Confidence:** confirmed (IEEE rounding of a non-power-of-two gain).
- **Direction:** Fold the gains into the kernels (see DMS-8). A tile applies the gain as it copies samples in and the reciprocal as it writes out. Native output samples can then be written from the unbalanced input directly, keeping them exact.

### DMS-5 — No CFA (Bayer/X-Trans) drizzle: every OSC frame is interpolated twice before combining
- **Where:** `pipeline/light_source.rs:381-406` (`prepare` always demosaics). `drizzle/` only takes demosaiced `LinearImage`s; grepping `drizzle/` for CFA turns up only `cfa_type: None` at `accumulator/mod.rs:368`.
- **Category:** precision (missing quality option)
- **Impact:** medium. For well-dithered OSC data, Bayer drizzle drops each photosite only into its own colour plane. That avoids both demosaic interpolation (with its correlated noise and colour artifacts on undersampled stars) and the second, registration interpolation. PixInsight (DrizzleIntegration on CFA), Siril ("CFA drizzle") and DSS ("Bayer drizzle") all offer it as the top-quality OSC path. Drizzle is explicitly in scope per `lumos/AGENTS.md`.
- **Confidence:** confirmed that it is missing. The quality gain is the industry's established result.
- **Direction:** Let the drizzle accumulator take a `CfaImage` plus its `CfaType`, where each input pixel contributes weight only to the plane of its own colour, and run registration on a quick demosaic or a green-only proxy. Keep RCD and Markesteijn as the default path for undithered or few-frame sets.

### DMS-6 — Third-party OSC FITS frames reach the demosaic unbalanced
- **Where:** `io/image/cfa/mod.rs:349-356` balances only when `metadata.camera_white_balance` is set. For FITS that comes only from lumos's own `LUMWB*` keys (`io/image/fits/metadata/mod.rs:98`), so ZWO/QHY/N.I.N.A. FITS never carry it.
- **Category:** precision (direction-decision quality)
- **Impact:** low. The code's own comment says the direction decisions "read a colour cast as structure". Typical OSC sky has R/B at about 0.5–0.7 of G, so Markesteijn's YPbPr homogeneity and RCD's cross-colour gradients see that cast on every astro-camera frame. Siril and PixInsight also debayer unbalanced, so this is parity with them rather than a regression.
- **Confidence:** likely (the effect on quality is unmeasured).
- **Direction:** When no camera WB exists, derive per-colour gains from the frame's per-CFA-colour background (the colour mesh/median is already computed). Only the direction decisions see these gains, since they come back out afterwards. Measure on real OSC data whether this changes star colour or zipper artifacts before adopting it.

---

## Performance

### DMS-7 — RCD's hot loops are scalar, stride-2 and branchy, outside `simd::Isa`, and part of the work is redundant
- **Where:** `io/raw/demosaic/bayer/rcd/tile.rs`.
  - `green` (`:166-225`), `diagonal_directions` (`:229-271`), `opposite_colours` (`:275-328`) and `colours_at_green` (`:332-375`) all step `col += 2` over interleaved planes, and `estimate_green` branches per call.
  - The LPF is computed at every pixel (`:169-180`), but only R/B sites read it. librtprocess computes it at half the sites (`rcd.cc:163-169`).
  - The H-HPF rolls a scalar recurrence (`:148-161`), where librtprocess fills a row buffer and sums it (`rcd.cc:143-156`).
  - The vertical HPF fills a whole tile plane (`:133-141`) where a 3-row ring would do.
- **Category:** performance
- **Impact:** medium. The heavy steps cannot vectorize. What does vectorize depends on the x86-64-v3 build flag that `lumos/AGENTS.md` says may be turned off ("keep every kernel correct and fast without it"), and there is no runtime dispatch here. The tile is exactly 512 KiB (`Tile::bytes`), the L2 of the one machine it was tuned on, and 40 % of each tile's compute is overlap ((128/108)²).
- **Confidence:** likely. Measure it: run the bench `bench_rcd_demosaic_core` 6000×4000 against a build without `target-cpu=x86-64-v3`, and grep the asm of `Tile::green` for packed ops.
- **Direction:** Work on per-phase half-resolution planes (R, B, G1, G2 split once at copy-in). The stride-2 loops then become contiguous and can be written over `simd::Isa`. Compute the LPF at R/B only and keep HPF ring rows. The smaller footprint allows a larger tile and less overlap at the same cache budget. Keep the librtprocess digest test as the oracle.

### DMS-8 — White balance and unbalance are four extra full-frame memory passes around the demosaic
- **Where:** `io/image/cfa/mod.rs:357-383`: one parallel pass multiplies the CFA plane by the gains, then three `par_iter_mut` passes divide each output plane.
- **Category:** performance
- **Impact:** medium. On a 24 MP frame that is about 768 MB of read+write traffic (about 25–40 ms at 20–30 GB/s) against a 125 ms RCD, plus a division per sample. The RCD tile already copies its crop in (`tile.rs:86-92`) and its rows out (`tile.rs:99-114`). Markesteijn reads the frame directly (`green_bounds`, `interpolate_green`, `seed`) and writes in `blend`.
- **Confidence:** likely. Measure the `demosaic` wall time with and without gains on a 24 MP frame.
- **Direction:** Pass the gains into both kernels. Multiply at the tile's input reads and multiply by the reciprocal at its output writes, or write native samples from the unbalanced input (DMS-4). The border fill applies the same gains. That removes the four passes and the per-element divisions.

### DMS-9 — Markesteijn three-pass recomputes 2.1× its output, and its 8-direction RGB buffer alone overflows L2
- **Where:** `io/raw/demosaic/xtrans/markesteijn/mod.rs:46` (`TILE = 96`) and `:75-80` (margin 15 at three passes, so the step is 66 and the compute is (96/66)² ≈ 2.12×; at one pass (96/78)² ≈ 1.51×). `tile.rs:57` makes `rgb` `8·96²·12 B = 884 KB` as AoS `[f32; 3]`.
- **Category:** performance
- **Impact:** low to medium. The choice is measured: 703 ms beats 866 ms at librtprocess's 114. But it is a local optimum of a layout that has to hold every direction's full RGB at once. Exact seams (the margin) are worth keeping, so the remaining lever is the footprint.
- **Confidence:** speculative. Profile the per-stage time and L2 misses on the 26 MP bench.
- **Direction:** Shrink each direction's working set. Compute YPbPr and derivatives row by row from `rgb` instead of storing full planes, and keep only the RGB the final blend reads. Then retune `TILE` per pass count, with separate tiles for one and three passes, so the 15-pixel margin costs less.

### DMS-10 — Nulls are repaired twice per light
- **Where:** `calibration_masters/mod.rs:437` (`image.repair_nulls()` at the end of `calibrate`) and `io/image/cfa/mod.rs:326` (again at the start of `demosaic`).
- **Category:** performance
- **Impact:** low. A frame with nulls (FITS BLANK, or masters with no data) runs every same-colour median twice. The second pass is a no-op unless cosmic-ray rejection changed a neighbour in between.
- **Confidence:** confirmed.
- **Direction:** Choose one owner. Either `demosaic` repairs only nulls not already flagged `REPAIRED`, or calibration's repair is the only one and the demosaic documents that precondition with a `debug_assert`.

---

## Design / simplification

### DMS-11 — The tiled-demosaic driver is duplicated between RCD and Markesteijn
- **Where:**
  - `OutputPlanes` exists twice (`bayer/rcd/tile.rs:37-43`, `xtrans/markesteijn/mod.rs:96-102`).
  - `demosaic_memory` and `workspace_bytes` are line-for-line twins (`rcd/mod.rs:49-67`, `markesteijn/mod.rs:107-125`).
  - Each has its own `tile_starts` (`rcd/mod.rs:71-74`, `markesteijn/mod.rs:91-94`).
  - The `par_iter().try_for_each_init(Tile::new, …)` + cancel + border-fallback block is the same in both (`rcd/mod.rs:135-180`, `markesteijn/mod.rs:130-172`).
  - The border fills are two functions that compute the same thing on a Bayer mosaic. RCD's `bilinear` (`rcd/mod.rs:215-257`) is an unweighted same-colour 3×3 mean with a 5×5 fallback. X-Trans `border::interpolate` (`markesteijn/border.rs:45-77`) is a distance-weighted 3×3 with a growing-radius fallback. On Bayer the distance weights are equal within each colour's neighbour set, so they agree everywhere except the fallback.
- **Category:** simplification / design
- **Impact:** medium. Two copies of the unsafe ownership contract and the memory accounting can drift apart, and the border policy differs between sensors for no stated reason.
- **Confidence:** confirmed.
- **Direction:** One generic tiled driver: a trait or struct for the kernel tile with `bytes()`, `margin`, `border`, `demosaic(place, out)`, plus a shared `OutputPlanes`, tile scheduling and memory accounting. Add one CFA-generic border fill over `CfaType::color_at`, taking the weighted 3×3 form and the radius fallback.

### DMS-12 — Test gaps: no signed or low-SNR inputs, no flux/colour-bias check, and several "runs/threshold" tests
- **Where:**
  - `bayer/tests.rs:393-488`: `rcd_gradient_image_green_smoothness` asserts `g > prev_g - 0.05`, and `rcd_sharp_edge_no_excessive_artifacts` asserts `> 0.7` / `< 0.3` / `< prev + 0.15`.
  - `xtrans/hex_lookup.rs:155-249`: the tests check `|offset| ≤ 3` and "is green", not the table.
  - `xtrans/markesteijn/tests.rs:139-156`: finite-only for the ramp.
  - No test of either kernel on zero-mean or negative noise, and none of per-plane mean preservation on a noisy flat at low level.
- **Category:** design (test quality)
- **Impact:** medium. DMS-1 and DMS-3 both sit exactly in the gap, and the user's rules ask for exact, hand-computed expectations rather than thresholds.
- **Confidence:** confirmed.
- **Direction:**
  - Replace the threshold tests with exact expectations: a linear ramp is reproduced exactly inside the border, and edge pixels have exact values (`rcd_interpolates_along_an_edge` already does this).
  - Assert the full `allhex` table for the fixture pattern against dcraw's values.
  - Add a table-driven test over levels {−1e-4, 0, 1e-5, 1e-4, 1e-2} on a flat field with symmetric ± noise: each output plane's mean equals the input mean within a stated bound, for both kernels.

### DMS-13 — Sensor-layout types live inside the demosaic algorithm modules, and colour indices are bare integers
- **Where:**
  - `CfaPattern` is in `io/raw/demosaic/bayer/mod.rs` (imported via `demosaic::bayer::CfaPattern` in 32 files), and `XTransPattern` is in `io/raw/demosaic/xtrans/xtrans_pattern.rs`. Both are used by FITS, calibration, defect maps and cosmic rays, and `CfaType` lives in `io/image/cfa/mod.rs`.
  - `BayerImage` shares `bayer/mod.rs` with `CfaPattern`, and `XTransImage` sits in `xtrans/mod.rs`.
  - `CfaPattern::color_at` returns `usize`, while `XTransPattern::color_at`/`CfaType::color_at` return `u8`.
- **Category:** design / style
- **Impact:** low. The layering is inverted (I/O and calibration depend on the demosaic algorithm module for their basic types). That breaks the one-major-struct-per-file rule, and a fixed set (R/G/B) is carried as integers in two widths.
- **Confidence:** confirmed.
- **Direction:** Move `CfaPattern` and `XTransPattern` beside `CfaType` (for example `io/image/cfa/{cfa_pattern.rs, xtrans_pattern.rs}`). Put `BayerImage`/`XTransImage` in their own files. Introduce a `CfaColour` enum (or at least one integer type) for colour indices.

### DMS-14 — `HexLookup` exists only to be flattened by `HexTable`, and the asymmetries around it
- **Where:**
  - `xtrans/hex_lookup.rs` builds `(dy, dx)` offsets that only `markesteijn/hex_table.rs:24-37` consumes, and it is not under `markesteijn/` although nothing else uses it.
  - Its comments narrate code (`:64, :79-80, :100-101, :107-109, :115-116, :127`), and `let g` is recomputed inside the `d` loop (`:83`).
  - `xtrans::demosaic` (`xtrans/mod.rs:27-44`) is a wrapper that only adds a timing log, with no Bayer counterpart.
  - `XTransImage::new` lacks the finite-data `debug_assert` that `BayerImage::new` has (`bayer/mod.rs:153-156`).
- **Category:** simplification / style
- **Impact:** low.
- **Confidence:** confirmed.
- **Direction:**
  - Build the hex table directly in `HexTable::new` (or in `XTransPattern`, per DMS-2) and delete `HexLookup`/`HexOffset`.
  - Trim the narration.
  - Call `markesteijn::demosaic` from `CfaImage::demosaic` the same way RCD is called.
  - Give `XTransImage::new` the same finite `debug_assert`.

---

## Checked and found OK

- **Pipeline order** (`pipeline/light_source.rs:381-406`, `calibration_masters/mod.rs:383-439`): load CFA → bias/dark subtract → flat divide (normalized per CFA colour, `prepared_flat/mod.rs:16-31`) → defect-map correction → null repair → cosmic-ray rejection on CFA → demosaic → detect/register/stack. This matches Siril, PixInsight, APP and DSS.
- **Negatives:** they are not clamped. lumos drops librtprocess's input `LIM01` and output `max(0)` (`rcd.cc:124,306-308`). The signed-denominator blend in `estimate_green` is continuous at the switch and has no 0/0 (tests in `rcd/tests.rs:17-86`). The RCD ratio's second-order noise bias cancels on symmetric neighbourhoods: the neighbour green enters both LPFs with weight 0.5, and the cross-derivative vanishes at c = s.
- **RCD algorithm:** the step order, stencils, `intp` argument order (equivalent to librtprocess's reversed `intp(a, H, V)`), `EPSSQ` floors and border width (10, same as darktable's `RCD_BORDER`) match. The step 4.1 deviation is a genuine fix. librtprocess's half-width index reads the HPF at column c+1 for even-column sites and c−1/c+2 for odd ones (`rcd.cc:216-218`), so off the diagonal. lumos reads the true diagonal neighbours (`tile.rs:255-269`).
- **Markesteijn:**
  - The YPbPr coefficients follow librtprocess's scalar path. Its SSE path swaps the green and blue luma weights (`markesteijn.cc:648`: `zd6780v * bluev + zd0593v * greenv`), a librtprocess bug that lumos does not inherit.
  - It fixes dcraw's one-pass 2×2-green-block fill (LibRaw issue 441).
  - The `u8` homogeneity sums cannot overflow (≤ 225), and the blend divisor is never zero.
  - The CIELab option is dropped for a stated reason.
- **Tiling:** RCD tiles start on even offsets (Bayer phase kept). The last tile is always ≥ 21 px, and ownership is disjoint at `TILE − 2·border`. Seams are bit-exact, and that is proven by tests (`rcd_beyond_the_border_matches_a_larger_frame`, `markesteijn_inside_the_border_matches_a_larger_frame`, the poison and margin tests). Odd sizes and frames smaller than the tile fall back to the border fill.
- **Memory accounting:** with `try_for_each_init`, at most one tile is live per worker, as `workspace_bytes` charges.
- **Flags:** flags are dilated by the measured demosaic support, and `NO_DATA` is kept at its own extent after repair.

## Suggested batches

1. **Signed-data correctness:** DMS-1 + DMS-3 + the low-level/signed tests of DMS-12. These are small kernel edits pinned by new exact tests, and the librtprocess digests stay valid.
2. **X-Trans pattern contract:** DMS-2 + DMS-14 (build the hex table inside the validated pattern, delete `HexLookup`, return errors for unsupported layouts).
3. **Shared tiled driver and gains in-kernel:** DMS-11 + DMS-8 + DMS-4 (one driver, one border fill, gains applied at tile input and output).
4. **RCD vectorization:** DMS-7, then retune `TILE`. Optionally DMS-9 for Markesteijn.
5. **Layout cleanup:** DMS-13 (move the pattern types, one struct per file, a colour enum).
6. **Separate features:** DMS-5 (CFA drizzle), DMS-6 (balance gains for FITS OSC), and DMS-10 (single null repair) as a one-line follow-up.
