# Review: RAW decode path (lumos `io/raw`, `libraw-sys`, decode-side image types)

Scope: `lumos/src/io/raw/{mod.rs, black_level/, sensor_layout.rs, raw_files/, quality_report.rs, error.rs, bench.rs}`,
`libraw-sys/{build.rs, src/lib.rs, shim/internal.cpp}`, and the decode-side types in `lumos/src/io/image/`
(`cfa/`, `sample_domain.rs`, `pixel_flags.rs`, `image_provenance.rs`, `linear.rs`, `linear_pixels.rs`,
`load_context.rs`). Demosaic internals were not reviewed. LibRaw is the vendored 0.22.2.

Paths below: `lumos/…` and `libraw-sys/…` are relative to `/home/xxorza/Projects/darkroom`; `LibRaw/…` is
`libraw-sys/LibRaw/…`; reference projects are under `/home/xxorza/Projects/darkroom/.tmp/`.

`cargo test -p lumos --tests --features ml io::raw`: 54 passed.

---

## Findings

### RAW-3 — Four-colour sensors (RGBE, CMYG) are classified as Bayer RGB
- **Where:** `lumos/src/io/image/cfa/mod.rs:87-101`; `lumos/src/io/raw/demosaic/bayer/mod.rs:57-88`
- **Category:** correctness
- **Impact:** medium. These cameras are rare (Sony DSC-F828, Nikon Coolpix E900/E950/E990/E4500/E5000 CMYG), but they get demosaiced as RGB with the wrong filters, which is quietly wrong output.
- **Confidence:** confirmed (by hand-evaluating `from_filters` on LibRaw's words)
- **Evidence:** `from_libraw` sends any 2-row-periodic `filters` to `CfaPattern::from_filters`, which counts
  colour index 3 as green and ignores `colors` and `cdesc`. The F828 is `filters = 0x9c9c9c9c, colors = 4,
  cdesc "RGBE"` (`LibRaw/src/metadata/identify.cpp:2956-2965`), which decodes to R,E/G,B and is accepted as
  `Rggb`, so emerald is treated as green. The Nikon CMYG table entries (`identify.cpp:406-416`, cf `0x1e`, `0x4b`,
  `0xe1`, `0xb4`) get `colors = 4` (`identify.cpp:752`). `0x1e` reads as BGGR, with cyan as blue and green as
  red. The test (`cfa/tests.rs:441-469`) never passes `colors = 4`.
- **Direction:** Accept Bayer only for `colors == 3` with `cdesc == "RGBG"` (read `idata.cdesc`). Everything else
  goes to `None`, and the LibRaw fallback then refuses `colors == 4` clearly (`mod.rs:437`).

### RAW-4 — The quantization σ claims a 1-ADU step for lossy codecs that bypass LibRaw's `curve`
- **Where:** `lumos/src/io/raw/mod.rs:184-187` (doc), `mod.rs:640-646`, `mod.rs:235-240`
- **Category:** precision
- **Impact:** medium. For Canon C-RAW (CR3 lossy) and Fuji lossy-compressed RAF, `quantization_sigma` is several times too small. It feeds the defect-map residual floor (`calibration_masters/defect_map/mod.rs:355-363`) and the σ the master carries (`combine/stack/quantization.rs`).
- **Confidence:** confirmed for the code paths (`crx.cpp` and `fuji_compressed.cpp` never touch `curve`). The size of the error is likely, not measured.
- **Evidence:** `linear_curve` checks only that `color.curve` is the identity. The doc comment says Canon C-RAW
  maps its codes through that curve. It does not: `LibRaw/src/decoders/crx.cpp` has no reference to `curve`,
  and its lossy wavelet mode is `encType == 3` (`crx.cpp:1673`, `2733-2760`). Fuji lossy RAF is quantized through
  q-tables when `fuji_lossless == 0` (`LibRaw/src/decoders/fuji_compressed.cpp:93-118, 152-166`), also without
  `curve`. Phase One `phase_one_load_raw_c` stores `pixel << 2`
  (`LibRaw/src/decoders/load_mfbacks.cpp:708`), so its step is 4 ADU while the claim is 1.
- **Direction:** Add shim accessors for the codec facts LibRaw keeps internally (`unpacker_data.fuji_lossless`,
  the CRX header `encType`/`imageLevels`, `is_phaseone_compressed`). Return `None`, or the true step, for those
  codecs. Fix the doc comment.

### RAW-5 — Canon frames never carry a temperature, so dark matching is always "unverified" for them
- **Where:** `lumos/src/io/raw/mod.rs:624-627`; consumer `lumos/src/calibration_masters/mod.rs:450-455`
- **Category:** correctness (needs a design call)
- **Impact:** medium. On the most common astro DSLR brand, the dark/light temperature check never runs.
- **Confidence:** confirmed (LibRaw source)
- **Evidence:** Only `makernotes.common.SensorTemperature` is read. Canon's value goes to `CameraTemperature`
  (`LibRaw/src/metadata/canon.cpp:190-198, 835`). Sony and Olympus fill `SensorTemperature`
  (`sony.cpp:1034`, `olympus.cpp:98-112`). Kodak, Leica, Samsung and Pentax fill `CameraTemperature`. Siril and DSS
  record no temperature from RAW at all.
- **Direction:** Decision for the user. The options:
  - Keep sensor-only and document that Canon has none.
  - Fall back to `CameraTemperature`, recorded as a distinct quantity, since a body sensor is not the die.
  - Add a separate `camera_temp` field that calibration uses only for matching.

### RAW-6 — Phase One compressed IIQ: the black is never subtracted, but the frame declares `Pedestal::Removed`
- **Where:** `lumos/src/io/raw/mod.rs:256-268, 274-305`
- **Category:** correctness
- **Impact:** low in practice (medium-format backs are rare in astro), but it is quietly wrong: the frame carries a `t_black` pedestal while claiming none, and skips LibRaw's per-row and per-column black and `phase_one_correct`.
- **Confidence:** confirmed (code reading)
- **Evidence:** For `phase_one_load_raw{,_c,_s}`, LibRaw leaves `color.black = 0`, `maximum = 0xffff`
  (`LibRaw/src/metadata/mediumformat.cpp:336`). It subtracts
  `phase_one_data.t_black` plus `rawdata.ph1_cblack/ph1_rblack` only in `raw2image`
  (`LibRaw/src/preprocessing/raw2image.cpp:72-85`, `LibRaw/src/utils/phaseone_processing.cpp:40-90`). lumos reads
  `raw_image` directly. DSS special-cases `is_phaseone_compressed()`
  (`dss/DeepSkyStackerKernel/RAWUtils.cpp:679`).
- **Direction:** Detect it through a shim `is_phaseone_compressed`, then either refuse it or apply the `ph1` black
  from the public `rawdata.ph1_cblack/ph1_rblack` and `color.phase_one_data`.

### RAW-7 — `linear_max` is trusted without bounds when choosing the saturation level
- **Where:** `lumos/src/io/raw/mod.rs:630-639`
- **Category:** correctness
- **Impact:** low to medium. A maker-note `linear_max` below black flags every pixel SATURATED, and one far above `maximum` flags none. LibRaw documents the field as informational and already patches several known bit-depth mismatches itself, so the field is known to arrive wrong.
- **Confidence:** likely (no failing file found; the risk follows from LibRaw's own fixes)
- **Evidence:** LibRaw repairs `linear_max` scale per maker: Sony `/4` (`LibRaw/src/utils/open.cpp:729-734`),
  Canon `/div` (`open.cpp:763-773`), Olympus `*4` (`identify.cpp:2286-2292`), Panasonic `-64`/`-16`
  (`tiff.cpp:167-171`). darktable uses `linear_max[0]` directly as the white point
  (`darktable/src/imageio/imageio_libraw.c:438-442`).
- **Direction:** Use `linear_max[c]` only when `black_c < linear_max[c] ≤ maximum`, otherwise use `maximum`.
  Pin the rule with a table test.

### RAW-8 — The 95 % saturation threshold applies even where the file states the true clip
- **Where:** `lumos/src/io/image/pixel_flags.rs:8-12`; `lumos/src/io/raw/mod.rs:632-639`
- **Category:** precision (needs a design call)
- **Impact:** low to medium. Unsaturated samples between 95 % and 100 % of span are flagged as lower bounds and lose their measurement status. The doc claim "stays above any unsaturated star core" does not hold for a linear sensor.
- **Confidence:** likely
- **Evidence:** The 95 % margin exists for cameras whose `maximum` overstates the clip (LibRaw's `adjust_maximum`
  rationale). When `linear_max` is present (Canon `SpecularWhiteLevel`, Sony SR2, Panasonic linearity limits), the
  clip is stated outright. RawTherapee uses per-camera, per-ISO white levels (`camconst.json`). darktable clips at
  the stated white point.
- **Direction:** Decision for the user. One option is a stated-clip path: `linear_max` minus a small stated ADU
  margin, keeping 95 % only for `maximum`. The other is to keep 95 % everywhere and correct the doc comment.

### RAW-12 — The decode hot path makes two passes over the raw buffer, and the second divides per pixel
- **Where:** `lumos/src/io/raw/mod.rs:212-233` (`decode_flags`) and `:274-305`;
  `lumos/src/io/raw/black_level/mod.rs:204-241`; `lumos/src/io/image/pixel_flags.rs:110-121, 294-305`
- **Category:** performance
- **Impact:** low to medium. Per frame this costs:
  - A second full read of the raw `u16` buffer.
  - One `usize` `%` and `/` per pixel in `decode_flags`.
  - A 1 B/px plane that is always allocated and zero-filled, even when nothing is flagged.
  - A serial `counts_of` pass over that plane.
  - In `normalize`, a per-pixel `match` on the CFA type, an `Option` branch on the repeat, and two `%` when a repeat exists.

  This is small next to LibRaw's own unpack, but it is pure overhead that one pass removes.
- **Confidence:** confirmed (code); size not measured
- **Direction:** Fuse into one row-parallel pass that writes the `f32` sample and the flag byte together. Before
  the pass, build per-row tables over the CFA period (2 or 6) and the repeat period: black, saturation threshold
  and channel. Accumulate flag counts per row and create the plane only once a row flags something. To measure:
  `normalize`+`decode_flags` time against `libraw_unpack` time for a 24 MP CR2 and a 26 MP RAF, at decode
  concurrency 1 and N.

### RAW-13 — LibRaw is built without OpenMP: CR3, Fuji-compressed and Panasonic-v8 decode single-threaded
- **Where:** `libraw-sys/build.rs:35-46`
- **Category:** performance
- **Impact:** low to medium. Frames decode in parallel (`lumos/src/combine/cache/loader/mod.rs:242-254`), so throughput suffers only when `decode_concurrency` is memory-limited or for single-file loads (preview, peek). There the tile-parallel decoders run on one core.
- **Confidence:** confirmed (build flags); impact not measured
- **Evidence:** `LIBRAW_USE_OPENMP` comes only from `_OPENMP` (`LibRaw/libraw/libraw_types.h:48-77`). Tile
  parallelism is behind it in `crx.cpp:2625-2695`, `fuji_compressed.cpp:1156-1177` and `pana8.cpp:81-132`.
  `Makefile.dist:9` leaves `-fopenmp` commented out as well. `cc` passes no `-march` for x86 (`cc-1.6.0` maps no
  `target-cpu`).
- **Direction:** Measure first: unpack time per format at concurrency 1. Enabling OpenMP needs a libgomp/libomp
  link (a system dependency, so it needs approval) and nests with rayon. A cheaper option is to give single-file
  loads more parallel work elsewhere.

### RAW-14 — The build cannot decode several formats that `RAW_EXTENSIONS` and the docs advertise
- **Where:** `libraw-sys/build.rs:8` (only `LIBRAW_NODLL`); `lumos/src/io/raw/mod.rs:41-49, 737-742`
- **Category:** correctness (coverage and honesty)
- **Impact:** low. Nothing decodes wrong, but these formats are refused:
  - Deflate DNG (integer and float). Needs `USE_ZLIB`. `deflate_dng_load_raw` throws: `LibRaw/src/decoders/fp_dng.cpp:328, :479-481`. LibRaw's own `Makefile.dist:25` enables it by default.
  - Lossy DNG and Kodak JPEG. Needs `USE_JPEG`; identify refuses: `identify.cpp:1269-1275`.
  - X3F. Needs `USE_X3FTOOLS`; `parse_x3f` is a no-op: `identify.cpp:688-690`.
  - GPR. Needs the GPR SDK; `dngsdk_glue.cpp:27-33`.

  `RAW_EXTENSIONS` still lists `x3f` and `gpr`, and the `load_raw` doc promises Foveon.
- **Confidence:** confirmed
- **Direction:** Drop `x3f` and `gpr` from the list, and Foveon from the doc, or enable them. Adding zlib (for
  example `libz-sys`) is a new dependency and needs approval.

### RAW-15 — Float DNG is silently quantized to `u16` by LibRaw
- **Where:** `lumos/src/io/raw/mod.rs:513-667` (no `rawparams.options` change)
- **Category:** precision
- **Impact:** low (float DNGs are rare in astro). Reachable through `uncompressed_fp_dng_load_raw` even without zlib.
- **Confidence:** confirmed (code reading)
- **Evidence:** The default `rawparams.options = LIBRAW_RAWOPTIONS_CONVERTFLOAT_TO_INT`
  (`LibRaw/src/utils/init_close_utils.cpp:89`). `convertFloatToInt` (`LibRaw/src/decoders/fp_dng.cpp:500-575`)
  clamps negatives to 0, truncates, and truncates the rescaled black (`unsigned(black * multip)`). lumos then
  claims a 1-ADU quantization step.
- **Direction:** Clear the option and read `rawdata.float_image` directly, with a float-aware domain. Or refuse
  float DNG with a typed error.

### RAW-16 — RAW frames drop camera identity, timestamp and optics that FITS frames carry
- **Where:** `lumos/src/io/raw/mod.rs:279-297, 338-355`; fields at `lumos/src/io/image/image_metadata.rs:13-60`
- **Category:** design
- **Impact:** low to medium. `instrument` (make/model), `date_obs` (`other.timestamp`), `focal_length` and pixel size stay `None`. A calibrated RAW saved as FITS loses them (`fits/metadata/mod.rs:150-193`), and nothing can check that masters come from the same camera.
- **Confidence:** confirmed
- **Evidence:** Siril fills `instrume`, `date_obs`, `focal_length`, `pixel_size`, `aperture`
  (`siril/src/io/image_formats_libraries.c:2186-2221, 2408-2423`).
- **Direction:** Fill the existing fields from `idata.normalized_make/model`, `other.timestamp`, and
  `other.focal_len`. Pixel size only from a stated source, never Siril's sensor-width table guess.

### RAW-17 — Peek (`raw_cfa_frame_info`) can disagree with the load
- **Where:** `lumos/src/io/raw/mod.rs:751-778` compared with `:513-667`
- **Category:** correctness
- **Impact:** low. Peek accepts a frame that the load then refuses or describes differently:
  - A Fuji SuperCCD file passes peek as a CFA frame, but the load refuses it (`:569-577`).
  - Sensors whose `filters` LibRaw changes inside `load_raw` peek with the pre-unpack pattern. Examples: OmniVision/RPi (`LibRaw/src/decoders/decoders_libraw_dcrdefs.cpp:316, 347, 382, 413`; `decoders_dcraw.cpp:1053`) and Pentax 4-shot (`decoders_libraw.cpp:141`).
  - Peek also duplicates the dimension validation.
- **Confidence:** confirmed (code), rare in practice
- **Direction:** Share one `identify`-stage validator between peek and load, and include the SuperCCD refusal. For
  the decoder-time filter changes, document the limit or have peek refuse those decoders.

### RAW-18 — `raw_files` aborts the whole scan on any one entry's `metadata` error, and stats every entry
- **Where:** `lumos/src/io/raw/raw_files/mod.rs:315-320` (`fs::metadata` before the extension test), `:337-339`
- **Category:** correctness
- **Impact:** low. A dangling symlink with any name (for example `notes.txt`) makes the directory unreadable as a light set. Every entry costs a `stat`. Errors are wrapped as message strings.
- **Confidence:** confirmed
- **Direction:** Test the extension first and use `entry.file_type()`, following a link only for RAW-named
  entries. Return a typed error that names the entry.

---

## Checked and found OK

- **RAII drop order:** `LibrawState::drop` runs `libraw_close` before `buf` drops, so LibRaw never reads freed bytes. The failure inside `open` still frees the handle.
- **Geometry validation:** `raw_pitch == 2·raw_width` and the margin bounds are checked before any `from_raw_parts`. LibRaw's `rwidth` over-allocation (`unpack.cpp:70-78`) only makes the buffer larger than the slice.
- **CFA phase after crop:** lumos reads the post-identify visible `filters`. LibRaw re-phases it when it evens the margins (`open.cpp:911-929`). The X-Trans pattern comes from the visible-origin `idata.xtrans` (`identify.cpp:2551`, `open.cpp:891-910`). The second green is channel 3 after identify (`identify.cpp:1283-1285`), as `channel_map` expects.
- **Black fold:** it mirrors `adjust_bl` (`utils_libraw.cpp:468-547`), including the 2×2 index precedence and the X-Trans 1×1 case. The spatial pattern is anchored at the visible origin, as LibRaw's `subtract_black.cpp:47` anchors it.
- **DNG float black:** the `dng_fblack`/`dng_fcblack` replacement matches how `tiff.cpp:1275-1336` stores the scalar and the 2×2 cases. Linear-DNG per-channel blacks only reach the domain (fallback path).
- **Normalization:** one span `maximum − common black` gives one ADU scale for all channels and a single `SampleDomain`. This is better for science than darktable's per-channel `white − black_c` (`darktable/src/iop/rawprepare.c:699-700`). The arithmetic is f64, rounded once to f32, and unclamped in the CFA frame.
- **Saturation timing:** it is decided on the raw value before black subtraction. `zero_is_bad` zeros map to `NO_DATA`, which is LibRaw's `remove_zeroes` intent without fabricating values.
- **White balance:** handling of `as_shot_wb_applied`, `cam_mul` of −1 or 0, and the X-Trans G2 copy.
- **Orientation:** none is applied (sensor geometry is kept, as Siril `user_flip=0` and DSS `RAWUtils.cpp:517` do). The fallback disables `use_fuji_rotate` and the pixel-aspect stretch.
- **Refusals:** Fuji SuperCCD, null `raw_image` (sRAW, linear DNG, Sinar or Pentax 4-shot go to the fallback or error), and lossy DNG or Kodak JPEG (refused cleanly at identify under `NO_JPEG`).
- **Shim:** compiling it without `LIBRAW_LIBRARY_BUILD` is ODR-safe. `internal/libraw_internal_funcs.h` adds only non-virtual member functions, so the class layout is identical.
- **build.rs:** static, thread-safe (no `LIBRAW_NOTHREADS`, which concurrent rayon decodes need), `opt-level = 3` for `libraw-sys` in dev (`Cargo.toml:104-107`), bindgen for the target triple, version pinned by test.
- **Fallback parameters:** gamma 1, `no_auto_bright`, `adjust_maximum_thr = 0`, `user_mul = 1`, raw colour, and output checks on bits, colours and size before slicing.
- **Lossy codecs that do use `curve`:** Sony cRAW, Nikon lossy NEF and DNG `LinearizationTable` all go through `curve`, so the `linear_curve` test does catch them.
- **`raw_files`:** sorting, and case-insensitive extension matching.

---

## Suggested change batches

1. **Trusting what LibRaw reports (classification and refusals):** RAW-3, RAW-6, RAW-4, RAW-15, RAW-7, RAW-17. Each needs either a small shim accessor (`fuji_lossless`, CRX header, `is_phaseone_compressed`) or a `colors`/`cdesc` check, behind the `Libraw` wrapper.
2. **Hot-path fusion:** RAW-12. Measure RAW-13 here before deciding on OpenMP.
3. **Build and coverage:** RAW-14, plus the OpenMP decision from RAW-13. Both need user approval for new system or crate dependencies.
4. **Metadata and policy decisions (user calls):** RAW-5, RAW-8, RAW-16.
5. **Standalone:** RAW-18.
