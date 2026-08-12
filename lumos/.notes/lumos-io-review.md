# lumos/src/io review

Findings only — each item describes what is there, not what to do about it. **Delete an item once you
have addressed it**; this file lists open findings and nothing else. No "done" markers, no history.

Items are anchored to symbol names rather than line numbers, which go stale within a session.

Scope: `lumos/src/io` production code — `image/` (incl. `fits/`) and `raw/` (incl. `demosaic/`,
`normalize/`). Test bodies and the APIs tests reach through were not reviewed. Where a finding turns
on what comparable software does, the reference is named and quoted.

References consulted: LibRaw `src/demosaic/xtrans_demosaic.cpp`, RawTherapee
`rtengine/rcd_demosaic.cc` and `rtengine/xtrans_demosaic.cc`, dcraw's `scale_colors` →
`*_interpolate` ordering, the FITS Standard 4.0 §5/§6.3 and the NOST floating-point agreement, and
Siril's FITS orientation documentation.

---

## FITS orientation is corrected for the CFA phase but not for the rows, and not for height parity

`read_bayer_cfa` flips the Bayer pattern on `ROWORDER = BOTTOM-UP`. The pixel rows are never
reordered, and the flip is applied without consulting `NAXIS2`.

- [ ] `pattern.flip_vertical()` runs unconditionally on `BOTTOM-UP`. Under the convention Siril
      documents — "the usual **RGGB** Bayer pattern becomes **GBRG** if the image is upside-down",
      i.e. `BAYERPAT` describes the top-down image — file row `f` corresponds to displayed row
      `H-1-f`. That is a phase flip only when `H` is **even**; for odd `H` the phase is unchanged and
      flipping inverts it. Odd visible heights are real (the LibRaw report for the EOS 1500D/200D
      gives 4015), and the failure mode is a fully mis-debayered frame, not a subtle one.
- [ ] Rows are left in file order, which matches Siril's rule that "`ROWORDER` shall not be used to
      unflip the image data for stacking", but nothing records the row order on the decoded image and
      nothing checks it is consistent across a stack. A `BOTTOM-UP` and a `TOP-DOWN` frame of the
      same target load as vertically mirrored images; star-pattern registration over a similarity
      transform cannot align a mirrored field, and the failure surfaces far from here.
- [ ] `CfaPattern::from_bayerpat` maps the string `"TRUE"` to `Rggb`. There is no comment saying
      which writer emits that or why RGGB is the right guess for it, and the mapping silently
      succeeds — a wrong guess here is a mis-debayered frame with no diagnostic.
- [ ] The `ROWORDER` flip is applied before the `XBAYROFF`/`YBAYROFF` shifts, and nothing states
      which frame the offsets are expressed in. `write_cfa_metadata` emits `ROWORDER` only for the
      `Bayer` and `XTrans` arms, so a `Mono` CFA FITS written by `save_cfa_fits` carries no row-order
      declaration at all.

## RCD's interpolated region is one pixel wider than the border it overwrites

`BORDER = 4` in `io/raw/demosaic/bayer/rcd`. RawTherapee's `rcd_demosaic.cc` uses
`constexpr int rcdBorder = 9` and calls `border_interpolate(W, H, rcdBorder, ...)`. The gap is not
cosmetic: at exactly `BORDER` the direction buffers are read where they were never written.

- [ ] `vh_dir` is written only for `ry ∈ [BORDER, rh-BORDER)` and `rx ∈ [BORDER, rw-BORDER)`; the rest
      of the buffer keeps its `vec![0.0; npix]` initialization. `avg4_diag(&vh_dir, idx, w1)` in the
      green step reads `(ry±1, rx±1)`, so at `ry == BORDER` two of the four terms are the
      never-written zeros, and likewise at `rx == BORDER` and at the far edges.
- [ ] The corrupted average is not merely averaged in — it decides. `vh_disc` picks
      `vh_neighbourhood` whenever `|0.5 - vh_central| < |0.5 - vh_neighbourhood|`, and zeros drag the
      neighbourhood away from 0.5, which makes the contaminated value the one selected.
- [ ] `pq_dir` has the same shape: written for `ry ∈ [BORDER, rh-BORDER)` at non-green sites only,
      read through `avg4_diag(pq_dir, ...)` in `process_step4_2_row`, and zero-filled outside.
      `step4_3_rb_at_green` reads `vh_dir` the same way.
- [ ] `border_interpolate` fills `ry < BORDER` and `ry >= height - BORDER` (and the matching column
      bands), so the ring at exactly `BORDER` survives into the output. On the `CfaImage::demosaic`
      path — the calibrated science path — `margin` is `Vec2us::ZERO`, so the raw and active extents
      coincide and the ring always lands inside the delivered image. On the `load_raw` path it lands
      in the output only when `top_margin < BORDER`.
- [ ] RawTherapee additionally clamps its RCD input (`cfa[indx] = ... LIM01(rawData[row][col] /
      scale)`). This module deliberately feeds unclamped, possibly negative calibrated samples in, and
      `estimate_green`'s `MIN_SIGNED_DENOMINATOR_RATIO` blend is a local invention for that case. The
      blend is continuous at both ends, but it has no reference to be cross-checked against and the
      tests exercise it only against itself.

## Demosaic runs on non-white-balanced data, unlike the pipelines the kernels are ported from

`camera_white_balance` is parsed, canonicalized, round-tripped through FITS, and applied nowhere.

- [ ] `ImageMetadata::camera_white_balance` is documented "Metadata only: RAW decoding and
      calibration keep unity white balance", and `rg camera_white_balance` outside `io/` finds no
      reader in `lumos`, `darkroom`, or `lens`. `canonical_camera_white_balance`,
      `read_camera_white_balance`, and the `LUMWBR`/`LUMWBG1`/`LUMWBB`/`LUMWBG2` keywords exist to
      carry a value nothing consumes.
- [ ] Both reference pipelines white-balance before interpolating. dcraw and LibRaw run
      `scale_colors()` — which applies `pre_mul`/`cam_mul` — ahead of `*_interpolate()`; RawTherapee
      applies it in `RawImageSource::preprocess` before `demosaic()`, and RawTherapee issue #5616
      documents that which white balance is applied beforehand visibly changes demosaic output
      ("green artifacts" under one choice, "much less" colour shift under another).
- [ ] Both kernels here are the kind that the choice affects most: RCD is ratio-corrected
      (`estimate_green` divides one channel's low-pass by another's), and Markesteijn's homogeneity
      decision runs on chroma derivatives (`rgb_to_ypbpr`'s Pb/Pr terms). Feeding them a frame where
      R and B sit 1.5–2× below G — the normal state of an unbalanced astro sub — moves both the ratio
      estimates and the direction selection away from the regime the coefficients were tuned in.
- [ ] Nothing in the module records that the samples are un-balanced in a way a later stage could act
      on; `ColorProvenance::SensorRgb` says only that the channels are sensor-native.

## The Markesteijn steps that are not parallel are the ones that cover the whole image

Every other stage in `markesteijn_steps` is a rayon pass. Two are not, and both are O(pixels).

- [ ] `score_homogeneity` is fully serial: `for d in 0..NDIR` around `build_summed_area_table` (a
      sequential prefix scan over `width × height`) and a nested `for y`/`for x` query loop. For a
      6032×4028 X-Trans frame that is 4 × 24.3 M SAT writes plus 4 × 24.3 M `sat_query` calls on one
      core, inside a function whose module doc targets "<500ms for 6032×4028".
- [ ] `demosaic_border` walks `for y in 0..height { for x in 0..width { if interior { continue } } }`
      serially — 24 M iterations to touch a ring of roughly 8 × 2 × (W + H) pixels. The RCD border
      pass in the same module solves exactly this and says so: "Only iterate actual border pixels:
      top/bottom bands + left/right edges. This avoids walking the entire image just to skip interior
      pixels."
- [ ] `blend_final` computes the full interpolated result for every pixel including the border ring,
      then `demosaic_border` overwrites the outer 8 pixels. LibRaw's `xtrans_interpolate` bounds its
      main loops and calls `border_interpolate(8)` for the rest.

## The Bayer path materializes the buffer the X-Trans path was restructured to avoid

The two RAW paths solve the same problem — normalize, correct black, demosaic — with opposite memory
strategies, and only one of them is documented as deliberate.

- [ ] `demosaic_bayer` calls `normalize_u16_to_f32_parallel` over the **whole raw buffer** (margins
      included), producing a `raw_pixels × 4` byte plane, then runs `apply_bayer_black_corrections`
      over it as a second full pass. `XTransImage::read_normalized` does the same arithmetic
      per-sample inside the kernel, and `process_xtrans` documents the reason: "Normalization happens
      on-the-fly during demosaicing, avoiding a separate P×4 byte f32 buffer" / "saves ~47 MB".
- [ ] `demosaic_xtrans` takes `self` by value specifically to drop libraw's ~77 MB before allocating
      its arena. `demosaic_bayer` takes `&self`, so libraw's buffer, the normalized f32 copy, and
      RCD's 6P working set are all live at once.
- [ ] `apply_bayer_black_corrections` recomputes `raw_filter_color(...)` and
      `BlackRepeat::at_raw(...)` — four modulo operations and a shift — per pixel over the full raw
      buffer, including the masked margins that are cropped away at the end of `rcd::demosaic`.

## Trust-boundary handling flips between error and panic inside single functions

The module states its own rule and then breaks it a few lines later.

- [ ] `demosaic_libraw_fallback` returns `raw_err` for a bad geometry with the comment "This is the
      trust boundary: reject a geometry `ImageDimensions` cannot hold rather than assert on it", then
      `.expect("libraw: image dimensions overflow")` on the pixel count and `assert!(data_size >=
      expected_size, ...)` on libraw's reported buffer size — release asserts on values that came
      from the same external library across the same boundary.
- [ ] `open_raw` validates both X-Trans patterns via `validate_xtrans_pattern` before accepting the
      file; `raw_cfa_frame_info` classifies the same file as `DemosaicKind::XTransMarkesteijn`
      without that validation, so `peek_dimensions` can promise a demosaic that the later
      `XTransPattern::new` in `process_xtrans` refuses.
- [ ] `StackableImage::peek_dimensions` for `CfaImage` is `CfaFrameInfo::from_file(...).ok()`, which
      collapses cancellation, I/O failure, an unreadable header, and an unsupported sensor into one
      `None`.
- [ ] The 8-bit arm of `demosaic_libraw_fallback` (`/ 255.0`, plus its own `assert!`) is unreachable:
      the same function sets `params.output_bps = 16` immediately above.
- [ ] `demosaic_libraw_fallback` normalizes by `65535.0` while every other path normalizes by
      `1/(maximum - black)`. These agree only because libraw's own `scale_colors` rescales to the
      full 16-bit range first — an undocumented dependency on internal libraw behaviour that
      `output_color = 0` and `user_mul = [1.0; 4]` are already carefully steering.
- [ ] `checksum_state` reaches `unreachable!("invalid FITS checksum is rejected before provenance is
      constructed")`, which holds only because `verify_selected_checksum` returns early on `Ignore`
      and errors on `Invalid` under both other policies. The invariant lives in a different function
      from the panic that depends on it.

## Metadata is parsed, written, and never read

- [ ] `ImageMetadata::header_dimensions` has zero readers anywhere in the workspace, and its two
      producers disagree on axis order: `load_raw` writes `[height, width, channels]` (C order) and
      `read_decoded_hdu` passes `plan.shape`, which is `Header::axes()` = `[NAXIS1, NAXIS2, ...]` =
      `[width, height, ...]` (FITS order). Anything that ever starts reading it gets transposed
      geometry depending on which decoder ran.
- [ ] `read_metadata` populates 25 fields; `write_image_metadata` emits 21 keywords. Outside `io/`
      and tests, essentially only `cfa_type`, `image_type`, `filter`, `exposure_time`, and
      `calibrated` have readers. `egain` is annotated "Used for noise modeling" and has none.
- [ ] `read_ra_deg` returns `Ok(parse_sexagesimal(value).map(...))` when `OBJCTRA` is present, so an
      unparseable `OBJCTRA` yields `None` instead of falling through to `CRVAL1`, which the same
      function reaches only when the keyword is absent. `read_dec_deg` has the same shape.
- [ ] `read_cfa_hdu` builds a `LoadContext::default()` internally, so the caller's cancel token and
      memory limit do not apply to it — the one entry point in `fits::decode` that ignores the policy
      object every other entry point threads through.

## Structure diverges from the workspace's own module rules

- [ ] `io/raw/mod.rs` is 1200 lines carrying ten top-level types — `LibrawState`,
      `ProcessedImageGuard`, `BlackRepeat`, `BlackLevel`, `RawActiveArea`, `ChannelBlackDelta`,
      `UnpackedRaw`, `LibrawDemosaiced`, `DecodedRawPreview`, `DemosaicedPixels` — against "One major
      struct, one file, same name". `BlackRepeat` is `pub(crate)` purely so
      `demosaic/xtrans/mod.rs` can reach `crate::io::raw::BlackRepeat`, a child module importing from
      its grandparent.
- [ ] Errors are not in `error.rs` where the rule puts them: `XTransPatternError` sits in
      `demosaic/xtrans/mod.rs`, `DemosaicError` and `Cancelled` in `demosaic/mod.rs`. `raw/error.rs`
      and `image/fits/error.rs` exist, so the convention is established and then not followed.
- [ ] The module's surface is `pub(crate)` free functions rather than methods, against "Avoid
      `pub`/`pub(crate)` free fns": `raw::{load_raw, load_raw_cfa, raw_cfa_frame_info, raw_err}`,
      `fits::decode::{load_linear_fits, load_preview_fits, load_cfa_fits, read_cfa_hdu,
      fits_cfa_frame_info}`, `fits::{fits_err, fits_unsupported, fits_to_io}`,
      `fits::cfa::save_cfa_fits`, and all five of `image::standard`'s. Several take a type the module
      owns as their first parameter.
- [ ] `demosaic/xtrans/mod.rs` holds `XTransPattern`, `XTransImage`, `PixelSource`,
      `XTransPatternError`, and the two `process_xtrans*` free functions; `demosaic/bayer/mod.rs`
      holds `CfaPattern` and `BayerImage`; `image/cfa/mod.rs` holds `CfaType`, `CfaFrameInfo`, and
      `CfaImage`. Same rule as the first item.
- [ ] `CfaImage::demosaic` has `use` statements inside two match arms, one of which
      (`use ...xtrans::process_xtrans_f32;`) imports a free function unqualified against "Free fns
      stay namespace-qualified". `markesteijn::demosaic` has `use std::time::Instant;` in its body.
- [ ] `rgb_to_ypbpr` returns `(f32, f32, f32)` and `compute_ypbpr_row` fills a
      `&mut [(f32, f32, f32)]`, against "No tuple returns — name a result struct". The sibling
      `solitary_green_candidate` does name one (`SolitaryGreenCandidate`).
- [ ] Five `#[allow(clippy::too_many_arguments)]` in the module (`process_xtrans` at 9,
      `opposite_color` at 10, `green_block_colors` at 8, `blend_final` at 9,
      `XTransImage::with_margins` at 8) — the lint is being silenced rather than the parameter
      bundles named. `#[allow(clippy::unnecessary_cast)]` on `xtrans_pattern_from_libraw` carries no
      comment explaining that `c_char` signedness is platform-dependent.
- [ ] `process_step4_2_row` is an `unsafe fn` with no `# Safety` section stating the contract its
      callers must uphold; the invariant is only in prose at the call site.

## Smaller items

- [ ] `step4_2_rb_at_opposing`'s `_ => return, // G-row → skip` arm is unreachable. `col_start` is
      `BORDER + (color_at(0, ry) & 1)` with `BORDER` even, which lands on the row's non-green site for
      every row, so `color` is always 0 or 2. The surrounding comments ("R-rows write B, B-rows write
      R", "G-row → skip") describe a scheme the code does not implement — it processes every row at
      its non-green sites, which is what RCD requires.
- [ ] `read_fits_plane` calls `read_image_section`, which allocates a fresh `Image` per chunk, then
      `physical_f32()` allocates a second `Vec`, then `copy_from_slice` copies into `output`.
      `fits_well` exposes `read_image_section_view`, described as the "Scratch-reusing counterpart",
      and it is unused. `preflight_fits_image` budgets `native_chunk_bytes` twice plus
      `physical_chunk_bytes` for exactly these temporaries, with no comment saying why twice.
- [ ] `dimensions_from_shape` rejects any rank other than 2 or 3, so a conforming image with
      degenerate trailing axes (`NAXIS = 4` with `NAXIS3 = NAXIS4 = 1`, routine in radio and some
      camera outputs) fails to load. The `[width, height, 1]` case is special-cased at rank 3 only.
- [ ] `FitsHduSelector::Auto` rejects any file with more than one image-bearing HDU. A primary image
      plus a bad-pixel-mask or uncertainty extension — the normal shape of a calibrated science
      product, and what the mission statement's "ancillary per-pixel quality planes" would produce —
      cannot be opened without the caller naming an index or `EXTNAME`.
- [ ] `rgb_to_ypbpr`'s coefficients reproduce RawTherapee's `xtrans_demosaic.cc` exactly
      (`0.2627/0.6780/0.0593` luma, `0.56433` Pb, `0.67815` Pr), including RawTherapee's own
      inconsistency: the Pr factor `0.67815` is `1/(2(1-K_r))` for BT.2020, but the Pb factor
      `0.56433` is `1/1.772`, BT.601's, where BT.2020 would give `0.53152`. Nothing in the module
      records that the constants are inherited verbatim or that LibRaw's `xtrans_demosaic.cpp` makes
      a different choice entirely — CIELab, with `g * 500 / 232` and `g * 500 / 580` cross-terms
      coupling the luma derivative into the chroma ones. The module doc says "perceptual
      derivatives", which is true of the LibRaw form and not of this one.
- [ ] `CfaPattern::from_bayerpat` allocates a `String` via `to_uppercase()` on every call to compare
      against four fixed literals, where `eq_ignore_ascii_case` is used for the same job three
      functions away in `read_cfa_from_headers`.
- [ ] `normalize/simd` dispatches SSE4.1 → SSE2 → NEON → scalar with no AVX2 tier, while
      `lumos/CLAUDE.md` advertises "hand-written SIMD (AVX2 / SSE4.1 / NEON) hot paths". The
      conversion is memory-bound, so the omission may be deliberate; nothing says so.
- [ ] `LibrawState`'s doc explains at length that it owns "the file bytes it parses in place" because
      `libraw_open_buffer` leaves libraw pointing into `buf`. On unix `open_libraw_input` always
      returns `Ok(None)` — the `buf` field is only ever populated on the `#[cfg(not(unix))]` path.
</content>
</invoke>
