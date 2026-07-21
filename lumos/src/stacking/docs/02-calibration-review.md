# Calibration specification versus implementation review

Date: 2026-07-21

Scope: production code implementing
[`02-calibration.md`](02-calibration.md), from calibration-master loading and
construction through per-light calibration, optional cosmic-ray rejection, and the
handoff to demosaic/stacking. Test-only and benchmark code were not reviewed.

## Outcome

The specification is substantially stronger than the implementation. Its section 11
gap table correctly identifies most missing scientific features, but it understates five
active correctness risks:

1. File-based flat construction normalizes each raw flat before subtracting its
   flat-dark or bias. This is mathematically different from the documented algorithm.
2. Light calibration marks and mutates the input before all fallible checks have run, so
   a dimension failure can leave a partially calibrated frame marked as complete.
3. Calibration roles are grouped by dimensions and finite samples only; metadata from
   frame zero silently becomes master metadata without checking the rest of the set.
4. The scalar uncertainty retained for Winsorized dark/bias masters does not decrease
   with frame count, and subsequent subtraction/flat division leaves that scalar in stale
   units.
5. The public master-stacking entry point bypasses validation performed by the general
   stacking API and can turn malformed configuration into an assertion or undefined
   calibration semantics.

The largest architectural difference is still the one already identified by the
document: `CfaImage` carries pixels, metadata, and one scalar quantization sigma, whereas
the target product carries pixels, mask bits, per-pixel variance, and immutable
provenance. Defect and cosmic-ray routines consequently overwrite samples that later
receive ordinary scientific weight.

The core arithmetic that is present is otherwise mostly sound. It keeps signed `f32`
samples, selects dark over bias without double subtraction, selects flat-dark over bias
for flat preparation, subtracts the light additive master before flat division, computes
the final prepared-flat normalization per CFA color, masks the full known defect union
while finding repair neighbors, and runs cosmic rejection before demosaic. The robust
center/scale and original-survivor mean used by Winsorized rejection match section 4.3.

## Current data flow

```text
from_files
  dark files      -> stack(no normalization) ---------------------> raw master dark
  bias files      -> stack(no normalization) ---------------------> master bias
  flat-dark files -> stack(no normalization) ---------------------> master flat-dark
  flat files      -> global raw-CFA median normalization -> stack -> combined raw flat
                                                                    |
                    master flat-dark else bias ---------------------+ subtract once
                                                                    |
                    per-CFA-color arithmetic mean -> floor at 0.1 -> prepared divisor

calibrate(light)
  assert not calibrated
  -> validate CFA pattern only
  -> set calibrated = true
  -> subtract dark else bias                 (dimension assertion may panic)
  -> divide by prepared flat                 (dimension assertion may panic)
  -> overwrite persistent defects with local medians
  -> optionally detect and overwrite cosmic rays
  -> demosaic
```

The first path explains the flat-order defect: the flat subtractor is unavailable until
after a generic stack has already normalized and combined the raw flat exposures. The
second explains the transactional defect: the only validation error is CFA-related, while
shape and divisor assumptions are asserted after mutation begins.

## Contract coverage

| Specification area | Production status | Main evidence |
|---|---|---|
| Signed CFA-domain calibration before demosaic | Implemented | `CfaImage::subtract`, pipeline order |
| Exactly one light additive path | Implemented for matched dark-or-bias only | `dark` takes precedence over `bias`; no scalable-dark representation |
| Per-flat additive correction before normalization | Incorrect | flat preset normalizes raw CFA frames before `from_images` subtracts one combined master |
| Acquisition/decode compatibility | Mostly missing | loader checks dimensions and finite samples; frame-zero metadata wins |
| Transactional application and complete validation | Incorrect | calibrated flag is set before assertion-based arithmetic |
| Mask and per-pixel variance product | Missing | `CfaImage` has neither plane |
| Prepared-flat robust normalization and validity | Incomplete | arithmetic mean plus unconditional `0.1` floor |
| Persistent defect detection and repair | Partly implemented | isolated hot/cold detection is CFA-aware; no persistent output mask, stability, or structure classes |
| Single-frame cosmic rejection | Partly implemented | correctly ordered and CFA-dispatched; destructive, unvalidated, uncancellable, and no safety mask/gate |
| Overscan, dark scaling, and frame/set quality selection | Missing | no corresponding calibration policy or result types |
| Cache publication | Partly implemented | atomic and schema-versioned, but lacks provenance and semantic validation |
| Cancellation | Incomplete | low-level role stack supports it; convenience construction, calibration, and cosmic work do not |

## Batch 1 — stop current silent corruption and panic paths

- [ ] **P0 — Calibrate every flat exposure before estimating its illumination and combining it.**

  **Contract.** The required sequence is subtractor → per-color illumination estimate →
  per-frame normalization → rejected combine → final per-color normalization at
  `02-calibration.md:531-575`. The document explicitly states that normalizing `F_j`
  before subtracting `P` is not equivalent.

  **Evidence.** `StackConfig::flat` selects `Normalization::Multiplicative` at
  `../combine/config.rs:249-259`. A CFA frame exposes one channel to the generic stack,
  so multiplicative normalization uses one global frame median at
  `../combine/normalization/mod.rs:207-255`. `from_files` stacks flat paths with that
  preset at `../calibration_masters/mod.rs:446-466` and
  `../calibration_masters/mod.rs:483-490`. Only after all four role stacks complete does
  `from_images` choose a flat-dark-or-bias subtractor and subtract it from the combined
  flat at `../calibration_masters/mod.rs:373-397`.

  **Impact.** For flat exposure `j`, the implemented scale is derived from `F_j`; the
  required scale is derived from `F_j - P_j`. A constant bias therefore changes the
  relative scale according to illumination, and a structured additive master is then
  subtracted in a scale inconsistent with every contributing exposure. The resulting
  divisor imprints residual gradients even when the final per-color mean is one.

  **Change.** Replace generic `stack_cfa_master(flat, StackConfig::flat())` with a flat
  builder that receives the matched flat-dark or bias first. Decode each flat, validate
  its compatibility, subtract the corresponding additive master, estimate a robust
  location independently for each CFA color on valid samples, normalize that exposure,
  and then feed the prepared exposures to rejection/combination. Preserve the final
  robust per-color normalization as a separate step.

  **Validation.** Hand-construct two flats with different illumination, one nonzero
  structured bias, and known CFA-color responses. Assert the exact result of
  `normalize(F_j - P)` and demonstrate that the old `normalize(F_j) - P` result differs.
  Cover Mono, all Bayer phases, X-Trans phases, resident and mmap tiers, and flat-dark
  precedence over bias.

- [ ] **P0 — Make per-light calibration validate first and publish atomically.**

  **Contract.** All dimensions, finite values, decoder/acquisition compatibility,
  divisors, masks, and uncertainty planes must validate before mutation; an error or
  cancellation must leave pixels and calibrated state unchanged
  (`02-calibration.md:92-109` and `02-calibration.md:654-668`).

  **Evidence.** `CalibrationError` represents only missing/mismatched CFA patterns at
  `../calibration_masters/mod.rs:85-103`. `calibrate` asserts on a second application,
  validates only CFA, sets `metadata.calibrated = true`, and then performs subtraction,
  division, and repair at `../calibration_masters/mod.rs:505-543`. Both
  `CfaImage::subtract` and prepared-flat application assert dimensions at
  `../../io/image/cfa.rs:178-197` and
  `../calibration_masters/prepared_flat/mod.rs:40-55`.

  **Impact.** A light matching CFA but not dimensions is marked calibrated before the
  first assertion. A dark can be subtracted successfully and a later flat mismatch can
  then panic, leaving both altered pixels and a completion marker. Public, externally
  sourced input is therefore handled as an internal invariant, and retrying or reporting
  the failure safely is impossible.

  **Change.** Introduce a complete `validate_application` pass returning typed expected
  errors for double application, dimensions, non-finite inputs, invalid divisor, and
  compatibility/provenance mismatch. Apply into a fresh output or a scratch plane and
  swap only after every stage succeeds; set `calibrated` and attach operation provenance
  last. Thread cancellation through the operation and discard scratch output on cancel.

  **Validation.** For every validation failure and a cancellation at each stage, assert
  exact equality of the original pixels, metadata, mask, and uncertainty. Include a
  valid dark followed by an invalid-size flat to lock the current partial-mutation
  regression, and make a second application return an error rather than panic.

- [ ] **P0 — Enforce typed within-role and cross-role compatibility instead of inheriting frame zero.**

  **Contract.** Section 2 requires a compatibility key spanning geometry, CFA phase,
  sensor/readout identity, decoder domain, gain/ISO, offset, exposure, temperature, and
  role-specific optical fields (`02-calibration.md:147-185`). Master metadata must be
  derived from checked common values, never copied arbitrarily from the first frame
  (`02-calibration.md:330-356`).

  **Evidence.** The cache loader compares only `ImageDimensions` and validates finite
  samples at `../combine/cache/loader/mod.rs:229-249`; it saves metadata only when
  `idx == 0`, then publishes that metadata at
  `../combine/cache/loader/mod.rs:252-277`. The disk tier explicitly clones frame-zero
  metadata at `../combine/cache/loader/mod.rs:283-355`. `run_stacking` copies that value
  into the master at `../combine/stack.rs:394-480`. `from_images` does not validate role
  metadata before flat arithmetic or defect detection at
  `../calibration_masters/mod.rs:353-409`. Although `ImageMetadata` has fields such
  as exposure, ISO, filter, gain, temperature, binning, and offset
  (`../../io/image/mod.rs:138-191`), `load_raw_cfa` currently populates chiefly ISO,
  sample type, dimensions, CFA, and white balance
  (`../../io/raw/mod.rs:1034-1065`).

  **Impact.** Different exposures can enter one dark, different filters can enter one
  flat, or a mismatched camera/readout can enter any role with no rejection. The master
  then advertises whichever metadata happened to be first, making a heterogeneous set
  look coherent. Cross-role and light/master checks cannot be implemented reliably
  because RAW decode provenance and much of the acquisition key are absent.

  **Change.** Add a typed `CalibrationCompatibilityKey` produced by Stage 1. Inspect all
  headers before decoding, compare the common fields and role-specific relations, repeat
  the checks on decoded facts, and make missing mandatory metadata an explicit policy
  decision rather than a wildcard. Extend RAW metadata/provenance before enabling the
  checks. Derive master metadata such as exposure interval, temperature range,
  `NCOMBINE`, and algorithm identity from the whole accepted set.

  **Validation.** Sweep one mismatch at a time across exposure, ISO/gain, offset,
  temperature, filter, dimensions, binning, CFA phase, decoder profile, and scale. Assert
  a typed error naming role, frame, field, expected value, and actual value. Permuting the
  accepted input order must leave master metadata and pixels unchanged.

- [ ] **P0 — Correct quantization uncertainty instead of retaining a one-frame floor or stale units.**

  **Contract.** Independent source quantization variance must reduce through combination;
  a master must not be floored at one input frame's quantization variance
  (`02-calibration.md:440-480`). Additive and ratio operations must propagate master and
  divisor uncertainty (`02-calibration.md:1243-1401`).

  **Evidence.** Dark and bias presets use Winsorized rejected means at
  `../combine/config.rs:229-246`. `run_stacking` handles Winsorized rejection specially by
  taking the maximum input sigma, without any `1/sqrt(N)` reduction, at
  `../combine/stack.rs:343-354` and `../combine/stack.rs:423-434`. Its comment says
  Winsorization replaces output samples, but this implementation only Winsorizes a
  scratch copy to estimate center/scale (`../combine/rejection.rs:263-317`), compacts
  original survivors (`../combine/rejection.rs:318-342`), and averages those survivors
  (`../combine/rejection.rs:921-983`). `CfaImage::subtract` changes pixels without
  updating `quantization_sigma` at `../../io/image/cfa.rs:178-197`; prepared-flat
  subtraction, normalization, and division likewise leave their input scalar untouched
  at `../calibration_masters/prepared_flat/mod.rs:9-55`. Hot detection then uses the
  master scalar directly as its resolution floor at
  `../calibration_masters/defect_map/mod.rs:417-430`.

  **Impact.** With `N` equal-sigma darks and no rejected samples, the stored master sigma
  is `sigma`, not `sigma/sqrt(N)`. Hot-pixel detection can therefore become too
  conservative as more dark frames are added. After subtraction and especially
  per-coordinate flat division, the scalar described as being in the current CFA units
  is no longer mathematically in those units, so downstream code receives plausible but
  false uncertainty.

  **Change.** As an immediate correction, use the existing survivor-index path for
  Winsorized rejection and compute the same linear mean propagation used by other
  rejected means. Do not preserve a scalar across spatially varying division; clear it
  unless it is rigorously recomputed. The complete fix is the document's per-pixel
  variance plane with actual survivor coefficients, covariance-aware flat propagation,
  and separate diagnostics for observed excess scatter.

  **Validation.** For `N = 1, 2, 4, 16` equal independent quantization sigmas, assert
  exact `sigma/sqrt(N)` for an unrejected mean and hand-compute changed survivor sets.
  Verify Winsorized and ordinary survivor tracking agree when they retain the same
  indices. Propagate a small light-dark-flat example analytically and assert every output
  variance and unit transformation.

- [ ] **P1 — Apply the normal stack-configuration validation in `stack_cfa_master`.**

  **Contract.** Public configuration is expected input and must fail as `Result`, not by
  assertion or silent fallback. Section 1.3 also requires every scalar used by
  calibration to be finite and valid (`02-calibration.md:92-105`).

  **Evidence.** The general public `stack` and `stack_images` paths call
  `config.validate()` and validate manual-weight cardinality at
  `../combine/stack.rs:104-115` and `../combine/stack.rs:152-163`. The public
  `stack_cfa_master` constructs the cache and calls the internal `run_stacking` directly
  without either check at `../calibration_masters/mod.rs:233-262`. Invalid rejection
  parameters are otherwise covered by `StackConfig::validate` at
  `../combine/config.rs:273-360`; a manual weight-count mismatch reaches an assertion in
  `CfaCache::process_chunked` at `../combine/cache/mod.rs:382-401`.

  **Impact.** An external caller can pass NaN/negative clipping thresholds or the wrong
  number of weights to a public `Result` API. Some values silently alter rejection;
  others panic in the combine path. Independently built masters therefore have weaker
  safety than ordinary stacks.

  **Change.** Call `config.validate()` and expose/reuse manual-weight cardinality
  validation before any decode. In the longer-term typed role API proposed in Batch 4,
  restrict calibration masters to role-valid normalization and weighting choices.

  **Validation.** Reuse the complete invalid-configuration table against
  `stack_cfa_master`; assert typed errors for NaN, infinity, nonpositive thresholds, zero
  iterations, invalid/manual weights, and wrong weight count, all before loading a file.

## Batch 2 — introduce the scientific sample product and repair semantics

- [ ] **P0 — Carry image, mask bits, per-pixel variance, and provenance as one calibration product.**

  **Contract.** Sections 1.2-1.4 require pixels, typed mask bits, variance, provenance,
  and calibrated state to travel together; repaired or interpolated samples must retain
  zero scientific weight (`02-calibration.md:68-145`).

  **Evidence.** `CfaImage` contains only `Buffer2<f32>`, metadata, and one scalar
  quantization sigma at `../../io/image/cfa.rs:57-66`. Defect repair overwrites the
  pixel buffer at `../calibration_masters/defect_map/mod.rs:144-184`, and cosmic rejection
  returns only a count after overwriting the same buffer at
  `../calibration_masters/cosmic_ray.rs:95-105`. The pipeline immediately demosaics that
  modified image at `../pipeline/streaming.rs:101-133`; no transient or persistent mask
  reaches stack weighting.

  **Impact.** Source-invalid, saturated, hot, cold, cosmic, flat-invalid, and
  interpolated values are indistinguishable from observations. A visually useful fill
  is counted as independent sensor data, biasing rejection, effective sample size, and
  inverse-variance weighting. Per-pixel calibration noise and shared-master uncertainty
  cannot be represented.

  **Change.** Introduce a CFA calibration product containing data, a bitwise mask plane,
  a variance plane, provenance, and calibrated state. Define mask propagation once and
  make subtraction, flat division, defect fill, demosaic, registration, and stacking
  consume that product. Fills may update data for geometric processing but must retain
  defect/cosmic plus `INTERPOLATED` bits and zero scientific confidence.

  **Validation.** Use tiny hand-computed products with overlapping mask bits. Assert
  exact pixels, mask OR behavior, variance, coverage/confidence, survivor count, and
  weight through calibration, demosaic, warp, and stack. A filled sample must never
  increase effective sample size.

- [ ] **P0 — Replace arithmetic-mean normalization and the hidden flat floor with an explicit validity policy.**

  **Contract.** Flat illumination and final normalization must use robust, per-color
  statistics on valid samples (`02-calibration.md:531-566`). A divisor below the maximum
  permitted correction is marked `FLAT_INVALID`; silently changing `0.05` to `0.1` is
  explicitly forbidden (`02-calibration.md:629-668`). A flat without a subtractor
  requires an explicit warned policy (`02-calibration.md:226-229`).

  **Evidence.** Mono normalization sums every sample and asserts a positive arithmetic
  mean at `../calibration_masters/prepared_flat/mod.rs:57-68`. CFA normalization does the
  same independently for three colors at
  `../calibration_masters/prepared_flat/mod.rs:70-116`. Both clamp every response with
  the hard-coded `MIN_NORMALIZED_FLAT = 0.1` at
  `../calibration_masters/prepared_flat/mod.rs:6-7`. `from_images` permits a flat when
  both flat-dark and bias are absent, without policy or warning, at
  `../calibration_masters/mod.rs:373-397`.

  **Impact.** Saturated pixels, dust defects, dead pixels, borders, and non-linear
  regions bias the normalization. A true divisor of `0.05` receives half the required
  correction while remaining apparently valid. Invalid or non-finite prebuilt flats can
  panic inside an API that otherwise returns `Result`, and uncalibrated flat pedestal is
  silently accepted.

  **Change.** Make robust location estimator, allowed response/gain, uncertainty limit,
  minimum survivors, border, and uncalibrated-flat behavior explicit policy. Estimate
  each color using only valid linear samples. Store invalid coordinates in the mask with
  a safe arithmetic placeholder (normally one), never as silently corrected science.
  Return typed construction errors for invalid normalization.

  **Validation.** Inject saturation, NaN/Inf, dead sites, dust shadows, borders, and a
  known `0.05` response into small Mono/Bayer/X-Trans flats. Assert robust normalization,
  exact `FLAT_INVALID` masks and placeholders, and distinct reject/display-clamp policy
  results. Verify every color has an independently valid normalization.

- [ ] **P1 — Make defect classification, union accounting, and failed repairs explicit.**

  **Contract.** Cold detection requires a positive local reference and a configured
  minimum neighbor count (`02-calibration.md:930-956`). Repair requires adequate spatial
  directions and retains defect/interpolated bits; underconstrained clusters remain
  scientifically invalid (`02-calibration.md:1017-1039`). Summary counts describe the
  coordinate union.

  **Evidence.** `detect_cold_pixels` classifies solely by
  `data[i] < dead_fraction * local`, with no `local > 0` or neighbor-count result, at
  `../calibration_masters/defect_map/mod.rs:432-465`. Neighbor estimators return the
  original defective center if no unmasked neighbor exists at
  `../calibration_masters/defect_map/mod.rs:664-707` and
  `../calibration_masters/defect_map/mod.rs:750-788`. `count` and `percentage` add hot and
  cold list lengths at `../calibration_masters/defect_map/mod.rs:133-142`, while
  correction iterates their concatenation even when one coordinate appears in both at
  `../calibration_masters/defect_map/mod.rs:170-183`.

  **Impact.** Nonpositive flat neighborhoods can be classified nonsensically; an
  isolated cluster with no usable support is reported as corrected while retaining its
  original bad value; overlapping hot/cold sites are counted and processed twice. With
  no output mask, none of these states is visible downstream.

  **Change.** Represent one sorted coordinate union with class bits. Have neighbor
  gathering return support count and directional coverage, require configured minima,
  and return a repair result that distinguishes filled from underconstrained. Keep all
  such coordinates masked. Validate indices are unique and in range at every public or
  deserialization boundary.

  **Validation.** Cover a hot+cold overlap, negative/zero local reference, border sites,
  fully surrounded defects, a full bad column, and adequate/inadequate opposing support.
  Assert exact union counts, class bits, fill status, values, and zero downstream weight.

- [ ] **P1 — Validate and make cosmic-ray rejection mask-producing, cancellable, and sampling-safe.**

  **Contract.** Section 8 requires an input bad/saturation/source mask, cumulative cosmic
  mask, validated positive noise parameters, two-stage growth for the normative
  L.A.Cosmic path, and separation of detection from optional fill
  (`02-calibration.md:1041-1241`). Bayer rejection requires a camera/optics sampling gate
  because phase-plane stellar PSFs are undersampled (`02-calibration.md:1185-1200`).

  **Evidence.** `CosmicRayConfig` has unrestricted public fields and no validation at
  `../calibration_masters/cosmic_ray.rs:41-93`. Parametric noise divides by
  `gain * full_scale` without checking gain, read noise, or scale at
  `../calibration_masters/cosmic_ray.rs:228-269`. Detection performs one neighbor-growth
  pass at `../calibration_masters/cosmic_ray.rs:271-317`, destructively fills in place,
  and returns only a count. Bayer is always deinterleaved when selected at
  `../calibration_masters/cosmic_ray.rs:353-385`. `calibrate_align_stack` validates only
  star detection before work (`../pipeline/streaming.rs:40-66`), then calls cosmic
  rejection without a token or safety policy at `../pipeline/streaming.rs:101-133`.

  **Impact.** NaN, zero, or negative physical parameters can produce non-finite or
  meaningless significance. Tight stellar cores can be removed on undersampled Bayer
  data. Cancellation cannot interrupt multiple full-frame median/Laplacian passes, and
  the only scientific evidence—the detected mask—is discarded after values are
  synthesized.

  **Change.** Add `CosmicRayConfig::validate`, accept source/mask/variance plus a cancel
  token, return a cumulative mask separately from an optional cleaned view, and expose
  the documented two-stage growth exactly. Require a validated sampling profile before
  Bayer mode; default to stack-time transient rejection when dithered exposures exist.
  Keep the X-Trans method explicitly identified as its documented heuristic rather than
  L.A.Cosmic equivalence.

  **Validation.** Lock exact primary, first-growth, lowered-growth, and cumulative masks
  on a hand-computed fixture. Sweep every invalid scalar, cancellation boundary, borders,
  masked defects/saturation, undersampled stars, broad PSFs, emission knots, and
  multi-pixel events. Assert detection output is independent of the chosen fill strategy.

## Batch 3 — make master construction auditable and selectable

- [ ] **P1 — Validate cache semantics and persist provenance, not just schema version.**

  **Contract.** A master bundle must carry source identities/hashes, compatibility key,
  construction algorithm/configuration, quality measurements, masks, variance, and
  validity interval (`02-calibration.md:328-356` and `02-calibration.md:1403-1447`).

  **Evidence.** Cache serialization contains four optional images plus defect index
  lists and dimensions at `../calibration_masters/mod.rs:130-193`. Save publishes
  atomically with magic and version at `../calibration_masters/mod.rs:302-318`, but load
  checks only those framing fields, deserializes, and converts directly at
  `../calibration_masters/mod.rs:320-350`. It does not validate component dimensions/CFA,
  finite pixels, positive prepared divisors, defect ordering/uniqueness/range, or source
  freshness. The loaded map later indexes the image without range checks during repair
  at `../calibration_masters/defect_map/mod.rs:170-183`.

  **Impact.** The document's section 11 claim that the cache contains a “coherent” bundle
  is stronger than the implementation. A stale but well-formed cache can be applied to
  unrelated data; a corrupted semantic payload can introduce non-finite arithmetic or
  panic through out-of-range defect indices while `load` presents an I/O `Result` trust
  boundary.

  **Change.** Persist a canonical source manifest/hash, typed compatibility key,
  construction and decoder schema identities, complete policy/config, quality report,
  mask/variance products, and validity interval. On load, validate every structural and
  numerical invariant and return `InvalidData` for violations. Distinguish schema
  version from algorithm/provenance identity so scientific cache invalidation is
  deliberate.

  **Validation.** Round-trip a valid bundle, then independently corrupt dimensions, CFA,
  divisor sign/finiteness, defect ordering/duplicates/range, algorithm identity, and
  source identity. Each must fail during load before calibration. Changing any source
  file or relevant policy must produce a cache miss/rebuild.

- [ ] **P1 — Add frame-level quality diagnostics and rejection before coordinate clipping.**

  **Contract.** Section 4.2 requires per-color location/scale, saturation/invalid
  fractions, row/column residuals, acquisition fields, flat illumination/gradient, and
  dark-pattern diagnostics. Whole bad exposures are rejected with recorded reasons
  before pixel combination (`02-calibration.md:358-375`).

  **Evidence.** `FrameStats` contains only per-channel median/MAD and one scalar
  quantization sigma at `../frame_store/mod.rs:80-119`. CfaCache accepts every
  dimension-compatible finite frame and passes it into coordinate rejection at
  `../combine/cache/loader/mod.rs:229-277`. There is no calibration-specific frame
  quality report or rejection phase in `from_files`
  (`../calibration_masters/mod.rs:411-503`).

  **Impact.** Pixel clipping cannot reliably remove a light-leaked dark, readout failure,
  globally saturated flat, gross flat gradient, or changed amplifier pattern because
  most or all coordinates move coherently. The master has no record of why a frame was
  retained or rejected, and no measurements from which a caller could audit quality.

  **Change.** Add role-specific `CalibrationFrameMeasurements` and an instrument/profile
  policy. Measure before combination, classify set discontinuities, reject with an
  explicit reason, and persist all measurements. Keep automatic thresholds visible and
  profile-derived; do not hide universal percentages in the builder.

  **Validation.** Inject one whole-frame offset, light leak, saturation band, severe flat
  gradient, exposure mismatch, and amplifier-pattern change into otherwise clean sets.
  Assert exact retained/rejected identities, reasons, diagnostics, and an unchanged
  clean master.

- [ ] **P1 — Model overscan and additive-master selection explicitly.**

  **Contract.** When available, identical amplifier-aware overscan correction and trim
  policy must precede combination for every role (`02-calibration.md:187-210`). Light
  calibration selects exactly one of matched raw dark, bias plus bias-free scalable
  dark, bias alone, or an explicit uncorrected result (`02-calibration.md:212-229`).

  **Evidence.** `CalibrationMasters` stores independent optional `dark`, `bias`, and
  `flat_dark` images with no typed additive mode at
  `../calibration_masters/mod.rs:116-128`. `calibrate` implements only dark-else-bias at
  `../calibration_masters/mod.rs:526-531`; it has no result/report for “no additive
  correction.” No overscan geometry, estimator, trim record, bias-free dark-current
  type, scale, or scale uncertainty appears in the construction or application APIs.

  **Impact.** Cameras that expose overscan cannot remove frame-varying bias drift through
  this architecture. Darks must match exactly, yet compatibility is not enforced, so a
  mismatched dark is currently subtracted as if matched. Future dark scaling added to the
  existing ambiguous `dark` field would risk scaling residual bias and double
  subtraction.

  **Change.** Introduce typed additive modes such as `MatchedRawDark`,
  `ScaledBiasFreeDark { bias, dark_current, scale_model }`, `BiasOnly`, and `None`, each
  with compatibility and uncertainty. Add amplifier/overscan geometry and estimator to
  Stage 1 so every role follows the identical correction/trim policy. Report the
  selected path per calibrated light.

  **Validation.** Use a synthetic amplifier with known row-varying overscan, fixed bias,
  dark current, and glow. Assert exact matched-dark, bias-only, uncorrected, and validated
  scaled-dark results. Prove raw-bias-containing dark is never scaled and bias is never
  subtracted twice.

- [ ] **P1 — Expand the persistent-defect model beyond isolated master outliers.**

  **Contract.** Sections 7.4-7.5 require time-series stability/RTS analysis, validity
  intervals, and coherent row/column/cluster classification
  (`02-calibration.md:958-1015`). These classifications affect whether local repair is
  even geometrically supportable.

  **Evidence.** `DefectMap` stores only hot and cold coordinate vectors at
  `../calibration_masters/defect_map/mod.rs:61-78`. Hot detection receives only the
  combined master dark, and cold detection only the combined prepared-flat precursor at
  `../calibration_masters/mod.rs:382-395`; individual dark time series are gone by then.
  No row, column, connected-component, stability, or validity-interval representation is
  present.

  **Impact.** RTS/fading pixels can look ordinary in a master mean, weak coherent columns
  can fall below isolated-pixel thresholds, and large structures are sent to an
  isolated-neighbor fill that may be underconstrained. A static bundle cannot express
  detector aging or the date range for which its map is valid.

  **Change.** Preserve individual calibrated-dark measurements long enough to run a
  profile-trained stability test; add class bits and validity interval to the defect
  product. Analyze residual components for coherent segments/columns before choosing a
  repair strategy. Keep underconstrained structures masked for stack-time recovery.

  **Validation.** Inject a two-state RTS series with an ordinary mean, a fading pixel, a
  weak coherent column with short gaps, a compact cluster, and detector changes across
  epochs. Assert exact classes, union geometry, validity selection, and repair/no-repair
  decision.

## Batch 4 — reduce API state and finish operational behavior

- [ ] **P2 — Thread cancellation through convenience construction, calibration, and cosmic work.**

  **Contract.** Cancellation is polled between decode, tile/statistic, chunk, and
  iteration boundaries, and cancellation leaves no partially published product
  (`02-calibration.md:1493-1503`).

  **Evidence.** `stack_cfa_master` and `CalibrationMasters::from_files` accept a token,
  and role loading, combining, and defect detection poll it. `CalibrationMasters::calibrate` has no token at
  `../calibration_masters/mod.rs:505-543`, and cosmic iterations have no cancellation
  input at `../calibration_masters/cosmic_ray.rs:95-163` and
  `../calibration_masters/cosmic_ray.rs:407-440`.

  **Impact.** Per-light cancellation can avoid launching decode and can interrupt
  demosaic, but not calibration or optional repeated full-frame cosmic passes already in
  progress.

  **Change.** Accept one caller token in `from_files`, propagate clones to every role and
  defect stage, and define cooperative polling granularity for flat preparation,
  application, and every cosmic iteration/filter pass. Pair this with transactional
  outputs so cancelled work is discarded.

  **Validation.** Cancel before construction, during each role decode/combine, during
  defect detection, after additive subtraction, during flat division, and in every
  cosmic iteration. Assert bounded completion latency, a cancellation error, no cache
  publication, and unchanged input/output state.

- [ ] **P2 — Remove construction-only masters from the application bundle.**

  **Evidence.** `from_images` uses `flat_dark` only to subtract the flat at
  `../calibration_masters/mod.rs:373-380`, but stores the full image in
  `CalibrationMasters` afterward at `../calibration_masters/mod.rs:397-408`.
  `calibrate` never reads `flat_dark` (`../calibration_masters/mod.rs:505-543`), yet
  `components`, `ram_bytes`, cache serialization, and every light's CFA validation retain
  and process it at `../calibration_masters/mod.rs:264-300`,
  `../calibration_masters/mod.rs:130-193`, and
  `../calibration_masters/mod.rs:553-585`. When a dark is present, the stored bias is also
  unused during application because both the light additive choice and prepared flat are
  already resolved.

  **Impact.** A construction input costs another full sensor plane in every live/cache
  bundle and can reject a valid light due to the CFA metadata of a component that is not
  applied. Four independent options allow nonsensical states and force each application
  to rediscover which additive master matters.

  **Change.** Separate `CalibrationBuildInputs` from an immutable application bundle.
  Store one typed resolved additive mode, one prepared divisor, the defect/mask product,
  uncertainty, compatibility, provenance, and quality report. Retain source identities
  and summaries for audit, not unused full pixel planes. Keep raw component masters only
  in a separately named library/cache when callers explicitly need them for rebuilding.

  **Validation.** Assert identical calibrated results before and after the state-model
  change for every valid path, reduced `ram_bytes`/cache size, and construction-time
  rejection of impossible combinations. An unused flat-dark mismatch must not affect a
  resolved light application bundle.

- [ ] **P2 — Replace the overly general master-stack API with a role-aware builder.**

  **Evidence.** `stack_cfa_master` accepts an unrestricted general `StackConfig` while
  its documentation describes dark/flat/bias role presets at
  `../calibration_masters/mod.rs:233-262`. A caller can legally request multiplicative
  normalization for a dark, global additive normalization for a flat, light-frame noise
  weighting, or any unrelated rejection algorithm, and the returned value has no role or
  algorithm provenance. `from_files` happens to pass the intended presets at
  `../calibration_masters/mod.rs:442-490`, but the public API does not preserve those
  scientific constraints.

  **Impact.** The broad generic surface is more flexible than calibration semantics and
  makes scientifically invalid masters easy to construct. The caller must coordinate
  role, subtractor availability, normalization order, validation, and metadata manually;
  the type system records none of those decisions.

  **Change.** Expose a role-aware builder whose configuration contains only meaningful
  calibration choices. Bias/raw-dark/flat-dark roles forbid normalization; the flat role
  requires its additive policy and performs preparation in the required order. Return a
  typed master product carrying role and exact algorithm identity. Keep the generic stack
  engine internal as a reusable mechanism rather than the calibration API.

  **Validation.** Add compile-time/API-shape checks that invalid role/normalization
  combinations cannot be expressed, plus behavior tests for every supported role and
  small-N fallback. Persist and round-trip the exact role and algorithm configuration.

## Documentation corrections

The normative sections 1-10 are internally consistent with the intended scientific
pipeline. The informative current-state section should be corrected in three places:

- `02-calibration.md:1517-1519` describes role presets as having propagated
  quantization floors. The current Winsorized branch uses the maximum one-frame sigma,
  not propagation through the final mean's survivor coefficients.
- `02-calibration.md:1527-1528` calls the cached bundle coherent. Atomic publication and
  schema versioning are implemented, but construction/loading do not establish complete
  component coherence and the cache has no scientific provenance validation.
- `02-calibration.md:1546` mentions a possible dimension panic after the calibrated flag
  is set, but the consequence should explicitly include a permanently partially mutated
  input: dark subtraction can succeed before a later flat/defect assertion fails.

The existing section 11 priority table otherwise matches the inspected production tree.

## Open questions

1. Is the target product unambiguously a scientific image+mask+variance pipeline, as
   sections 1-10 specify, or must Lumos also retain a lighter display-only calibration
   mode? If both are required, they should be distinct result types and policies rather
   than one mode silently clamping invalid flats.
2. Which RAW acquisition fields can the pinned LibRaw integration recover reliably for
   the initial compatibility key, and which require an instrument profile or sidecar?
   Missing mandatory facts must result in an explicit unverifiable policy outcome.
3. Should cached masters embed full source manifests and hashes, or reference a separate
   calibration library database? Either design must bind pixels to exact inputs,
   decoder/algorithm schema, policy, and quality report.
4. What camera/optics measurement will authorize Bayer single-frame cosmic rejection?
   Until a sampling gate is defined, the scientifically safe default is to leave it off
   and rely on registered multi-frame rejection.

## Recommended implementation order

1. Fix flat construction order, complete application/config validation, make calibration
   transactional, and correct the Winsorized scalar uncertainty regression.
2. Introduce the CFA image+mask+variance+provenance product and carry it through stacking;
   then replace flat flooring and destructive defect/cosmic semantics.
3. Add typed compatibility/master metadata, cache validation/provenance, frame-quality
   rejection, and explicit additive selection.
4. Add overscan, validated dark scaling, RTS/structured defects, and the normative cosmic
   growth/sampling policies on top of those foundations.
5. Collapse the runtime bundle to resolved application state, make master building
   role-aware, and finish end-to-end cancellation.

This order prevents advanced calibration algorithms from being built on products that
still cannot represent invalid samples, uncertainty, compatibility, or transactional
failure.
