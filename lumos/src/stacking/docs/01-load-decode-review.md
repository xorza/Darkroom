# Load/decode specification versus implementation review

Date: 2026-07-21

Scope: production code implementing
[`01-load-decode.md`](01-load-decode.md), including the load call sites that decide
whether decoded data can enter calibration or stacking. Test-only code was not reviewed.
The review uses the current working-tree version of the in-tree `fits-well` dependency.

## Outcome

The normative document is substantially stronger than the implementation. Its existing
section 10 audit correctly identifies most format and provenance gaps, but four defects
need immediate attention because they can change scientific results or defeat the memory
contract:

1. Integer FITS physical values are divided a second time, often by the per-frame
   `DATAMAX` value.
2. `stack(paths)` reaches the mixed-purpose `LinearImage::from_file`, so warning-only
   PNG/JPEG/TIFF inputs and clipped direct-RAW previews can enter a scientific stack.
3. FITS pixels are decoded, and compressed images can be decompressed, before Lumos
   validates shape or a memory budget; a zero-length axis then reaches an assertion.
4. Scientific X-Trans demosaic peaks at approximately 22 full `f32` planes per in-flight
   frame, while the planner budgets 14 and the RAM path uses a separate fixed concurrency
   cap instead of the planner's calculated decode concurrency.

No high-confidence mismatch was found in the main interior formulas of the signed RCD
or one-pass/four-direction Markesteijn implementations. Their important defects are at
the data boundary: the calibrated path discards the raw halo promised by the document,
RCD does not define a safe minimum image size, cancellation is not threaded through all
entry points, and the Markesteijn working set is under-budgeted.

The existing gap audit is therefore directionally correct, but it is not yet a complete
runtime-risk audit. In particular, stating that a feature is missing does not capture
when the current call graph silently substitutes a scientifically different product.

## Current call graph

```text
stack(paths)
  -> LightCache::from_paths
  -> StackableImage::load<LinearImage>
  -> LinearImage::from_file
       -> FITS physical_f32 -> integer re-normalization -> LinearImage
       -> RAW demosaic -> preview clamp -> LinearImage
       -> PNG/JPEG/TIFF decode -> warning only -> LinearImage

calibrate_align_stack(raw_paths)
  -> load_raw_cfa
  -> active-area signed CfaImage
  -> calibrate
  -> demosaic with zero margins
  -> align and stack
```

That split explains most of the drift: the generic stack has no way to require a
scientific product, while the explicitly scientific path accepts only camera RAW and
loses full-raw geometry before demosaic.

## Contract coverage

| Specification area | Production status | Main evidence |
|---|---|---|
| Distinct scientific-CFA and linear/preview products | Not enforced | `LinearImage::from_file` returns FITS, clipped RAW, and generic raster data through one type and one method |
| Content-based dispatch | Missing | Dispatch is based on three extension lists |
| FITS physical conversion and null rejection | Partly implemented, then corrupted | `physical_f32()` is correct; `prepare_fits_pixels` subsequently rescales integers |
| FITS HDU, cube, checksum, orientation, and provenance policy | Mostly missing | First image and every three-plane cube are accepted by fixed policy |
| Signed camera-RAW CFA extraction | Implemented for supported one-component LibRaw output | `extract_cfa_pixels::<false>` preserves negative values |
| Complete RAW/DNG compatibility metadata and masks | Missing | Output retains chiefly ISO, CFA, nominal `UInt16`, and white balance |
| RCD interior algorithm | Substantially aligned | Signed denominator fallback and four-pixel interior are present |
| Markesteijn one-pass interior algorithm | Substantially aligned | `NDIR = 4`, canonical staged arena, and eight-pixel informational border are present |
| Full-raw stencil support in the calibrated path | Missing | The active crop is stored before calibration and later demosaiced with zero margins |
| Standard-image transfer and alpha policy | Missing | Values are converted to `f32`; alpha is discarded and non-float input only warns |
| Bounded memory and cancellation | Incomplete | FITS is read whole; X-Trans is under-budgeted; several entry points use a never-cancel token |

## Batch 1 — prevent silent scientific-data corruption

- [x] **P0 — Remove the second normalization of FITS physical values.**

  **Contract.** The document defines `physical = BZERO + BSCALE * stored` and explicitly
  forbids `physical / DATAMAX` at
  `lumos/src/stacking/docs/01-load-decode.md:283` and
  `lumos/src/stacking/docs/01-load-decode.md:319`.

  **Evidence.** `fits-well` already evaluates physical values before returning them:
  `lumos/src/io/astro_image/fits.rs:44-51`. Lumos then calls
  `prepare_fits_pixels` at `lumos/src/io/astro_image/fits.rs:83-93`, which divides every
  integer sample by a positive `DATAMAX`, or otherwise by a logical-type maximum, at
  `lumos/src/io/astro_image/fits.rs:201-210`. The fallback divisors are defined by
  `BitPix::normalization_max` at `lumos/src/io/astro_image/mod.rs:52-65`.

  **Impact.** This changes physical units and gives sibling lights, darks, flats, and
  biases different gains. Signed integer data are also scaled by only the positive type
  maximum even though valid values may be negative. The defect is active in normal
  loading, not merely a missing option.

  **Change.** Replace `prepare_fits_pixels` with finite/null validation only. Keep
  `DATAMIN`/`DATAMAX` as descriptive provenance or display hints. Any later normalized
  view must name and record one stable scale selected by the caller.

  **Validation.** Decode two otherwise byte-identical integer FITS fixtures with
  different `DATAMAX` cards and assert identical physical arrays. Also lock exact
  negative `BSCALE`, nonzero `BZERO`, signed integer, and canonical unsigned results
  against `fits-well::ReadImage::physical`.

- [x] **P0 — Make scientific intent a type/API boundary; do not let preview products enter `stack(paths)`.**

  **Contract.** Sections 1.1 and 2.2 require distinct products and an explicit intent:
  `lumos/src/stacking/docs/01-load-decode.md:21-58` and
  `lumos/src/stacking/docs/01-load-decode.md:148-170`.

  **Evidence.** `LinearImage::from_file` chooses solely by extension and returns physical
  FITS, direct-RAW preview, or generic raster values from the same API at
  `lumos/src/io/astro_image/mod.rs:262-307`. A non-float raster only emits a warning at
  `lumos/src/io/astro_image/mod.rs:291-303`; RGBA is converted to three-channel
  `RGB_F32`, dropping alpha without a policy, at
  `lumos/src/io/astro_image/mod.rs:555-615`. Direct RAW is clamped at
  `lumos/src/io/raw/mod.rs:919-927` and `lumos/src/io/raw/mod.rs:987-993`.
  `StackableImage for LinearImage` delegates directly to that method at
  `lumos/src/io/astro_image/mod.rs:467-482`; `LightCache` instantiates the generic loader
  with `LinearImage` at `lumos/src/stacking/combine/cache/loader/mod.rs:157-172`; and the
  public path stack calls it at `lumos/src/stacking/combine/stack.rs:104-131`.

  **Impact.** A JPEG, transfer-encoded PNG, untagged TIFF, or clipped/demosaiced RAW
  preview can be statistically combined as if it were linear scientific data. Metadata
  contains no provenance discriminator capable of detecting the substitution later.

  **Change.** Give `CfaImage`, `LinearImage`, and `PreviewImage` separate `from_file`
  constructors whose concrete result types establish the requested contract. Restrict
  path-based scientific stacking to the first two types. Keep `PreviewImage` opaque and
  convertible to the display image type without an implicit conversion into `LinearImage`.
  Record container, decoder, transfer, color, clipping, and demosaic provenance.

  **Validation.** Assert that path stacking rejects JPEG, untagged PNG/TIFF, alpha
  raster data, and direct RAW preview. Assert that a physical FITS image and an explicitly
  declared linear float TIFF follow the scientific route, while the same TIFF under a
  preview policy cannot be passed accidentally.

- [x] **P0 — Route mosaic FITS through `CfaImage` and CFA calibration.**

  **Contract.** Mosaic RAW and mosaic FITS are both scientific-CFA products
  (`lumos/src/stacking/docs/01-load-decode.md:25-40`), and the required sequence is
  calibrate before demosaic (`lumos/src/stacking/docs/01-load-decode.md:73-89`).

  **Evidence.** FITS CFA parsing only assigns `metadata.cfa_type` while returning an
  `LinearImage` at `lumos/src/io/astro_image/fits.rs:83-104`. `CfaImage`'s frame-store
  loader accepts camera RAW only at `lumos/src/io/astro_image/cfa.rs:87-95`, and the
  end-to-end calibration pipeline hard-codes `load_raw_cfa` at
  `lumos/src/stacking/pipeline/streaming.rs:101-113`.

  **Impact.** OSC FITS cannot use the documented sensor-domain calibration path. A
  one-plane Bayer FITS can instead be treated as ordinary monochrome pixels, leaving its
  CFA metadata informational and making later detection/stack behavior ambiguous.

  **Change.** Make FITS loading return a CFA product whenever validated CFA metadata and
  scientific-CFA intent agree. Feed RAW and FITS CFA through the same calibration and
  demosaic interface, with source-specific normalization/provenance retained beside the
  common sample plane.

  **Validation.** Construct a small Bayer FITS with distinct per-color site values,
  calibrate it with known CFA masters, and assert exact native-site preservation after
  demosaic. The equivalent RAW-derived `CfaImage` should produce the same result when the
  normalized input plane and CFA phase are identical.

## Batch 2 — make FITS a bounded, explicit trust boundary

- [ ] **P0 — Validate FITS shape and budget before data read/decompression; return errors instead of assertions.**

  **Contract.** Section 3.3 requires zero-axis, overflow, unsupported-shape, and budget
  rejection before allocation (`lumos/src/stacking/docs/01-load-decode.md:230-255`).
  Section 8 repeats the header-first and bounded-decompression requirements at
  `lumos/src/stacking/docs/01-load-decode.md:1340-1354`.

  **Evidence.** Lumos first allocates the entire file at
  `lumos/src/io/astro_image/fits.rs:27-35`, then calls `read_image` and allocates the
  physical `Vec<f32>` at `lumos/src/io/astro_image/fits.rs:44-52`; compressed images are
  decompressed inside that call. Only afterward does it inspect supported shape at
  `lumos/src/io/astro_image/fits.rs:54-81`. `fits-well` legitimately permits an axis of
  zero and gives it a zero product at `fits-well/src/data/mod.rs:51-61`, but Lumos passes
  a two-axis zero shape to `ImageDimensions::new`, whose external-input assertions are at
  `lumos/src/io/astro_image/mod.rs:75-92`. Three-plane FITS also duplicates the complete
  physical array via `to_vec` at `lumos/src/io/astro_image/fits.rs:95-100`.

  **Impact.** A malformed zero-axis image-bearing HDU can panic. A huge unsupported cube
  or compressed image can consume the file size plus decompressed pixels before it is
  rejected, and RGB transiently holds another full decoded image. This bypasses the
  pipeline's advertised memory bound.

  **Change.** Open through the seeking/streaming reader, select an HDU from parsed
  headers, validate nonzero logical axes and checked byte counts, apply a configured
  source/decompressed/decoded memory limit, and only then read pixels. Map all
  file-derived invalid dimensions to `ImageError`. Move planar data without three
  full-plane copies, or decode directly to final planes where the API permits.

  **Validation.** Cover zero axes, overflowing products, unsupported large cubes, a
  compressed-output limit, truncated data, and a valid RGB image. Verify rejection
  occurs before image-size allocation and measure peak resident memory for the RGB case.

- [ ] **P1 — Preserve exact FITS logical sample type and expose a precision policy.**

  **Contract.** Section 3.4 requires distinct `I8`, `U8`, `I16`, `U16`, `I32`, `U32`,
  `I64`, and `U64` provenance and caller-visible lossy narrowing policy at
  `lumos/src/stacking/docs/01-load-decode.md:257-281`.

  **Evidence.** `map_bitpix` folds `I8` into `UInt8` and `U64` into `Int64` at
  `lumos/src/io/astro_image/fits.rs:143-158`. The public `BitPix` enum has no variants
  for those logical types at `lumos/src/io/astro_image/mod.rs:40-50`. Every image is
  unconditionally narrowed via `physical_f32` at
  `lumos/src/io/astro_image/fits.rs:46-51`, although the dependency explicitly documents
  `physical()` for large integers/fine scaling at `fits-well/src/data/mod.rs:486-505` and
  already exposes all logical variants at `fits-well/src/data/mod.rs:541-586`.

  **Impact.** Provenance is false even when the `f32` values happen to be usable, and
  callers cannot reject a file whose `I64`/`U64`/`F64` precision is scientifically
  inadequate.

  **Change.** Store the dependency's complete logical sample type, stored `BITPIX`,
  scaling, and a `Lossless`/`NarrowedToF32` result flag. Accept a precision policy that
  can retain exact integer or `f64` data, permit documented narrowing, or reject it.

  **Validation.** Use boundary fixtures for every logical type, especially signed byte,
  `2^63`, `u64::MAX`, adjacent large integers, and fine `BSCALE`; assert both values and
  the recorded loss classification.

- [ ] **P1 — Add explicit HDU selection, cube semantics, and checksum policy.**

  **Contract.** Sections 3.2, 3.3, and 3.7 specify selector, channel semantics, and
  checksum behavior (`lumos/src/stacking/docs/01-load-decode.md:206-255` and
  `lumos/src/stacking/docs/01-load-decode.md:346-356`).

  **Evidence.** Lumos always picks the first image-bearing HDU at
  `lumos/src/io/astro_image/fits.rs:37-42`, and shape alone makes every `NAXIS3=3` image
  RGB at `lumos/src/io/astro_image/fits.rs:57-60`. It never calls checksum verification.
  The current dependency already supplies `hdu_index`/`image_indices` at
  `fits-well/src/reader/mod.rs:312-361` and `verify_checksum` at
  `fits-well/src/reader/mod.rs:947-999`.

  **Impact.** Multi-extension observations can load the wrong science array, a
  time/wavelength/Stokes cube can be mislabeled as RGB, and corrupt transferred data are
  accepted despite the available verification API.

  **Change.** Add `FitsLoadOptions` with index or `EXTNAME`/`EXTVER` selector, explicit
  cube/channel interpretation, and `Ignore`/`VerifyIfPresent`/`RequireValid` checksum
  policy. Record the chosen HDU and checksum result. Ambiguous multiple images or
  three-plane cubes should fail under the scientific default.

  **Validation.** Test an empty primary plus multiple image extensions, duplicate
  `EXTNAME` values distinguished by `EXTVER`, a three-time-slice cube, explicit RGB, and
  valid/absent/corrupt checksums under every policy.

- [ ] **P1 — Treat FITS orientation, CFA phase, and WCS as one coordinate transform.**

  **Contract.** Sections 3.8 and 3.9 require any pixel reversal to transform WCS and CFA
  together, with height-aware phase (`lumos/src/stacking/docs/01-load-decode.md:358-400`
  and `lumos/src/stacking/docs/01-load-decode.md:402-456`).

  **Evidence.** `read_cfa_from_headers` has no image height or orientation-policy input.
  It unconditionally flips the 2x2 pattern for `BOTTOM-UP`, then applies offsets, at
  `lumos/src/io/astro_image/fits.rs:214-249`; pixel rows and WCS are not transformed.
  Metadata retains only a few pointing scalars at
  `lumos/src/io/astro_image/fits.rs:107-140`, not the selected header/WCS or the applied
  coordinate transform.

  **Impact.** The phase is wrong for one parity when rows are interpreted/reversed, and
  a future physical flip based on this metadata would leave astrometry inconsistent.
  Conflicting `COLORTYP` and `BAYERPAT` values are not diagnosed.

  **Change.** Parse file orientation first, construct an explicit integer pixel affine,
  and apply it to pixels, WCS, CFA origin, and Bayer offsets as a unit. Include height in
  vertical phase. Validate `COLORTYP`; either support the documented 36-site X-Trans
  convention or reject it explicitly when CFA intent is requested.

  **Validation.** Sweep odd/even height and width, all Bayer patterns, both row orders,
  signed offsets, absent row order, and rotated/skewed WCS. Assert every output site's
  color and world coordinate, not only the four-character pattern label.

- [ ] **P1 — Replace extension-only dispatch with bounded content detection.**

  **Contract.** Section 2 requires signature-first detection and distinguishes `.fits.fz`
  from `.fits.gz` at `lumos/src/stacking/docs/01-load-decode.md:124-146`.

  **Evidence.** FITS and raster formats are fixed arrays at
  `lumos/src/io/astro_image/mod.rs:23-29`; camera RAW is another fixed array at
  `lumos/src/io/raw/mod.rs:35-36`; and `from_file` uses only `Path::extension` at
  `lumos/src/io/astro_image/mod.rs:274-305`. Diagnostics report only the extension at
  `lumos/src/io/astro_image/error.rs:32-33`.

  **Impact.** `.fts`, `.fits.fz`, and outer-gzip names are rejected, broader LibRaw
  formats cannot be probed, and a misleading extension selects the wrong decoder before
  the observed signature is reported.

  **Change.** Centralize a bounded prefix detector for FITS/TIFF-DNG/PNG/JPEG, then let
  LibRaw probe unknown camera containers. Treat extensions as hints. If outer gzip is not
  implemented, detect and reject it specifically with compressed/decompressed limit
  context rather than treating it as an unknown `gz` image.

  **Validation.** Rename valid fixtures, use compound extensions, mismatch signatures
  and extensions, exercise a LibRaw-supported format outside the current list, and check
  deterministic diagnostics for truncated/unknown prefixes.

## Batch 3 — make RAW calibration inputs reproducible

- [ ] **P1 — Retain RAW compatibility provenance and validate more than CFA pattern.**

  **Contract.** Sections 4.1, 4.5, and 7 require camera identity, active area, exposure,
  temperature, gain/offset, black/white model, denominator, linearization, masks, and
  transform history (`lumos/src/stacking/docs/01-load-decode.md:494-531`,
  `lumos/src/stacking/docs/01-load-decode.md:676-697`, and
  `lumos/src/stacking/docs/01-load-decode.md:1282-1321`).

  **Evidence.** `UnpackedRaw` retains dimensions/margins, black model, CFA, white
  balance, and ISO only at `lumos/src/io/raw/mod.rs:428-446`; `open_raw` extracts only
  those fields at `lumos/src/io/raw/mod.rs:809-862`. Published RAW metadata is chiefly
  ISO, nominal `UInt16`, dimensions, CFA, and white balance at
  `lumos/src/io/raw/mod.rs:977-984` and `lumos/src/io/raw/mod.rs:1052-1059`.
  `AstroImageMetadata` has no decoder/source domain, active rectangle, black model,
  normalization denominator, white level, linearization status, or source masks at
  `lumos/src/io/astro_image/mod.rs:138-192`. Calibration compatibility validates only
  CFA presence/equality at `lumos/src/stacking/calibration_masters/mod.rs:505-587`.

  **Impact.** Masters from a different camera, readout mode, denominator, ISO/gain,
  temperature, exposure, active area, or decoder policy can pass compatibility whenever
  dimensions and the CFA pattern happen to agree. Saturated and decoder-invalid samples
  become ordinary finite numbers.

  **Change.** Add a structured, source-specific `SensorProvenance` and validity/
  saturation planes. Define compatibility fields and tolerances per master type rather
  than putting more unrelated `Option` fields into the flat metadata bag. Validate the
  complete compatible subset before any calibration mutation.

  **Validation.** Table-drive exact accept/reject cases for camera/readout identity,
  geometry and phase, denominator/black semantics, ISO/gain, exposure, temperature, and
  filter. Propagate known invalid/saturated sites through calibration and demosaic.

- [ ] **P1 — Either implement a declared DNG profile exactly or reject unsupported DNG forms early.**

  **Contract.** Section 4 distinguishes LibRaw compatibility normalization from the DNG
  reference model at `lumos/src/stacking/docs/01-load-decode.md:609-657`.

  **Evidence.** The pinned dependency is LibRaw 0.20.1 (`Cargo.lock:2400-2401`). Lumos
  exposes `dng` unconditionally in `RAW_EXTENSIONS` at `lumos/src/io/raw/mod.rs:35-36`,
  but accepts only LibRaw's one-component `u16 raw_image` union member at
  `lumos/src/io/raw/mod.rs:448-465`. Normalization uses
  `maximum_raw - common_black` at `lumos/src/io/raw/mod.rs:209-255`; it does not model
  DNG `WhiteLevel - max(BlackLevel + DeltaH + DeltaV)` or preserve the discarded
  per-row/per-column deltas.

  **Impact.** Supported-looking DNG files can either fail late based on a null union
  member or decode under LibRaw compatibility semantics that are not equivalent to the
  document's conforming DNG path. No provenance tells calibration which model was used.

  **Change.** Define a supported DNG profile and inspect required tags/buffer layout
  before advertising scientific support. Implement exact one-time linearization, full
  black model, per-plane white denominator, and supported compression/sample formats;
  otherwise return a feature-specific unsupported error. Record LibRaw version and
  whether its unpacker already applied linearization.

  **Validation.** Pin fixtures with a non-identity linearization table, nonconstant
  `BlackLevelDeltaH/V`, per-plane white levels, compressed integer data, float raw data,
  and `color3`/`color4` buffers. Assert exact transformed samples or exact early errors.

- [ ] **P1 — Resolve the calibrated-path raw-halo contradiction.**

  **Contract.** The common demosaic contract says to crop only after stencils that use
  margins (`lumos/src/stacking/docs/01-load-decode.md:735-747`), and RCD Step R8 says to
  run in full-raw coordinates so margins support the active boundary
  (`lumos/src/stacking/docs/01-load-decode.md:969-980`).

  **Evidence.** Direct Bayer preview normalizes the full raw buffer and passes real raw
  dimensions/margins at `lumos/src/io/raw/mod.rs:521-551`. Scientific
  `load_raw_cfa`, however, extracts only the active rectangle at
  `lumos/src/io/raw/mod.rs:1049-1065`. After calibration, `CfaImage::demosaic` supplies
  `raw_width=width`, `raw_height=height`, and zero margins for both Bayer and X-Trans at
  `lumos/src/io/astro_image/cfa.rs:127-168`.

  **Impact.** The calibrated path always uses fallback interpolation for the outer four
  Bayer or eight X-Trans pixels even when usable sensor halo exists. The document's
  statement that current RAW loading keeps full dimensions and active margins separate
  at `lumos/src/stacking/docs/01-load-decode.md:1393` is true only of the temporary
  decoder/direct path, not the scientific `CfaImage` product.

  **Change.** Decide which margin pixels are scientifically usable. Either retain and
  calibrate a validity-tagged halo in `CfaImage`, preserving raw geometry until the final
  crop, or explicitly define the active rectangle as a hard boundary and revise the
  normative full-raw-stencil claim. Optical-black pixels must not be treated as image
  halo merely because they are adjacent.

  **Validation.** Use a fixture with known active margins and compare every edge pixel
  against a full-raw reference. Separately mark optical-black margins invalid and assert
  the selected boundary fallback does not consume them.

- [ ] **P2 — Correct unknown-CFA preview provenance and the stale fallback contract.**

  **Evidence.** A developed unknown-sensor fallback can return multiple channels but is
  labeled `Some(CfaType::Mono)` at `lumos/src/io/raw/mod.rs:970-984`. The comment at
  `lumos/src/io/raw/mod.rs:997-1004` says `load_raw_cfa` falls back to a developed mono
  wrapper, while the implementation correctly returns an unsupported error at
  `lumos/src/io/raw/mod.rs:1034-1046`.

  **Impact.** Preview metadata falsely describes RGB as a monochrome sensor product, and
  the stale contract encourages a future caller or refactor to reintroduce a forbidden
  scientific-domain fallback.

  **Change.** Mark the developed result with preview provenance and `cfa_type=None`;
  validate that the returned channel count is one or three and describe it independently
  of the unknown original CFA. Remove the obsolete fallback statement.

  **Validation.** Force the unknown-sensor preview branch and assert channel count,
  preview domain, clipping status, and absent CFA label. Assert scientific CFA loading
  returns `UnsupportedCfa` without invoking development.

- [ ] **P2 — Consolidate the duplicate direct-preview and scientific RAW transforms.**

  **Evidence.** Direct Bayer loading first clamps common-black normalization
  (`lumos/src/io/raw/normalize.rs:5-39`), clamps residual black correction again
  (`lumos/src/io/raw/mod.rs:282-312`), demosaics through a separate full-raw method, and
  finally clamps all channels at `lumos/src/io/raw/mod.rs:919-993`. Scientific loading
  uses the separate active-only `extract_cfa_pixels::<false>` path at
  `lumos/src/io/raw/mod.rs:467-517` and demosaics later through `CfaImage`.

  **Impact.** Decode, black correction, geometry, cancellation, and metadata behavior can
  drift between two implementations. The current preview clips before and after an
  adaptive demosaic, while the document correctly says those are separate policies at
  `lumos/src/stacking/docs/01-load-decode.md:659-670`.

  **Change.** Materialize one internal signed `DecodedSensorFrame` containing the sensor
  plane, active rectangle/halo, black/white model, masks, CFA, and provenance. Build the
  scientific CFA product directly from it; build preview by demosaicing the same signed
  representation and applying one explicit final display clamp. Keep specialized u16
  kernels only as verified storage optimizations behind the same semantic transform.

  **Validation.** Given identical normalized sensor input and geometry, assert the
  preview equals `clamp(scientific_demosaic, 0, 1)` when no other preview transform is
  selected. Include sub-black samples and interpolation overshoot so pre-clipping would
  fail the comparison.

## Batch 4 — make resource and cancellation promises real

- [ ] **P0 — Derive memory planning from the selected demosaic algorithm and honor it on both tiers.**

  **Evidence.** Markesteijn allocates an 18-`f32`-word-per-pixel arena at
  `lumos/src/io/raw/demosaic/xtrans/markesteijn.rs:41-70`, then allocates three output
  planes while that arena is still live at
  `lumos/src/io/raw/demosaic/xtrans/markesteijn.rs:222-245`. The scientific input CFA is
  another full `f32` plane, so the core peak is approximately 22 planes before
  row/thread scratch or downstream detection. The planner hard-codes 14 decode planes at
  `lumos/src/stacking/frame_store/mod.rs:455-486`. The streaming path uses
  `decode_concurrency` at `lumos/src/stacking/pipeline/streaming.rs:153-176`, but the RAM
  path always uses four workers at `lumos/src/stacking/pipeline/streaming.rs:29-33` and
  `lumos/src/stacking/pipeline/streaming.rs:81-96`.

  **Impact.** X-Trans can exceed its claimed memory budget by roughly eight complete
  `f32` planes per in-flight frame, multiplied by concurrency. The current in-RAM fit
  test may often imply enough room for four workers under its own constants, but the
  14-plane X-Trans underestimate invalidates that implication and the separate fixed cap
  can drift independently from future estimator changes.

  **Change.** Give each decoder/demosaicer a single-source-of-truth peak estimator that
  includes source storage, live arena regions, outputs, row/thread scratch, masks, and
  the next simultaneously live pipeline stage. Feed it into `plan_memory` and use the
  returned decode concurrency in both RAM and streaming branches.

  **Validation.** Lock analytical byte counts for mono, Bayer RCD, and X-Trans. Run each
  under constrained budgets at concurrency boundaries and compare measured peak RSS to
  the estimate plus an explicit allocator tolerance. Assert RAM and streaming paths make
  the same concurrency decision for the same per-frame peak.

- [ ] **P1 — Thread cancellation and resource context through all load/decode entry points.**

  **Evidence.** `StackableImage::load` accepts only a path at
  `lumos/src/stacking/frame_store/mod.rs:121-136`, so generic loading can poll only
  between frames. FITS whole-file read/decompression and physical conversion have no
  cancel input. Direct Bayer uses `CancelToken::never` at
  `lumos/src/io/raw/mod.rs:521-556`, and direct X-Trans does the same at
  `lumos/src/io/raw/demosaic/xtrans/mod.rs:24-73`. The calibrated demosaic path does pass
  a real token, which demonstrates the intended staged behavior at
  `lumos/src/io/astro_image/cfa.rs:125-175`.

  **Impact.** Cancellation of a large FITS, direct RAW decode, or direct demosaic can be
  delayed for the entire operation. The caller also cannot give the decoder a memory or
  scientific-intent policy through the shared frame-store interface.

  **Change.** Replace path-only loading with a `LoadContext` carrying intent, selector,
  cancellation, and resource limits. Poll between bounded FITS tiles/conversion chunks
  and every demosaic stage. Document LibRaw unpack as the remaining uninterruptible
  region and avoid launching it after cancellation.

  **Validation.** Cancel during FITS scaling/decompression, RCD, Markesteijn, and between
  LibRaw frames; assert prompt `Cancelled`, no cache publication, and bounded drain work.

- [ ] **P1 — Define and enforce safe minimum dimensions for RCD.**

  **Contract.** Section 11.4 requires a documented tiny-image fallback and rejection
  below its supported size at `lumos/src/stacking/docs/01-load-decode.md:1540-1550`.

  **Evidence.** `BayerImage::with_margins` checks only nonzero dimensions and buffer
  containment at `lumos/src/io/raw/demosaic/bayer/mod.rs:107-177`. RCD's border is four,
  but its top band always loops `0..4` and its left band can loop `0..4` regardless of
  actual height/width at `lumos/src/io/raw/demosaic/bayer/rcd.rs:618-704`.

  **Impact.** A Bayer image shorter or narrower than four pixels can index beyond the
  CFA/output buffers and panic. The public `CfaImage` representation allows such a
  buffer to reach the demosaic path.

  **Change.** Choose a minimum RCD size and return `DemosaicError::UnsupportedDimensions`,
  or make all border bands use bounded ranges and define a complete wider-search
  fallback for every channel. Do not rely on normal camera sizes to uphold an API
  invariant.

  **Validation.** Sweep every width/height around 1, 2, 3, 4, 8, and the first full
  interior size for all Bayer phases. Each case must either return a typed error or exact
  finite, native-sample-preserving output without panic.

- [ ] **P1 — Use checked size arithmetic and fallible allocation in demosaic working sets.**

  **Evidence.** RCD derives `npix = raw_width * raw_height` and immediately allocates
  several planes at `lumos/src/io/raw/demosaic/bayer/rcd.rs:92-123`. Markesteijn derives
  `pixels = width * height` and `total = 18 * pixels` before an infallible allocation at
  `lumos/src/io/raw/demosaic/xtrans/markesteijn.rs:63-70`. There is no production
  `try_reserve`/allocation-error path in these modules.

  **Impact.** Overflow can invalidate unsafe initialization assumptions, while a large
  but representable working set aborts the process rather than returning a bounded
  decode error. Header-level checks alone are insufficient because the demosaic
  functions receive dimensions from upstream image objects and do not establish the
  checked-arithmetic invariant at their own allocation boundary.

  **Change.** Centralize checked plane/sample byte calculations, validate them before
  constructing image views, use fallible allocation for externally influenced sizes,
  and propagate an allocation/resource-limit variant without publishing partial output.

  **Validation.** Exercise arithmetic-overflow dimensions through constructors that do
  not require allocating the claimed image, inject allocation-limit failures at each
  working-set allocation, and assert clean error propagation.

## Batch 5 — simplify metadata and keep the audit accurate

- [ ] **P2 — Replace contradictory generic metadata fields with typed source geometry and numeric provenance.**

  **Evidence.** `AstroImageMetadata` exposes a defaulted `BitPix`, untyped
  `Vec<usize> header_dimensions`, and `data_max` described as a saturation level at
  `lumos/src/io/astro_image/mod.rs:138-192`. FITS stores its NAXIS-first shape
  `[width,height,...]` at `lumos/src/io/astro_image/fits.rs:44-51` and
  `lumos/src/io/astro_image/fits.rs:107-120`, while RAW writes
  `[height,width,channels]` at `lumos/src/io/raw/mod.rs:977-984` and
  `lumos/src/io/raw/mod.rs:1052-1059`. Inside production Lumos, these fields are assigned
  but `header_dimensions` is not consumed; `BitPix::normalization_max` chiefly enables
  the incorrect FITS rescaling.

  **Impact.** The same public field uses opposite axis order by source, defaults can
  imply `UInt8` without evidence, and FITS `DATAMAX` is conflated with a detector
  saturation threshold. Adding more optional fields to this bag will not make
  compatibility or provenance reliable.

  **Change.** Keep canonical output geometry in `ImageDimensions`. Add source-specific
  enums/records for FITS stored shape and numeric scaling, RAW active/raw geometry and
  sensor normalization, and raster transfer/color metadata. Remove
  `normalization_max`; rename descriptive FITS extrema separately from detector
  saturation. Avoid compatibility shims while the API is pre-1.0.

  **Validation.** Round-trip each provenance variant through cache serialization and
  assert canonical dimensions, original FITS axis order, exact logical sample type, RAW
  active rectangle, and distinct descriptive/saturation extrema.

- [ ] **P2 — Update section 10 from a feature checklist to an executable risk audit.**

  **Evidence.** The current audit at
  `lumos/src/stacking/docs/01-load-decode.md:1379-1451` already discloses the major FITS,
  standard-raster, metadata, mask, DNG, and optional-method gaps. It does not disclose
  the active `stack(paths)` preview/raster call chain, decode-before-validation panic and
  allocation order, calibrated-path halo loss, pattern-only master compatibility,
  X-Trans memory undercount/RAM-concurrency bypass, cancellation holes, or tiny-RCD
  panic. Its statement that RAW dimensions and margins are retained can be read as
  applying to the scientific product when they survive only in temporary/direct decode.

  **Change.** After implementation policy is decided, add these runtime defects to the
  current-state audit and distinguish three states consistently: implemented and
  verified, missing and rejected safely, and missing with an unsafe/silent fallback.
  Keep normative algorithms separate from repository-state claims so a checked-off
  feature cannot hide a violating call site.

  **Validation.** Make every section-10 claim point to a production entry point and one
  exact invariant test. A review should be able to trace each accepted file kind from
  public API to final product domain without relying on a warning or a comment.

## Open decisions

- [ ] Are any full-raw margins known to be valid illuminated halo for RCD/Markesteijn,
  or must the active rectangle always be a hard scientific boundary? This decides
  whether code or the normative full-raw-stencil requirement changes.
- [ ] Should unselected `NAXIS3=3` FITS remain a compatibility-mode RGB assumption, or
  should scientific loading reject it unless `FitsLoadOptions` declares channel
  semantics? The document currently favors rejection.
- [ ] Which public precision modes must Lumos support beyond `f32`: exact integers,
  `f64`, rejection-only, or some combination? Provenance must still report narrowing.
- [ ] Is `LinearImage::from_file` intended to remain a preview convenience? If so, name
  and document it as such rather than using it as `StackableImage::load`.

## Recommended implementation order

1. Hot-fix FITS by removing integer re-normalization and block preview/unknown-transfer
   products from path-based scientific stacking.
2. Introduce typed load intent/result/provenance and `LoadContext`; route FITS CFA
   through it.
3. Refactor FITS to select and validate header semantics/budgets before decode, then add
   checksum, precision, orientation, and WCS policy.
4. Consolidate RAW decode into one signed sensor record, decide halo policy, and expand
   DNG/compatibility/mask provenance.
5. Replace constant-plane memory estimates with algorithm-owned estimates, honor
   concurrency on both tiers, and finish cancellation/fallible-allocation plumbing.
6. Enforce tiny-image behavior, simplify legacy metadata fields, and update the source
   document's current-state audit and invariant tests together.
