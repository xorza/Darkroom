# Review: `lumos`

Scope: `lumos/src`, read for data structures, pipelines and the public API of every pipeline;
callers in `lens` read to judge the API. Tests ran once: 1518 pass, the three binaries in 3.3 s.
Nothing else was executed: every finding below is from reading the code, and a figure marked
*estimate* is arithmetic, not a measurement.

The kernels are in good shape. No wrong constant, off-by-one, swapped bound or wrong inverse was
found in the rejection methods, the mean, the resampling, the drizzle kernels, the demosaics, the
L.A.Cosmic port, the centroid fits or the RANSAC. What the review found sits above the kernels: in
what the pipelines decide, in the types the configurations and results are made of, and in the three
front ends that grew side by side.

# Part 1 — Proposal

## Root causes

1. **A rule that a type could hold is checked at run time.** A configuration can spell a state
   that `validate` then refuses, a per-pixel branch guards, or nothing catches.
   `Combine`/`CombineMethod`/`SmallN`/`Rejection::None`, `Weighting::Manual`,
   `AlignStackConfig::cosmic_ray`, `DrizzleConfig` (a Point kernel drops `pixfrac` silently,
   Lanczos is refused unless `scale == pixfrac == 1`), `FwhmConfig`, `TransformModel`,
   `WarpConfig::border_value`, `CalibrationOutcome`, the three coupled facts that decide which
   masters a light loses.
2. **One fact has two homes.** The copies drift, or one is dead. Decode policy in three types;
   saturation in `ObservationHeader::data_max` and `DerivedFacts::saturation_flagged`; the
   saturated-star policy in the detector and in registration, with opposite contracts; row order in
   `ROWORDER` and `LUMROWO`; `FrameAdmission::first_facts` beside `facts[0]`; `RunReport` counters
   mirrored by `CalibrationNotes` and written by four stages after the fact; `rms_error`,
   `median_fwhm`, `AlignStackResult::reference`, `Coverage::Uniform::size`, `LAYER_SIGMA`.
3. **Three front ends, three planners.** The combine loader, the registered pipeline and the
   drizzle each decode, admit, plan memory, check the disk and store frames by their own code and
   their own vocabulary. They have drifted: the drizzle plan counts RAM bytes as disk writes and
   drops a held frame the other two charge. The drizzle cannot be reached from RAW lights without
   calibrating every light twice.
4. **A capture is compared on too little, and a few policies depart from the field without a
   record.** Darks, biases and flats are matched to a light on exposure and temperature only.
   `DATAMAX` is read as a saturation level, which the FITS standard does not say. The quality
   planes state a variance that clipping made larger. Bright stars with seeing wings are dropped by
   a pixel-count limit.
5. **State shared between workers where a return value or a borrow serves.** Counters in atomics
   that a `reduce` or the returned values already give, a union-find in atomics over sets no two
   workers share, a pool lease that holds an `Arc` only to coexist with a `&mut` of its owner.
6. **Sibling entry points take the same thing three ways.** Construction (`StarDetector::new`
   panics, `RegistrationConfig::registrar()` borrows, stacking validates inside), cancellation
   (`ProgressMonitor`, `&CancelToken`, `&LoadCtx`), the masters' cache (a generic parameter and a
   generic error).

## Findings that are bugs

Each is fixed by the structure in the batch named; none is dropped.

| # | Where | Defect | Batch |
|---|---|---|---|
| B1 | `lumos/src/io/fits/decode/pixels/mod.rs:120-131` | `DATAMAX` is taken as a saturation level and every sample at or above 95 % of it is flagged `SATURATED`. FITS 4.0 §4.4.2.5 defines it as the array's own maximum; the saturation reading is the SBIG/MaxIm extension. A flat from a standard-conforming writer has its plateau flagged, the master keeps the flag, and `imposed_flags` turns it into `NO_DATA` on every light. | 1 |
| B2 | `lumos/src/drizzle/drizzle_shape.rs:67-70`, `lumos/src/frame_store/run_storage.rs:62-64` | `DrizzleShape::tier` adds the shared gains' RAM bytes to `RunWrites::kept`. A file drizzle of gain-carrying lights with no decode cache, on the resident tier, reaches `debug_assert_eq!(writes.kept, 0, …)`. Release over-asks the disk. No test reaches it. | 2 |
| B3 | `lumos/src/drizzle/drizzle_run.rs:112-116` | The drizzle plan drops `peek.held`, which the loader and the pipeline charge: a decoded frame held unplanned. | 2 |
| B4 | `lumos/src/calibration/error.rs:233-234`, `calibration_masters/mod.rs:166` | `MasterBuildError::Cache(E)` prints its cause in `Display` and has no `source`, the form the root `AGENTS.md` refuses. A failed `cache.store` returns before `Ok(Some(master))`: a read-only frames folder fails the build after the role is stacked. | 4 |
| B5 | `lumos/src/image/cfa/cfa_lattice.rs:151-153`, `calibration/defect_map/mod.rs:284-288` | A defect whose every stencil neighbour is masked keeps its own value and is flagged `DEFECT | REPAIRED`. | 4 |
| B6 | `lumos/src/calibration/calibration_masters/mod.rs:344-386` | `from_images` drops a flat-dark given with no flat; `build` refuses the same set with `FlatDarksWithoutFlats`. | 4 |
| B7 | `lumos/src/ingest/frame_admission.rs:97-114` | `first_facts` is set only by `admit_measured`. When frame 0 is reused from the decode cache it stays unset, and a mismatched set decodes and writes every new frame before `SetFacts::check_set` refuses it. | 2 |
| B8 | `lumos/src/io/fits/metadata/mod.rs:326-338` | Rows are never reordered at load, and every write states `ROWORDER = 'TOP-DOWN'` with the true order in a private `LUMROWO`: a recorded decision of the code, reversed here. A frame from a bottom-up source is written with bottom-up rows under a top-down label; a reader that takes the keyword for the display orientation shows it mirrored against its sub-frames. | 1 |
| B9 | `lumos/src/combine/stack/mean_combine.rs:62-68` | A small-N median pixel reads inverse variance 0, the value of a pixel no frame reached, pixels of one and two samples included, where the median is a linear combination with an exact variance. Under `StackConfig::light()` that is every pixel fewer than five frames reach. | 3 |
| B10 | `lumos/src/drizzle/output_grid.rs:35` | The output size is `(width as f32 * scale).ceil()` while the mapping uses `f64::from(scale)`: for an inexact scale the two disagree by a pixel (3000 × 1.1 is 3300 in `f32`, 3301 in `f64`). | 8 |
| B11 | `lumos/src/stack_product/mod.rs` (`fill_where`) | A `Coverage::Uniform` is skipped by the fill, so it would keep stating 1.0 at filled pixels. Unreachable today: a drizzle is always per pixel. | 3 |

## Findings on what the pipelines decide

| # | Where | Finding | Decision taken |
|---|---|---|---|
| P1 | `lumos/src/frame/capture_conditions/mod.rs:13-21`, `calibration/dark_match/mod.rs:48-103` | Gain, ISO, offset and binning are read into the header and compared nowhere; a master states its first frame's. | Compared where both sides state them; a difference refuses a bias or a dark, an unstated side is counted as unverified. |
| P2 | `lumos/src/calibration/dark_scale.rs:17,85-103` | The dark-scale fit is accepted on a pixel count; its standard error is computed and not read. The pixels it is fitted on are all replaced by the defect repair, so the factor acts only where it was not measured. | Accepted on its relative standard error against the uncertainty of the factor it replaces. |
| P3 | `lumos/src/calibration/cosmic_ray/noise_model/mod.rs:58-66` | The measured noise model divides by the sky as it stands; with a pedestal still on the light the photon term is understated (3.5× at a pedestal of 2.5 skies, *estimate*). | A kept pedestal is subtracted; an unknown one with no bias removed refuses `Measured` without a gain. |
| P4 | `lumos/src/calibration/defect_map/mod.rs:354-360` | The hot threshold is in the master's σ, which falls with the frame count, and the flagged share has no bound. | At most 1 % of a colour's photosites, the share Siril flags in red; past it the cut rises to the quantile that holds it, and the summary says so. |
| P5 | `lumos/src/star_detection/star_detector/stages/detect/mod.rs:201` | `max_area` (500, 200 in `high_resolution`) drops single-peak stars whose footprint above threshold exceeds it: a Moffat β 2.5 star of FWHM 6 above peak SNR ≈ 455 (*estimate*). A Gaussian never trips it, so no synthetic test sees it. | `max_area: Option<usize>`, `None` by default, as SExtractor's `DETECT_MAXAREA 0`. |
| P6 | `star_detection/star_detector/stages/filter/mod.rs:35`, `registration/register.rs:56-72` | The filter drops every saturated star; registration is written to receive them flagged. `Star::saturated` is false on every star a result holds, and reads one pixel. | The detector keeps and flags them, by any saturated pixel in the core; the final fit already leaves them out. |
| P8 | `lumos/src/bg_mesh/tile_stats/mod.rs:16` | A tile's statistics read at most 1024 samples on a fixed column lattice: a 128 px tile a sixteenth of its pixels, columns ≡ 0 and 3 (mod 4) never. | Every pixel of the tile. |
| P9 | `star_detection/star_detector/stages/prepared_frame/mod.rs:242-257` | Channel noise is measured again inside `detect`, on a demosaiced frame from interpolated pixels, where `FrameStats` reads the mosaic's. | `detect` reads the metadata's mosaic noise where it is stated. |
| P10 | `star_detection/centroid/measure_grid.rs:15,29` | Flux and SNR are sums over a square of side `2⌈1.75·FWHM⌉ + 1`: a FWHM of 3.99 against 4.01 moves the pixel count from 225 to 289 and the SNR by 12 %. | A circular aperture of `1.75·FWHM` with partial-pixel weights. |
| P11 | `lumos/src/registration/model_search.rs:95-101` | Under `Auto { sip: Some }` every candidate carries the correction: a light with 8–20 pairs registers under `default()` and is dropped under `precise()` and `wide_field()`. | Plain and SIP candidates compete under the GRIC already implemented. |
| P12 | `registration/final_fit/fit_catalogs.rs:98-127,188` | A pair's variance is its formal centroid variance times one median scale; a floor that does not shrink with SNR gates out the best-measured stars (≈ 58 % at σ 0.01 px under a 0.03 px floor, *estimate*). | An additive `σ_excess²` estimated per fit. Confidence medium: the test in batch 6 comes first. |
| P13 | `registration/distortion/sip/mod.rs:376-408` | The Euclidean+SIP angle is found by a secant iteration that refactors an angle-independent design each step. | The closed form `θ = atan2(S, C)` on the projected vectors. |
| P14 | `combine/reducer/combined_sample.rs:65`, `combine/stack/mean_combine.rs:99` | Both variance planes ignore what choosing the survivors costs: the light preset understates the variance by ≈ 6 %, 12–14 % where the noise floor does not bind (Monte Carlo of the first pass, outside the code). | Both planes state the variance of the clipped mean. |
| P15 | `combine/normalization/mod.rs:178-196` | The reference frame is the one of lowest raw σ before the frames share a scale: a hazy frame qualifies. | The norms are re-expressed on the frame of smallest `gain·σ` after the fit, an exact rescale. |
| P16 | `lumos/src/io/fits/decode/plan.rs:373-380` | `preview` refuses a third-party three-plane FITS, and clamps a RAW mosaic but not the same mosaic from FITS. | `preview` reads a cube as RGB and clamps on one rule. |
| P17 | `lumos/src/noise/mrs_noise.rs:22` | `LAYER_SIGMA` repeats, to three digits, what `wavelet::white_noise_sigma` computes. | Derived. |

## Decisions

- **Refused: one entry per operation over an input enum.** The entries stay methods of their
  configuration, one per source, as `lumos/AGENTS.md` decides and as `image::open` stands beside
  `ImageReader::new`. What changes is what the entries share: their run, their planner and their
  argument order. Distinct constructors named by their source are the Rust form
  ([C-CTOR](https://rust-lang.github.io/api-guidelines/predictability.html#c-ctor)).
- **A rule lives in the type.** A value with a range is a checked newtype with `checked` and
  `new`, as the root `AGENTS.md` sets; a field one kind alone reads moves into that kind's variant
  ([C-CUSTOM-TYPE](https://rust-lang.github.io/api-guidelines/type-safety.html#c-custom-type),
  [parse, don't validate](https://lexi-lambda.github.io/blog/2019/11/05/parse-don-t-validate/)).
  Refused: more `validate` cases, which leave the state spellable.
- **`Combine` is three cases.** `Median`, `Mean`, `RejectedMean { rejection, small_n }`. Refused:
  `Mean(Rejection::None)`, a "no rejection" spelled inside the rejection; a fallback typed as a
  combine method, which admits a rejecting one. The reducer is chosen once, not per pixel
  ([push ifs up](https://matklad.github.io/2023/11/15/push-ifs-up-and-fors-down.html)).
- **`Weighting::Manual` goes.** No code in the workspace builds one, and `lens` leaves it out by
  design. It costs `validate(frame_count)`, two errors, `for_survivors` and `Copy`. When a caller
  exists the weight rides on the frame, as IRAF `imcombine` (`weight = @file | !keyword`), ccdproc
  (`Combiner.weights`) and PixInsight (a keyword per file) hold it per frame. Refused: a `Vec` in
  the configuration, which no stacker keeps in a preset.
- **Resampling is the one step of a registered run that varies.** `Resample::{Warp, Drizzle}` on
  `AlignStackConfig`, as Siril and PixInsight offer drizzle as an option of the registered stack.
  Refused: a drizzle front end of its own, which is where B2 and B3 came from and which makes a RAW
  drizzle calibrate each light twice.
- **The warps-in-hand drizzle stays, on `DrizzleStackConfig`.** A caller with its own registration
  or per-pixel weight maps keeps a route, three entries where there were four. Refused: drizzle
  reachable end to end only, which drops caller-supplied warps and weights.
- **Mosaic lights under `Resample::Drizzle` deposit by colour**, registered on the green proxy.
  A demosaic before a drizzle is two interpolations, which `Resample::Warp` already is.
- **`DATAMAX` is the array's maximum**, "the maximum valid physical value represented by the
  array" ([FITS keyword dictionary](https://heasarc.gsfc.nasa.gov/docs/fcg/standard_dict.html)):
  it still decides a float HDU's scale, as `io/fits/decode/plan.rs` has it, and flags nothing. A
  saturation level is read from and written to `SATURATE`, the default `SATUR_KEY` of SExtractor's
  `default.sex`; a written file carries no `DATAMAX`. The MaxIm DL reading, "pixel values above
  this level are considered saturated"
  ([header definitions](https://cdn.diffractionlimited.com/help/maximdl/FITS_File_Header_Definitions.htm)),
  is a stated choice of the load.
- **`ROWORDER` states the rows as stored**, the source's order, as the keyword is "order of the
  rows in image array" and serves "displaying the image with the intended orientation"
  ([Siril, FITS orientation](https://siril.readthedocs.io/en/stable/file-formats/FITS.html)).
  `BAYERPAT` goes out in that convention by the inverse of the read rule, so a reload converts
  once, which is what the `TOP-DOWN`-always rule was for. A frame with no source order is written
  `TOP-DOWN`. Refused: `TOP-DOWN` always with the truth in `LUMROWO`, which only lumos reads;
  reordering the rows on write, which Siril's page forbids for stacking and which mirrors a master
  against its sub-frames.
- **A sensor setting both sides state must agree** for a bias or a dark: gain, ISO, offset,
  binning, as [Siril's calibration](https://siril.readthedocs.io/en/latest/preprocessing/calibration.html)
  asks, though Siril itself checks none of them. A side that states nothing is counted as
  unverified. A filter that differs between flat and light is recorded, not refused. Refused:
  recording a stated difference and calibrating all the same, which leaves a wrong-offset bias or
  a wrong-ISO dark in the stack behind a counter.
- **The detector keeps a saturated star and flags it**, by any saturated pixel in its core, as
  SExtractor's `FLAGS` bit 4, "at least one object pixel is saturated"
  ([flagging](https://sextractor.readthedocs.io/en/latest/Flagging.html)). Registration matches
  on them and fits without them, as it is already written to. Refused: the filter dropping them
  and registration's handling deleted, which keeps the brightest stars out of the pattern match; a
  filter option, a knob nothing needs.
- **Under `WithSip(Auto, _)` the plain models compete with the SIP ones**, and the GRIC picks
  (Torr 1998, a criterion made for nested models), charging a SIP candidate its two parameters a
  term. `most_general()` is then a homography, which sizes the star gates. Refused: SIP
  candidates alone, which drop a light of 8–20 pairs that a plain model fits; SIP at every order
  up to the configured one, three times the final fits a light for a case no dataset has shown.
- **A star's flux and SNR are measured over a circle of radius `1.75·FWHM`**, each boundary
  pixel weighed by its exact area inside the circle, as photutils'
  [`CircularAperture`](https://photutils.readthedocs.io/en/stable/api/photutils.aperture.CircularAperture.html)
  does by its `exact` method. The radius is not rounded, so neither figure steps with the FWHM.
  Refused: the square of side `2⌈1.75·FWHM⌉ + 1`, documented, which keeps a 12 % step in the SNR;
  a PSF-weighted SNR, which needs a profile fit the moments path does not have.
- **A region has no maximum area by default**: `max_area` is `None`, as SExtractor's
  `default.sex` ships `DETECT_MAXAREA 0`, unlimited. Refused: a limit derived from the FWHM, which
  needs a profile to turn brightness into area; no field at all, which leaves no cap for a caller
  that wants one.
- **The variance planes state the clipped mean's variance**, `σ²/(n·Z·tv)` for a band of ±k
  (Huber, *Robust Statistics*, §3.2). Refused: the variance conditional on the survivors, which
  reads up to 14 % low and which no plane's reader can correct, not knowing the band.
- **A small-N pixel of one or two samples takes the weighted mean**, whose variance is exact. Of
  three or more it keeps the median, which still rejects one outlier among three, and states
  `f(n)·mean(vᵢ)/n`: the equal-weight model variance times the Gaussian median's small-sample
  factor (1.346 at 3, 1.193 at 4, π/2 in the limit), exact for samples of one variance and an
  approximation otherwise, as the plane's doc says. Refused: 0, which a consumer reads as no
  data; a mean fallback, which lets a trail at a dither edge into the stack.
- **A dark-scale fit is taken on its precision.** It replaces the exposure ratio when its
  relative standard error is below that ratio's own uncertainty: 12 % a degree of temperature
  difference, the code's dark-current figure, floored at the tolerance; a third where no
  temperature compared, a fit significant at 3σ. PixInsight accepts its scale on a noise
  criterion ([master frames](https://pixinsight.com/tutorials/master-frames/)). Refused: a pixel
  count, a proxy for the standard error the fit already has; fitting only on a stated mismatch,
  which leaves a dark that matched on paper alone uncorrected.
- **The masters' cache is a `&dyn` and cannot fail a build.** `lens` picks it at run time, the
  case dynamic dispatch is for; lumos composes the key, so no ingredient is left out by an
  implementor. A miss on load and a warning on store. Refused: the generic kept with its error
  given a `source`, which keeps seven public types and the hand-composed key; failures handed
  back beside the masters, a boxed error and a result struct for an optimisation's trouble.
- **`preview` reads a three-plane FITS cube as RGB**, whatever `FitsCubeInterpretation` says, and
  clamps a mosaic's preview to `[0, 1]` from either container, recorded as `clipped`. It is the
  display loader, beside JPEG and PNG of unknown transfer; `linear` and `cfa` keep refusing an
  undeclared cube. Refused: each caller setting `cube: Rgb`, which leaves the default context
  failing on the masters every other stacker writes.
- **Not taken: a first pass that keeps stars only under `Reference::Auto`.** On the spilled tier
  it saves a 13 B/px write and read-back of each light for a second decode, and for RAW lights a
  second calibration, cosmic-ray pass and demosaic. A measurement decides it; it goes to
  `.notes/PERF.md`, not to this plan.
- **Not taken: the offset from the paired sample medians.** It removes a pass over each spilled
  plane and widens the offset's error from 0.0003σ to 0.005σ; `lumos/AGENTS.md` ranks precision
  first. The common domain built at store time is taken: it is bit-identical.
- **Not taken: an output canvas other than the reference's.** A framing choice (Siril's
  min/max/cog) is a capability, not a defect. The preset named `mosaic` is renamed for what it is.
- **Kept: `image` ↔ `io`.** The only top-level cycle, through `LinearImage::write_fits`,
  `write_raster` and `CfaImage::write_fits`: a type and its codec, placed by the I/O decision of
  `lumos/AGENTS.md`.
- **Kept: the bundle.** `read_bundle` and `write_bundle` have no caller in the workspace; they are
  the masters' public file form, beside the per-role cache.
- **Kept: thin-plate splines**, by `lumos/AGENTS.md`. Joining a warp needs
  `enum Distortion { Sip, Tps }`; not in this plan.
- **Kept: `QualityPlanes` as four `bool`s.** All sixteen combinations are legal.
- **Kept: two separations in detection.** `deblend_min_separation` says which peaks are two
  stars; `duplicate_min_separation` drops the fainter of two stars nearer than it, the
  `min_separation` of photutils'
  [`DAOStarFinder`](https://photutils.readthedocs.io/en/stable/api/photutils.detection.DAOStarFinder.html).
  One knob for both would change which stars a result holds, for no defect shown.
- **Kept: `FileSet`'s take-once cells.** `first` and `probes` hand each path's probe to the job
  that decodes it, by index, through `&self`. Moving the probes into `par::slots_owned` moves the
  lock to another module, as that is itself a `Vec<Mutex<Option<T>>>`, and costs an enum of job
  items over three callers.
- **The depth a drizzle's fill gate reads comes out of the combine**, which already gathers each
  frame's drop weight at each pixel. Refused: a depth plane a worker, an output plane for each
  frame in flight; a pass over the stored frames after the stage, a second read of a spilled set.
- **`LinearImage` and `CfaImage` are one generic `Frame<S>` under two aliases**, as the `image`
  crate's [`ImageBuffer`](https://docs.rs/image/latest/image/struct.ImageBuffer.html) stands
  under `RgbImage` and `GrayImage`. Refused: two structs with the same three companions, whose
  checks have already drifted (`assert!` in one `set_flags`, `debug_assert!` in the other).
- **A reference catalog owns its stars.** A `Star` is `Copy` and a catalog a few thousand of
  them; a borrow would tie the catalog to the frame the stars were moved out of.

## Data structures

Types not listed are unchanged.

### Combine — `lumos/src/combine/stack_config/`

```rust
// combine.rs
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Combine {
    Median,
    Mean,
    RejectedMean { rejection: Rejection, small_n: Option<SmallN> },
}
impl Combine {
    pub const fn rejected(rejection: Rejection) -> Self;   // small_n = rejection.small_n()
    pub const fn sigma_clip(sigma: f32) -> Self;           // sugar over `rejected`
    pub const fn winsorized(sigma: f32) -> Self;
    pub const fn linear_fit(sigma: f32) -> Self;
    pub const fn trim(percent: f32) -> Self;
    pub const fn gesd() -> Self;
    pub(crate) const fn planes(self, requested: QualityPlanes) -> QualityPlanes;
}

// small_n.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SmallN { pub min_frames: usize, pub fallback: Fallback }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fallback { Median, Mean }

// weighting.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Weighting { Equal, Noise }

// mod.rs
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StackConfig {
    pub combine: Combine,
    pub weighting: Weighting,
    pub normalization: Normalization,
    pub quality: QualityPlanes,
    pub min_survivors: usize,
}
impl StackConfig {
    pub(crate) fn validate(&self) -> Result<(), StackConfigError>;
    pub(crate) fn combine(&self, cache: &FrameCache) -> Result<Combined, StackError>;
}

// combine/stack/mod.rs
#[derive(Debug)]
pub(crate) struct Combined {
    pub(crate) product: StackProduct,
    pub(crate) depth: Option<ArrayVec<Buffer2<f32>, 3>>,
}
// combine/stack/rejected_mean.rs
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct RejectedMean {
    pub(crate) rejection: Rejection, pub(crate) small_n: Option<SmallN>,
    pub(crate) min_survivors: usize, pub(crate) quality: SampleQuality,
}
impl RejectedMean {
    pub(crate) fn reducer(self) -> impl Fn(PixelSamples<'_>, &mut ScratchBuffers) -> CombinedSample + Sync + Copy;
}
// combine/frame_cache/mod.rs
#[derive(Debug, Clone, Copy, Default)]
struct RowTallies { excluded: FlagCounts, kept_flagged: FlagCounts, small_n_fallback_pixels: u64 }
impl RowTallies { fn merge(self, other: Self) -> Self; }
#[derive(Debug, Clone, Copy)]
pub(crate) struct SlotTerms { pub(crate) norm: SlotNorm, pub(crate) weight: f32, pub(crate) noise: Option<NoiseAt> }
```

- `Combine`: how one pixel's samples become a value. A small-N policy exists only beside a
  rejection, and its fallback cannot reject: both by the enum.
- `SmallN::min_frames` is at least 2, held by `StackConfig::validate`; 0 and 1 would be a second
  spelling of `None`.
- `Rejection` loses `None` and `Default`; gains
  `pub const fn small_n(&self) -> Option<SmallN>`, the one home of the frame floors (5 for sigma
  clip and linear fit, 15 for GESD, none for Winsorized and trim).
- `Combined::depth`: the drops' weight of every drizzled frame gathered at a pixel, summed
  before any frame weight or rejection, one plane a weight plane; `None` for frames not drizzled.
- `SlotTerms`: one frame's norm, weight and noise in one slot, resolved once a run from one
  `NormalizedStats`; identity and unit values stand where a frame has none, so the gather matches
  on nothing a sample.
- `CombineMethod`, `SmallN::none`, `SmallN::falls_back`, `StackConfig::for_survivors`,
  `StackConfigError::{RejectingSmallNFallback, InvalidManualWeight, InvalidManualWeightSum,
  ManualWeightCountMismatch}`, `CombinePlan` and `FrameSet` are deleted. The combine takes
  `&StackConfig`.

### Rejection configs — `lumos/src/combine/rejection/`

```rust
pub struct TrimConfig { pub percent: TrimBounds }
pub struct TrimBounds { pub low: f32, pub high: f32 }       // `symmetric`, `asymmetric`, as SigmaBounds
pub struct GesdConfig { pub alpha: f32, pub outliers_fraction: f32 }   // 0.3, as PixInsight and Siril
```

The five positional `new`s go: a struct literal over `DEFAULT` is the one route, and
`Rejection::sigma_clip(σ)` its sugar.

### Product — `lumos/src/stack_product/`

```rust
pub struct StackProduct {
    pub image: LinearImage,
    pub coverage: Option<Coverage>,
    pub weight: Option<WeightPlane>,
    pub inverse_variance: Option<QualityMap>,
    pub dispersion: Option<QualityMap>,
    pub cfa_type: Option<CfaType>,
    pub report: RunReport,
}
pub struct WeightPlane { pub map: QualityMap, pub kind: WeightKind }
pub enum Coverage { Full, PerPixel(QualityMap) }
impl Coverage { pub fn to_plane(&self, channel: usize, size: USize2) -> Buffer2<f32>; }

// run_report.rs
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RunReport {
    pub combine: CombineReport,
    pub memory: MemoryReport,
    pub calibration: Option<CalibrationReport>,
    pub drizzle: Option<DrizzleReport>,
}
pub struct CombineReport {
    pub excluded_samples: FlagCounts, pub kept_flagged_samples: FlagCounts,
    pub small_n_fallback_pixels: u64, pub variance_bg_only: bool,
}
pub struct MemoryReport {
    pub spilled_frames: u64, pub parked_lights: u64,
    pub overcommit_bytes: u64, pub planned_peak_bytes: Option<u64>,
}
pub struct CalibrationReport {
    pub unverified_dark_exposures: u64, pub unverified_dark_temperatures: u64,
    pub unverified_settings: u64, pub camera_temperature_darks: u64,
    pub dark_temperature_differences: u64, pub scaled_darks: u64, pub fitted_dark_scales: u64,
    pub filter_differences: u64,
    pub flat_dark: Option<DarkComparison>, pub floored_flat_pixels: u64,
}
pub struct DrizzleReport { pub unconverged_points: u64 }
```

- Each stage returns its group whole. `None` says the stage did not run, which a zero could not.
  The memory group is the one that passes through: the entry that holds the plan makes it
  (`PlannedTier::report`), `CacheCore` carries it into the combine, and the combine raises
  `overcommit_bytes` by its own stage's before the product takes it.
- `WeightKind` is written (`LUMWGTK`) and required only beside a `WEIGHT` extension; `LUMCOVU`
  becomes a logical for `Coverage::Full`.
- `CalibrationReport` implements `FromIterator<CalibrationOutcome>` and takes the masters' two
  facts through `with_masters(self, &CalibrationMasters) -> Self`.

### Calibration — `lumos/src/calibration/`, `lumos/src/frame/`

```rust
// frame/capture_conditions/mod.rs
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub(crate) struct CaptureConditions {
    pub(crate) exposure_time: Option<f64>,
    pub(crate) ccd_temp: Option<f64>,
    pub(crate) camera_temp: Option<f64>,
    pub(crate) setting: SensorSetting,
}
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub(crate) struct SensorSetting {
    pub(crate) gain: Option<f64>, pub(crate) iso: Option<u32>,
    pub(crate) offset: Option<i32>, pub(crate) binning: Option<[i32; 2]>,
}
// frame/capture_condition.rs
pub enum CaptureCondition { Exposure, Temperature, CameraTemperature, Gain, Iso, Offset, Binning }
// calibration/error.rs
pub enum DarkMismatch {
    Exposure { frame: f64, dark: f64 },
    Setting { condition: CaptureCondition, frame: f64, master: f64 },
}
pub enum MasterBuildError { Stack(MasterStackError), Calibration(CalibrationError), Cancelled }

// calibration/calibration_masters/mod.rs
#[derive(Debug, Default)]
pub struct CalibrationMasters {
    additive: Additive,
    flat: Option<PreparedFlat>,
    defect_map: Option<DefectMap>,
    imposed: Option<PixelFlags>,
}
#[derive(Debug, Default)]
enum Additive {
    #[default] None,
    Bias(CfaImage),
    DarkWithBias(CfaImage),
    Thermal(CfaImage),
    BiasAndThermal { bias: CfaImage, thermal: CfaImage },
}
#[derive(Debug, Clone, Copy)]
pub struct MastersConfig<'a> { pub hot_sigma: SigmaThreshold, pub cache: Option<&'a dyn MasterCache> }
impl CalibrationMasters {
    pub fn build<P: AsRef<Path>>(
        frames: CalibrationSet<&[P]>, config: &MastersConfig<'_>,
        ingest: &IngestConfig, monitor: ProgressMonitor,
    ) -> Result<Self, MasterBuildError>;
    pub fn from_images(
        images: CalibrationSet<Option<CfaImage>>, hot_sigma: SigmaThreshold, cancel: &CancelToken,
    ) -> Result<Self, CalibrationError>;
    pub fn read_bundle(path: impl AsRef<Path>, cancel: &CancelToken) -> Result<Self, BundleError>;
    pub fn calibrate(&self, image: &mut CfaImage) -> Result<CalibrationOutcome, CalibrationError>;
}

// calibration/master_cache.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MasterKey([u8; 32]);
#[derive(Debug, Clone, Copy)]
pub struct MasterSlot<'a> { pub role: MasterRole, pub frames: &'a [&'a Path], pub key: MasterKey }
pub trait MasterCache: Sync {
    fn load(&self, slot: &MasterSlot<'_>) -> Option<CfaImage>;
    fn store(&self, slot: &MasterSlot<'_>, master: &CfaImage);
}

#[derive(Debug)]
pub(crate) struct MasterParts { pub(crate) additive: Additive, pub(crate) flat: Option<PreparedFlat>, pub(crate) defect_map: Option<DefectMap> }
impl CalibrationMasters { pub(crate) fn assemble(parts: MasterParts) -> Result<Self, CalibrationError>; }

// calibration/cosmic_ray/mod.rs
#[derive(Debug)]
pub(crate) struct CosmicRayDetector { config: CosmicRayConfig, planes: Option<DetectorPlanes> }
impl CosmicRayDetector {
    pub(crate) fn new(config: CosmicRayConfig) -> Self;
    pub(crate) fn reject(&mut self, image: &mut CfaImage) -> Result<usize, CosmicRayError>;
}
enum DetectorPlanes { Mono(MonoScratch), Bayer(BayerScratch) }

// image/metadata/dark_comparison.rs, unverified_conditions.rs
pub struct DarkComparison { /* … */ pub settings_compared: bool }
pub struct UnverifiedConditions { pub exposure: bool, pub temperature: bool, pub settings: bool }

// calibration/calibration_outcome.rs
pub struct CalibrationOutcome { pub dark: DarkOutcome, pub bias_settings_compared: Option<bool> }
pub enum DarkOutcome {
    NoDark,
    AsIs(DarkComparison),
    Scaled { comparison: DarkComparison, scale: DarkScale },
}
// calibration/calibration_set.rs
impl<T> CalibrationSet<T> {
    pub fn map<U>(self, f: impl FnMut(MasterRole, T) -> U) -> CalibrationSet<U>;
}
```

- `SensorSetting`: what a bias and a dark must share with the light. `CaptureConditions::shared`
  fills a master's header from the set, so a master states its frames' settings, not its first
  frame's.
- `Additive`: which additive masters a light loses. A bias beside a dark that still holds it is
  not a case; `dark_holds_bias` and `used_bias` go.
- `MasterKey`: lumos composes it from the recipe, each frame's name and identity, and the keys of
  the masters subtracted, before the stack reads a frame.
- `MasterCache`: a miss or an unreadable entry is `None`; a store that cannot keep warns in the
  implementor. `MasterBuild`, `MasterInputs`, `SubtractedFrames`, `CachedMaster`, `NoMasterCache`,
  `MasterBuildError::with_cache_error`, `MastersBuilder` and its four setters go; the bundle reader
  calls `assemble`, which holds the two rules a set has: one sensor, and a defect map only with
  the master it was detected on.
- `CosmicRayDetector`: one worker's detector, its working planes built for the first light and
  kept for the next. `cosmic_ray::reject` and the detector built a light go.
- `settings_compared` is true when both sides state every setting either states;
  `bias_settings_compared` is `None` without a bias.
- `DefectMap` gains `cfa_type: CfaType` and its `lattice: CfaLattice`; `Sensor::cfa_type` stops
  being an `Option`, and the two `expect`/`unreachable!` on it go.
- `CfaLattice::median` returns `Option<f32>`; `None` leaves the pixel `DEFECT` without `REPAIRED`.

### Star detection — `lumos/src/star_detection/`

```rust
// star_detection_config/matched_filter.rs  (replaces fwhm_config.rs)
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MatchedFilter {
    Off,
    Fixed(Psf),
    Estimated { seed: Psf, min_stars: usize, sigma_factor: f32 },
}
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Psf { pub fwhm_px: f32, pub axis_ratio: f32, pub angle_rad: f32 }

// extraction_config.rs
pub struct ExtractionConfig {
    pub sigma_threshold: f32, pub connectivity: Connectivity, pub deblend: Deblend,
    pub deblend_min_separation: usize,
    pub min_area: usize, pub max_area: Option<usize>, pub edge_margin: usize,
}
// bg_config.rs
pub struct BgConfig {
    pub tile_size: usize, pub sigma_clip_max_iterations: usize,
    pub refinement: Option<IterativeRefinement>,
}

// star_detector/detection_result.rs
pub struct DetectionResult { pub stars: Vec<Star>, pub diagnostics: DetectionDiagnostics }
impl DetectionResult { pub fn median_fwhm(&self) -> Option<f32>; }   // unsaturated stars
pub struct DetectionDiagnostics {
    pub extraction: ExtractionCounts,
    pub measured: usize,
    pub filter: QualityFilterDiagnostics,
    pub matched_filter: FwhmSource,
}
pub struct ExtractionCounts {
    pub pixels_above_threshold: usize, pub connected_components: usize,
    pub candidates: usize, pub deblended_components: usize,
}

// bg/bg_estimate.rs, bg/noise_map.rs (was sky_noise.rs)
#[derive(Debug)]
pub(crate) struct BgEstimate { pub(crate) level: Pooled<MeshSurface>, pub(crate) noise: NoiseMap }
#[derive(Debug)]
pub(crate) struct NoiseMap { pub(crate) sigma: Pooled<MeshSurface>, pub(crate) floor: f32 }
```

- `StarDetectionConfig { bg, extraction, matched_filter, measurement, filter }`.
  `BgRefinement`, `FwhmConfig`, `FwhmMode`, `DetectionDiagnostics::{median_fwhm, median_snr}` and
  `QualityFilterDiagnostics::saturated` go.
- `Star::saturated`: any saturated pixel within the core of the star's centre.
- `BgEstimate` and `NoiseMap` (was `SkyNoise`): one word for each of the sky level and its
  noise, where `bg`, `sky` and `noise` each named two things. Once the frame planes are owned
  (Scratch, below) the two surfaces are `FramePlanes::level` and `sigma`, and these hold borrows.
- `StarDetector::new`/`checked`/`detect` are unchanged.

### Registration — `lumos/src/registration/`

```rust
// transform_model.rs
pub enum ModelChoice<M> { Fixed(M), Auto }
pub enum TransformModel {
    Plain(ModelChoice<TransformType>),
    WithSip(ModelChoice<LinearModel>, SipConfig),
}

// registrar.rs
#[derive(Debug, Clone)]
pub struct Registrar { config: RegistrationConfig }
impl Registrar {
    pub fn checked(config: RegistrationConfig) -> Result<Self, InvalidConfigField>;
    pub fn new(config: RegistrationConfig) -> Self;
    pub fn reference(&self, stars: &[Star]) -> Result<ReferenceCatalog<'_>, RegistrationError>;
    pub fn register(&self, reference: &[Star], target: &[Star])
        -> Result<RegistrationResult, RegistrationError>;   // sugar: reference(..)?.register(..)
}
#[derive(Debug)]
pub struct ReferenceCatalog<'a> { registrar: &'a Registrar, stars: Vec<Star>, brightest: KdTree, origin: DVec2 }
impl ReferenceCatalog<'_> {
    pub fn register(&self, target: &[Star]) -> Result<RegistrationResult, RegistrationError>;
}

// warp_transform/mod.rs
#[derive(Debug, Clone)]
pub struct WarpTransform { linear: Transform, sip: Option<SipPolynomial> }
impl From<Transform> for WarpTransform;
impl WarpTransform {
    pub const fn linear(&self) -> Transform;
    pub const fn sip(&self) -> Option<&SipPolynomial>;
    pub fn apply(&self, p: DVec2) -> DVec2;
    pub fn inverse(&self) -> InverseWarp;
}

// registration_result/mod.rs
pub struct RegistrationResult { warp: WarpTransform, matches: Vec<StarMatch>, rms_error_px: f64 }
impl RegistrationResult {
    pub const fn warp(&self) -> &WarpTransform;
    pub fn into_warp(self) -> WarpTransform;
    pub fn matches(&self) -> &[StarMatch];
    pub const fn rms_error_px(&self) -> f64;
}

// resample/warp_config.rs
#[derive(Debug, Clone, Copy)]
pub struct WarpConfig { pub method: InterpolationMethod, pub clamping_threshold: Option<ClampingThreshold> }

// registration_config/mod.rs, ransac/ransac_config.rs
pub struct RegistrationMatchingConfig { pub max_stars: usize, pub min_matches: usize, pub triangle: TriangleConfig }
pub const MAX_STARS: usize = 2_000;        // the dense vote matrix: 16 MB at the bound
pub struct RansacConfig { /* … */ pub local_optimization: Option<NonZeroUsize>, pub max_rotation_rad: Option<f64>, /* … */ }
pub struct RegistrationConfig { pub model: TransformModel, pub matching: RegistrationMatchingConfig, pub ransac: RansacConfig, pub max_rms_error_px: f64 }
impl ScaleRange { pub fn checked(min: f64, max: f64) -> Result<Self, InvalidConfigField>; pub fn new(min: f64, max: f64) -> Self; }
```

- `TransformModel`: homography with SIP is not a case, by `LinearModel`. The internal `FitModel`
  is this with the choice resolved.
- `ReferenceCatalog`: the reference checked against the star gates, its brightest stars as the
  tree triangle matching reads, and the origin a SIP correction is taken about, made once. The
  pair's noise scale stays the pair's: `PreparedPair::new(&ReferenceCatalog, target)` pools the
  two catalogs' FWHM as today. A reference no light can register to fails once, as the run's error, not once per light as
  `AllFramesDropped`.
- `RegistrationResult` is built once, by the final fit, which holds `{ result, scale }`.
  `transform()`, `sip()`, `inlier_count()`, `quality_score()`, `WarpTransform::{new, with_sip,
  has_sip}`, `RegistrationConfig::registrar`, `min_stars`, `WarpConfig::{border_value, validate}`
  and `AlignStackConfigError::Warp` go. A pixel a warp has no sample for reads 0 and its coverage
  says so.
- `RegistrationConfig::mosaic` is renamed `wide_scale`.

### Decode and image — `lumos/src/io/`, `lumos/src/ingest/`, `lumos/src/image/`

```rust
// io/decode_config.rs
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct DecodeConfig { pub fits: FitsLoadConfig, pub nulls: NullPolicy, pub xtrans_passes: MarkesteijnPasses }
impl DecodeConfig { pub(crate) fn digest(&self) -> u64; }      // serializes self: no field left out

// io/load_ctx.rs
pub struct LoadCtx { pub decode: DecodeConfig, pub cancel: CancelToken, pub(crate) decode_threads: NonZeroUsize }
// ingest/ingest_config.rs
pub struct IngestConfig { pub cache_dir: PathBuf, pub keep_cache: bool, pub memory_override: Option<u64>, pub decode: DecodeConfig }
// ingest/ingest_run.rs
pub(crate) struct IngestRun<'a> { pub(crate) config: &'a IngestConfig, pub(crate) budget: Budget, pub(crate) ctx: LoadCtx, pub(crate) progress: ProgressCallback }

// io/fits/fits_load_config.rs
pub enum FitsDataMax { DataMaximum, SaturationLevel }           // FITS 4.0 by default
pub enum FitsFloatScale { Auto, Normalized, FullScale(f64) }
pub struct FitsLoadConfig { /* … */ pub data_max: FitsDataMax }

// image/metadata/derived_facts.rs
pub enum Saturation { Flagged, Level(f32) }                    // Level: normalized, stated by the decoder

// io/error.rs
#[derive(Debug, thiserror::Error)]
#[error("failed to load '{}'", path.display())]
pub struct ImageError { pub path: PathBuf, #[source] pub kind: LoadError }
pub enum LoadError { Cancelled, UnsupportedFormat, Io(io::Error), Fits(FitsLoadError), Raster(RasterLoadError), Raw(RawError) }

// image/frame.rs
#[derive(Debug, Clone)]
pub struct Frame<S> { pub metadata: ImageMetadata, samples: S, flags: Option<PixelFlags>, flat_gain: Option<FrameGain> }
pub type LinearImage = Frame<PlanarImage>;
pub type CfaImage = Frame<MosaicPlane>;
pub struct MosaicPlane { data: Buffer2<f32>, cfa_type: CfaType }
pub struct FrameParts<S> { pub samples: S, pub metadata: ImageMetadata, pub flags: Option<PixelFlags>, pub flat_gain: Option<FrameGain> }
pub trait Samples: sealed::Planes {}                     // PlanarImage and MosaicPlane alone
mod sealed {
    pub trait Planes: Send + Sync + fmt::Debug + Sized {
        type Pattern: Copy + fmt::Debug;                  // () for planes, CfaType for a mosaic
        fn desc(&self) -> PlanarDesc;
        fn channel(&self, channel: usize) -> &Buffer2<f32>;
        fn pattern(&self) -> Self::Pattern;
        fn cfa_type(&self) -> Option<CfaType>;
        fn into_planes(self) -> ArrayVec<Buffer2<f32>, 3>;
        fn from_planes(planes: ArrayVec<Buffer2<f32>, 3>, pattern: Self::Pattern) -> Self;
    }
}
impl<S: Samples> Frame<S> {
    pub fn size(&self) -> USize2;
    pub const fn flags(&self) -> Option<&PixelFlags>;
    pub fn set_flags(&mut self, flags: Option<PixelFlags>);          // release assert on the size
    pub const fn flat_gain(&self) -> Option<&FrameGain>;
    pub fn set_flat_gain(&mut self, gain: Option<FrameGain>);
    pub fn ram_bytes(&self) -> usize;
}
impl<S: Samples> From<Frame<S>> for FrameParts<S>;
impl<S: Samples> From<FrameParts<S>> for Frame<S>;
```

- `DecodeConfig`: the one statement of how a file decodes. `DecodeParams`,
  `IngestConfig::decode_params`, `StorageParams` and the hand-listed tuple in `CacheKey` go.
- `Saturation` replaces `ObservationHeader::data_max`, `saturation_level()` and
  `DerivedFacts::saturation_flagged`: one state, and the 0.95-of-full-scale guess is the decoder's
  statement, not each consumer's.
- `Frame<S>`: an image and its three companions, once. `LinearParts`, `ImageParts`, the duplicated
  accessors and `StackableImage` go: code over either image takes `Frame<S: Samples>`, and
  `from_planes` takes the pattern its samples have, where `StackableImage::from_parts` took an
  `Option<CfaType>` one impl unwrapped and the other ignored. `StoredImage::into_image::<S>` takes
  `S::Pattern`. `FrameInfo::cfa_type` becomes `Option<CfaType>`, as every other record spells "not
  a sensor frame".
- A written file states a saturation level as `SATURATE` and no `DATAMAX`.
- `LUMROWO` and `read_declared_row_order` go.

### One run — `lumos/src/memory/`, `lumos/src/frame_store/`, `lumos/src/ingest/`

```rust
// memory/run_shape.rs
#[derive(Debug, Clone, Copy)]
pub(crate) struct RunShape<const N: usize> {
    pub(crate) frame_count: usize,
    pub(crate) stages: [Stage; N],
    pub(crate) writes: RunWrites,
    pub(crate) beside_bytes: usize,
}
impl<const N: usize> RunShape<N> {
    pub(crate) fn tier(&self, budget: Budget, storage: RunStorage<'_>) -> Result<PlannedTier, FrameStoreError>;
}
#[derive(Debug)]
pub(crate) struct PlannedTier { pub(crate) tier: StorageTier, pub(crate) plan: RunPlan }
impl PlannedTier { pub(crate) fn report(&self, frame_count: usize, parked_lights: u64) -> MemoryReport; }

// memory/stage.rs, frame_store/run_storage.rs
pub(crate) struct Stage { pub(crate) kept_bytes: usize, pub(crate) held_bytes: usize, pub(crate) in_flight_bytes: usize, pub(crate) units: Units }
pub(crate) struct RunWrites { pub(crate) kept_bytes: u64, pub(crate) spilled_bytes: u64 }

// ingest/frame_origin.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FrameOrigin { Files, Held, Stored }

// frame_store/file_set.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FilesPeek { pub(crate) idx: usize, pub(crate) info: FrameInfo, pub(crate) uncached_count: usize, pub(crate) held_bytes: usize, pub(crate) gains_bytes: usize }
impl FilesPeek { pub(crate) const fn kept_write_bytes(&self) -> u64; }   // uncached × (frame + gain)
#[derive(Debug)]
pub(crate) struct Loaded<I> { pub(crate) image: I, pub(crate) cached: Option<Cached> }
#[derive(Debug)]
pub(crate) struct Cached { pub(crate) stored: StoredImage, pub(crate) admitted: Admitted }
impl<'a, P: AsRef<Path>, D: Decoder> FileSet<'a, P, D> {
    pub(crate) fn new(paths: &'a [P], config: &IngestConfig, ctx: &LoadCtx) -> Result<Self, FrameStoreError>;
    pub(crate) fn peek(&self, ctx: &LoadCtx) -> Result<FilesPeek, LoadError>;
    pub(crate) fn decode(&self, idx: usize, ctx: &LoadCtx) -> Result<Loaded<D::Image>, LoadError>;
}

// combine/frame_cache/core.rs
pub(crate) struct CacheCore { pub(crate) tier: StorageTier, pub(crate) desc: PlanarDesc, pub(crate) metadata: ImageMetadata, pub(crate) monitor: ProgressMonitor, pub(crate) memory: MemoryReport }
```

- `RunShape::tier` is the one place a plan becomes a tier: `RunPlan::of`, the disk check over
  `writes`, and the bytes held beside the combine. `LightShape`, the loader's `stages()` and the
  drizzle each only fill one. `holding_beside` stops being a step a caller remembers.
- `FilesPeek::kept_write_bytes` is the one statement of what a file set writes to the decode
  cache: B2 and B3 are a second and third statement of it.
- `PlannedTier::report` makes the run's `MemoryReport`; `StorageTier` stops carrying
  `planned_peak_bytes` and `prior_overcommit`.
- `FrameAdmission::first_facts` goes: `check_facts` reads `facts[0]`.
- `FrameOrigin` is the one word for where a set's frames come from, in logs and plans.
  `LightSource::{Held, Files}` keeps its data under the same words; `CombineSource` and
  `DrizzleSource` go.
- `FrameStats::spread` is always measured; `Spreads` and `measuring_spread` go.

### The registered run — `lumos/src/pipeline/`, `lumos/src/drizzle/`

```rust
// pipeline/align_stack_config.rs
#[derive(Debug, Clone)]
pub struct AlignStackConfig {
    pub detection: StarDetectionConfig,
    pub registration: RegistrationConfig,
    pub resample: Resample,
    pub stack: StackConfig,
    pub reference: Reference,
}
#[derive(Debug, Clone, Copy)]
pub enum Resample { Warp(WarpConfig), Drizzle(DrizzleConfig) }
#[derive(Debug)]
pub struct CalibratedLights<'a, P> {
    pub paths: &'a [P],
    pub masters: &'a CalibrationMasters,
    pub cosmic_ray: Option<&'a CosmicRayConfig>,
}
impl AlignStackConfig {
    pub fn align_stack_images(&self, lights: Vec<LinearImage>, ingest: &IngestConfig, monitor: ProgressMonitor) -> Result<AlignStackResult, AlignStackError>;
    pub fn align_stack_paths<P: AsRef<Path>>(&self, paths: &[P], ingest: &IngestConfig, monitor: ProgressMonitor) -> Result<AlignStackResult, AlignStackError>;
    pub fn calibrate_align_stack_paths<P: AsRef<Path>>(&self, lights: CalibratedLights<'_, P>, ingest: &IngestConfig, monitor: ProgressMonitor) -> Result<AlignStackResult, AlignStackError>;
}

// pipeline/align_stack_result.rs
#[derive(Debug)]
pub struct AlignStackResult { pub product: StackProduct, lights: Vec<LightOutcome> }
impl AlignStackResult {
    pub fn lights(&self) -> &[LightOutcome];
    pub fn reference(&self) -> usize;                       // the light whose registration is Reference
    pub fn stacked_count(&self) -> usize;
    pub fn dropped(&self) -> impl Iterator<Item = usize> + '_;
}

// drizzle/drizzle_config.rs
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DrizzleConfig { pub sampling: DrizzleSampling, pub fill_value: f32, pub min_weight_fraction: f32 }
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DrizzleSampling {
    Drops { shape: DropShape, scale: DrizzleScale, pixfrac: Pixfrac },
    Point { scale: DrizzleScale },
    Lanczos,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropShape { Square, Turbo, Gaussian }
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DrizzleScale(f64);    // checked: finite, > 0
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pixfrac(f64);         // checked: in (0, 1]
impl DrizzleScale { pub fn checked(scale: f64) -> Result<Self, InvalidConfigField>; pub fn new(scale: f64) -> Self; pub const fn get(self) -> f64; }
impl Pixfrac { pub fn checked(pixfrac: f64) -> Result<Self, InvalidConfigField>; pub fn new(pixfrac: f64) -> Self; pub const fn get(self) -> f64; }
impl DrizzleConfig {
    pub(crate) fn validate(&self) -> Result<(), DrizzleConfigError>;      // fill_value, min_weight_fraction
    pub(crate) fn gate(&self, product: &mut StackProduct, depth: &[Buffer2<f32>], deposit: Deposit);
}
impl Resample {
    pub(crate) fn resampled(&self, input: PlanarDesc, deposit: Deposit) -> Result<Resampled, FrameDescError>;
}
/// A light of one descriptor once resampled: its descriptor, and how it is stored.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Resampled { pub(crate) desc: PlanarDesc, pub(crate) layout: StoredLayout }
// drizzle/drizzle_stack_config.rs
#[derive(Debug, Clone, Copy)]
pub struct DrizzleStackConfig { pub drizzle: DrizzleConfig, pub stack: StackConfig }
impl DrizzleStackConfig {
    pub fn drizzle_paths<P: AsRef<Path>>(&self, frames: Vec<DrizzleFrame<P>>, ingest: &IngestConfig, monitor: ProgressMonitor) -> Result<StackProduct, DrizzleError>;
    pub fn drizzle_cfa_paths<P: AsRef<Path>>(&self, frames: Vec<DrizzleFrame<P>>, ingest: &IngestConfig, monitor: ProgressMonitor) -> Result<StackProduct, DrizzleError>;
    pub fn drizzle_images<S: Samples>(&self, frames: Vec<DrizzleFrame<Frame<S>>>, ingest: &IngestConfig, monitor: ProgressMonitor) -> Result<StackProduct, DrizzleError>;
}
// drizzle/accumulator/mod.rs
#[derive(Debug, Clone, Copy)]
pub(crate) struct DrizzleInput<'a, S> { pub(crate) image: &'a Frame<S>, pub(crate) warp: &'a WarpTransform, pub(crate) pixel_weights: Option<&'a Buffer2<f32>> }
impl Drizzler {
    pub(crate) fn new(input: PlanarDesc, deposit: Deposit, config: &DrizzleConfig) -> Self;
    pub(crate) fn drizzle<S: Samples>(&mut self, input: DrizzleInput<'_, S>, idx: usize) -> Result<DrizzledPlanes, DrizzleError>;
    pub(crate) fn recycle(&mut self, returned: NewFrame<LinearImage>);
}

// progress/progress_monitor.rs
impl From<CancelToken> for ProgressMonitor;                 // progress reported nowhere

// pipeline, internal
#[derive(Debug)]
pub(super) enum Prepared { Linear(LinearImage), Mosaic(CfaImage) }               // prepared.rs
#[derive(Debug)]
pub(super) enum PipelineFrame { Resident(Prepared), Spilled(StoredImage) }       // pipeline_frame/mod.rs
impl PipelineFrame { pub(super) fn into_resident(self, cfa_type: Option<CfaType>) -> Prepared; }
#[derive(Debug)]
pub(super) struct MosaicLight<'a> { pub(super) cfa: CfaImage, pub(super) path: &'a Path }   // mosaic_preparation.rs
#[derive(Debug)]
pub(super) struct CalibratedLight { pub(super) image: Prepared, pub(super) outcome: CalibrationOutcome }
#[derive(Debug)]
pub(super) struct PreparedLight {                                                // light_files.rs
    pub(super) image: Prepared, pub(super) cached: Option<Cached>,
    pub(super) calibration: Option<CalibrationOutcome>,
}
#[derive(Debug)]
pub(super) enum Resampler { Warp(Option<WarpResult>), Drizzle(Drizzler) }        // resampler.rs
#[derive(Debug)]
pub(super) struct LightWorker {                                                  // light_worker.rs
    pub(super) detector: StarDetector,
    pub(super) cosmic_ray: Option<CosmicRayDetector>,
    pub(super) resampler: Resampler,
}
#[derive(Debug)]
pub(super) struct DetectedLight<I = PipelineFrame> {                             // detected_light.rs
    pub(super) image: I, pub(super) stars: Vec<Star>,
    pub(super) admitted: Admitted, pub(super) diagnostics: DetectionDiagnostics,
    pub(super) calibration: Option<CalibrationOutcome>,
}
impl<I> DetectedLight<I> { pub(super) fn try_map<J, E>(self, f: impl FnOnce(I) -> Result<J, E>) -> Result<DetectedLight<J>, E>; }
#[derive(Debug)]
pub(super) struct ParkedLight {                                                  // frame_registrar.rs
    pub(super) frame: Option<StoredFrame>, pub(super) outcome: LightOutcome,
    pub(super) calibration: Option<CalibrationOutcome>,
}
pub(super) struct FrameRegistrar<'a> { reference: usize, catalog: ReferenceCatalog<'a>, run: &'a AlignStackRun<'a>, parking: &'a FrameParking, count: usize, desc: PlanarDesc, scratch: WarpScratch }
```

- `Resample`: the one step that varies. `DrizzleKernel`, `DrizzleConfigError::InvalidLanczosSampling`,
  `DrizzleShape`, `AlignStackConfigError::CosmicRayOnLinearLights`, `Preparation`,
  `DetectedFrame`, `Parked`, `LightIntake`, `PassWorker`, `CalibrationNotes`,
  `FrameParking::parked` and `AlignStackResult::registered` go.
- `Prepared`: what a light is when it is resampled. A mosaic stays one under `Resample::Drizzle`
  when its sensor has a colour pattern; every other light is its planes. Its stars are detected
  on the planes themselves, or on a mosaic's green proxy, which lives for the detection alone.
- `AlignStackResult::lights` is private: the pipeline alone builds one, so exactly one light is
  the reference and `reference()` finds it.
- `DrizzleStackConfig`: the warps-in-hand entries. Their run stays `drizzle/drizzle_run.rs`,
  built on `RunShape`, the `Drizzler` and `DrizzleConfig::gate` the registered run uses;
  `drizzle_images` and `drizzle_cfa_images` are one generic.
- `Drizzler` keeps its `OutputSums` from frame to frame, rebuilt by `recycle` from the buffers
  the spilled tier hands back, as a `WarpResult` cycles.
- `OutputGrid::new(scale: DrizzleScale, input: USize2)` computes its size in `f64`, from the value
  the mapping uses.

### Scratch — `lumos/src/scratch_pool/`, `lumos/src/star_detection/resources.rs`

```rust
#[derive(Debug)]
pub(crate) struct ScratchPool<T: Scratch> { shelf: Mutex<Shelf<T>> }
#[derive(Debug)]
pub(crate) struct Pooled<'p, T: Scratch> { value: ManuallyDrop<T>, pool: &'p ScratchPool<T> }
#[derive(Debug)]
pub(crate) struct DetectionResources {
    planes: FramePlanes,
    pools: WorkerPools,
    measured: Vec<MeasuredStar>, values: Vec<f32>,
}
#[derive(Debug)]
struct FramePlanes {          // what one detection holds, each reshaped with the frame
    measure: Buffer2<f32>, detection: Buffer2<f32>,
    saturation: BitBuffer2, threshold: BitBuffer2, sources: BitBuffer2, no_data: BitBuffer2,
    level: MeshSurface, sigma: MeshSurface,
}
#[derive(Debug)]
struct WorkerPools { bands: ScratchPool<Buffer2<f32>>, rows: ScratchPool<RowScratch>, tiles: ScratchPool<TileScratch> }
```

`reshape(&mut self)` cannot run with a lease out: `Shelf::generation` and the stale-return branch
go, with `PEAK_FLOATS`, `PEAK_SURFACES`, `PEAK_BITMASKS` and the test that pins them to the pool.
A stage borrows the planes it reads and writes, disjoint fields of `FramePlanes`, and leases from
`WorkerPools` through `&self` on its workers.

## Ownership graph

```
caller thread                                             ownership (target)
├─ owns AlignStackConfig
│  ├─ owns StarDetectionConfig, RegistrationConfig, StackConfig (Copy), Reference
│  └─ owns Resample::{Warp(WarpConfig) | Drizzle(DrizzleConfig)}
├─ owns IngestConfig
│  └─ owns DecodeConfig
├─ owns CalibrationMasters
│  ├─ owns Additive (the bias and thermal masters)
│  ├─ owns PreparedFlat
│  │  └─ shared Arc<FlatGain>            — kept, see the table
│  └─ owns DefectMap (cfa_type, lattice, hot, cold)
└─ AlignStackRun<'a>                                      one entry call
   ├─ borrows &AlignStackConfig, &IngestConfig (for the call)
   ├─ owns IngestRun<'a> { &IngestConfig, Budget, LoadCtx, ProgressCallback }
   ├─ owns Registrar (a clone of the checked config)
   ├─ owns LightSource::{Held(Vec<LinearImage>) | Files(Box<LightFiles>)}
   │  └─ LightFiles owns FileSet { cells Mutex<…> — kept; SharedGains — kept }
   ├─ owns PlannedTier { StorageTier, RunPlan } → FrameParking owns the tier
   ├─ FrameRegistrar<'_> owns ReferenceCatalog<'_> (borrows &Registrar), WarpScratch
   ├─ owns Vec<LightWorker>, one a slot
   ├─ ── thread boundary: common::par::slots, plain threads ──
   │  ├─ moved in: a held LinearImage, or an index into the FileSet
   │  ├─ borrows &LightFiles, &FrameIntake, &FrameRegistrar, &FrameParking
   │  ├─ borrows &mut LightWorker { StarDetector, Option<CosmicRayDetector>, Resampler } (its slot)
   │  └─ moved out: ParkedLight { Option<StoredFrame>, LightOutcome, Option<CalibrationOutcome> }
   └─ RegisteredSet owns Vec<ParkedLight>
      └─ FrameCache owns Vec<StoredFrame>, CacheCore { StorageTier, MemoryReport } → Combined → AlignStackResult
```

Every shared-ownership and interior-mutability case of the current code:

| Case | Where | Verdict |
|---|---|---|
| `FrameGain::Shared(Arc<FlatGain>)`, `PreparedFlat::gain` | `image/flat_gain/frame_gain.rs:12`, `calibration/prepared_flat/mod.rs:34` | Kept. Immutable, plane-sized, held by every light one flat divided, and lights outlive any borrow of the masters. Never serialized through the pointer: a file stores it by `GainKey`. |
| `SharedGains { Mutex<Vec<Arc<FlatGain>>> }` | `image/flat_gain/shared_gains.rs:15` | Kept. A run's intern table, filled by decodes on several threads; one uncontended lock a frame. |
| `DecodeCache::gains: Mutex<Vec<GainKey>>` | `frame_store/decode_cache/mod.rs:35` | Kept. Workers race to write one gain file a run. |
| `FlatGain::key: OnceLock<GainKey>` | `image/flat_gain/mod.rs:49` | Kept. A memo of a pure function of immutable data. |
| `FileSet::first: Mutex<Option<Box<First>>>`, `KeptSet::probes: Vec<Mutex<Probed>>` | `frame_store/file_set.rs:39,57` | Kept. Take-once state a path, handed to the job that decodes it by index through `&self`; moving it into `par::slots_owned` moves the lock, not removes it. |
| `FrameAdmission::first_facts: OnceLock<SetFacts>` | `ingest/frame_admission.rs:34` | Replaced: derived from `facts[0]`. |
| `FrameAdmission::facts: Box<[OnceLock<FrameFacts>]>` | `ingest/frame_admission.rs:36` | Kept. One writer a cell, read after a failure, when the jobs' own results are gone. |
| `ScratchPool::shelf: Arc<Mutex<Shelf>>`, `Pooled { shelf: Arc, generation }` | `scratch_pool/mod.rs:83,179` | The lock is kept: workers lease through `&self`. The `Arc` and the generation are replaced by `Pooled<'p, T>`. |
| `CalibrationNotes`, six `AtomicU64` | `pipeline/calibration_notes/mod.rs:13` | Replaced: each job returns its `CalibrationOutcome`; `CalibrationReport: FromIterator`. |
| `FrameParking::parked: AtomicU64` | `pipeline/frame_parking.rs:32` | Replaced: `FrameParking::hold` returns whether it wrote, counted over the lights returned. |
| `AtomicFlagCounts` ×2, `AtomicU64` in the combine | `combine/frame_cache/mod.rs:350-352` | Replaced: each row task returns `RowTallies`, folded by `reduce`. |
| `UnionFind { parent: Vec<AtomicU32>, next_label }` | `star_detection/labeling/union_find.rs:15` | Replaced: a strip unions only its own labels; the parent table is split one `&mut [u32]` a strip, stitched after. |
| `AtomicUsize deblended_components` | `star_detection/star_detector/stages/detect/mod.rs:141` | Replaced: `.map(..).sum()`. |
| `StageCounter` atomics | `progress/stage_counter/mod.rs:20` | Kept. Workers count through `&self`; the hand-off loses no report. |
| `RunScratch::next: AtomicU64` | `frame_store/run_scratch.rs:23` | Kept. Names made through `&self` must differ. |
| `ProgressCallback(Option<Arc<dyn Fn>>)` | `progress/progress_callback/mod.rs:13` | Kept. A caller's handle, cloned across runs. |
| `static LUTS: [OnceLock<LanczosLut>; 3]`, `static RULES: OnceLock<…>` | `math/lanczos/lanczos_lut.rs:32`, `math/pixel_quadrature/mod.rs:155` | Kept. Memos of pure functions, immutable once built. |
| `UnsafeSendPtr` | `image/cfa/demosaic/tiled.rs:53`, `image/flags/pixel_flags.rs:264` | Kept. Disjoint 2-D rectangles and column strips of a row-major plane, which `chunks_mut` cannot split without cutting the parallel width to the band count. |
| `Recycled<T>` | `recycled/mod.rs` | Kept. Sound: a const assert on size and alignment, length 0. |

## Call graph

```
AlignStackConfig::calibrate_align_stack_paths(&self, CalibratedLights, &IngestConfig, ProgressMonitor)
        -> Result<AlignStackResult, AlignStackError>                                   pipeline
├─ AlignStackConfig::validate(&self) -> Result<(), AlignStackConfigError>
├─ CosmicRayConfig::validate(&self) -> Result<(), InvalidConfigField>                  for lights.cosmic_ray
├─ IngestRun::new(&IngestConfig, ProgressMonitor) -> IngestRun<'_>                     ingest
├─ Registrar::new(RegistrationConfig) -> Registrar                                     registration
├─ LightFiles::calibrated(&[&Path], MosaicPreparation, &IngestRun) -> Result<LightFiles, AlignStackError>
│  └─ FileSet::new(&[P], &IngestConfig, &LoadCtx) -> Result<FileSet, FrameStoreError>   [I/O: cache probe]
└─ AlignStackRun::stack(&self, LightSource) -> Result<AlignStackResult, AlignStackError>
   ├─ LightSource::shape(&self, &IngestRun) -> Result<LightShape, AlignStackError>
   │  └─ FileSet::peek(&self, &LoadCtx) -> Result<FilesPeek, LoadError>                [I/O: one header]
   ├─ LightShape::found_reference | named_reference(&self, &AlignStackConfig, workers) -> RunShape<N>
   │  └─ Resample::resampled(&self, PlanarDesc, Deposit) -> Result<Resampled, FrameDescError>
   ├─ RunShape::tier(&self, Budget, RunStorage) -> Result<PlannedTier, FrameStoreError>   memory  [I/O: free space]
   ├─ FrameIntake::map | map_owned(lights, &mut [LightWorker], job) -> Result<Vec<_>, _>   ingest  [threads]
   │  ├─ LightFiles::prepare(&self, idx, &mut LightWorker, &LoadCtx) -> Result<PreparedLight, AlignStackError>
   │  │  ├─ FileSet::decode(&self, idx, &LoadCtx) -> Result<Loaded<CfaImage>, LoadError>   [I/O: decode]
   │  │  └─ MosaicPreparation::prepare(&self, MosaicLight, Option<&mut CosmicRayDetector>, &LoadCtx)
   │  │         -> Result<CalibratedLight, AlignStackError>
   │  │     ├─ CalibrationMasters::calibrate(&self, &mut CfaImage) -> Result<CalibrationOutcome, CalibrationError>
   │  │     ├─ CosmicRayDetector::reject(&mut self, &mut CfaImage) -> Result<usize, CosmicRayError>
   │  │     └─ CfaImage::demosaic(self, ..) -> Result<LinearImage, Cancelled>           Resample::Warp, or a mono sensor
   │  ├─ FrameIntake::admit_measured(&self, idx, &Frame<S>, Option<Admitted>) -> Result<Admitted, AdmissionError>
   │  ├─ StarDetector::detect(&mut self, &LinearImage) -> DetectionResult               star_detection
   │  │      on the planes, or on CfaImage::green_proxy(&self) -> LinearImage
   │  └─ FrameRegistrar::park(&self, &mut Resampler, idx, DetectedLight) -> Result<ParkedLight, AlignStackError>
   │     ├─ ReferenceCatalog::register(&self, &[Star]) -> Result<RegistrationResult, RegistrationError>
   │     ├─ PipelineFrame::into_resident(self, Option<CfaType>) -> Prepared             [I/O: a parked light read back]
   │     ├─ Resampler::Warp    → WarpResult::warp_into(&mut self, &SourceImage, Warp, &WarpScratch)   registration
   │     │  Resampler::Drizzle → Drizzler::drizzle(&mut self, DrizzleInput, idx) -> Result<DrizzledPlanes, DrizzleError>
   │     └─ StorageTier::store(&self, impl FrameBuffers, Admitted, SetKind)
   │            -> Result<Stored<_>, FrameStoreError>                                   [I/O: spilled tier]
   │        └─ returned buffers → the Resampler: the next WarpResult, or Drizzler::recycle
   └─ RegisteredSet::combine(self, &AlignStackRun) -> Result<AlignStackResult, AlignStackError>
      ├─ PlannedTier::report(&self, frame_count, parked_lights) -> MemoryReport
      ├─ FrameCache::from_stored_frames(Vec<StoredFrame>, CacheCore, &StackConfig) -> Result<FrameCache, StackError>
      │  └─ FrameNorm::measure(SlotPlanes, Normalization) -> Result<Option<Vec<FrameNorm>>, StackError>   [I/O: spilled planes]
      ├─ StackConfig::combine(&self, &FrameCache) -> Result<Combined, StackError>       combine
      │  ├─ Combine::Median       → FrameCache::process_chunked(request, median reducer)
      │  ├─ Combine::Mean         → FrameCache::process_chunked(request, mean reducer)
      │  └─ Combine::RejectedMean → FrameCache::process_chunked(request, RejectedMean::reducer())
      ├─ DrizzleConfig::gate(&self, &mut StackProduct, &[Buffer2<f32>], Deposit)        Resample::Drizzle only
      └─ CalibrationReport::from_iter(outcomes).with_masters(&CalibrationMasters)

AlignStackConfig::align_stack_paths / align_stack_images       the same tree; every light Prepared::Linear,
                                                               no preparation, no calibration group

DrizzleStackConfig::drizzle_paths / drizzle_cfa_paths / drizzle_images
        -> Result<StackProduct, DrizzleError>                                           drizzle
└─ DrizzleRun::run(&self, frames) -> Result<StackProduct, DrizzleError>
   ├─ FileSet::new → FileSet::peek → DrizzleRun::shape(&self, FrameInfo, Option<&FilesPeek>) -> RunShape<3>
   │    → RunShape::tier                                                                [I/O]
   ├─ per frame, one at a time: FileSet::decode → FrameIntake::admit_measured
   │    → Drizzler::drizzle(DrizzleInput { image, warp, pixel_weights }, idx) → StorageTier::store → Drizzler::recycle
   ├─ FrameCache::from_stored_frames → StackConfig::combine -> Combined
   └─ DrizzleConfig::gate(&self, &mut StackProduct, depth, Deposit)

StackConfig::stack_paths(&self, &[P], &IngestConfig, ProgressMonitor) -> Result<StackProduct, StackError>
├─ StackConfig::validate(&self) -> Result<(), StackConfigError>
├─ FrameCache::from_paths(&[&Path], &StackConfig, IngestRun) -> Result<FrameCache, StackError>
│  ├─ FileSet::new → FileSet::peek → loader::shape(&FilesPeek, &StackConfig) -> RunShape<3> → RunShape::tier   [I/O]
│  └─ FrameIntake::map(idxs, .., decode → admit → store)                                [I/O, threads]
└─ StackConfig::combine(&self, &FrameCache) -> Result<Combined, StackError>

StackConfig::stack_images(&self, Vec<StackFrame>, ProgressMonitor) -> Result<StackProduct, StackError>
├─ StackConfig::validate
├─ FrameCache::from_stack_frames(Vec<StackFrame>, &StackConfig, ProgressMonitor)        MemoryReport::default()
└─ StackConfig::combine

CalibrationMasters::build(CalibrationSet<&[P]>, &MastersConfig, &IngestConfig, ProgressMonitor)
        -> Result<CalibrationMasters, MasterBuildError>                                calibration
└─ build_from(CalibrationSet<&[&Path]>, &MastersConfig, &IngestConfig, ProgressMonitor)   non-generic
   ├─ per role, bias → flat-dark → flat → dark:
   │  ├─ MasterKey::of(MasterRole, &[&Path], MasterRecipe, &[MasterKey]) -> MasterKey   [I/O: one stat a frame]
   │  ├─ MasterCache::load(&self, &MasterSlot) -> Option<CfaImage>                      [I/O: the caller's]
   │  ├─ MasterStack::run(self, ProgressMonitor) -> Result<CfaImage, MasterStackError>
   │  └─ MasterCache::store(&self, &MasterSlot, &CfaImage)                              [I/O: the caller's]
   └─ CalibrationMasters::from_images(CalibrationSet<Option<CfaImage>>, SigmaThreshold, &CancelToken)
      └─ CalibrationMasters::assemble(MasterParts) -> Result<CalibrationMasters, CalibrationError>

Registrar::register(&self, &[Star], &[Star]) -> Result<RegistrationResult, RegistrationError>
├─ Registrar::reference(&self, &[Star]) -> Result<ReferenceCatalog<'_>, RegistrationError>
└─ ReferenceCatalog::register(&self, &[Star])
   ├─ PreparedPair::new(&ReferenceCatalog, &[Star]) -> Result<PreparedPair, RegistrationError>
   ├─ PreparedPair::matches(&self) -> Vec<PointMatch>
   ├─ ModelSearch::fit(&self, TransformModel, DVec2) -> Result<FinalFit, RegistrationError>
   │      plain and SIP candidates, one RANSAC a TransformType, the lowest GRIC
   └─ FinalFit { result: RegistrationResult, scale } held to max_rms_error_px
```

## Module layout

```
lumos/src
├─ combine/stack_config/     combine.rs (Combine)  small_n.rs (SmallN, Fallback)  weighting.rs
│                            mod.rs (StackConfig, its entries, `combine`)          − combine_method.rs
├─ combine/stack/            mod.rs (Combined, reducer choice)  rejected_mean.rs (RejectedMean; was mean_combine.rs)
├─ combine/rejection/        trim_config.rs (TrimConfig, TrimBounds)  gesd_config.rs
├─ stack_product/            mod.rs  coverage.rs  weight_plane.rs (WeightPlane, WeightKind; was weight_kind.rs)
│                            run_report.rs (RunReport and its four groups)
├─ calibration/              calibration_masters/mod.rs (CalibrationMasters, Additive, MastersConfig, MasterParts)
│                            master_cache.rs (MasterCache, MasterSlot, MasterKey)
│                            calibration_outcome.rs (CalibrationOutcome, DarkOutcome)
│                            cosmic_ray/mod.rs (CosmicRayDetector, DetectorPlanes)
├─ frame/                    capture_conditions/mod.rs (CaptureConditions, SensorSetting)
├─ star_detection/           star_detection_config/matched_filter.rs (MatchedFilter, Psf)   − fwhm_config.rs
│                            star_detector/detection_result.rs (… ExtractionCounts)
│                            bg/noise_map.rs (NoiseMap)
│                            resources.rs (DetectionResources, FramePlanes, WorkerPools)
├─ registration/             registrar.rs (Registrar)  reference_catalog.rs (ReferenceCatalog)
│                            transform_model.rs (TransformModel, ModelChoice)
│                            resample/mod.rs (WarpResult and its `impl FrameBuffers`)
├─ io/                       decode_config.rs (DecodeConfig)  error.rs (ImageError, LoadError)
│                            − decoder/decode_params.rs
├─ image/                    frame.rs (Frame, FrameParts, Samples)  linear_image.rs (impl Frame<PlanarImage>)
│                            cfa/cfa_image.rs (MosaicPlane, impl Frame<MosaicPlane>)   − stackable_image.rs
├─ ingest/                   frame_origin.rs (FrameOrigin)
├─ memory/                   run_shape.rs (RunShape, PlannedTier)
├─ frame_store/              file_set.rs (FileSet, FilesPeek, Loaded, Cached)   − storage_params.rs
├─ drizzle/                  drizzle_config.rs (DrizzleConfig, DrizzleSampling, DropShape, DrizzleScale, Pixfrac)
│                            drizzle_stack_config.rs (DrizzleStackConfig)  drizzle_run.rs (DrizzleRun)
│                            accumulator/mod.rs (Drizzler, DrizzleInput, and OutputSums' `impl FrameBuffers`)
│                            − drizzle_shape.rs  − drizzle_kernel.rs
└─ pipeline/                 align_stack_config.rs (AlignStackConfig, Resample, CalibratedLights, Reference)
                             detected_light.rs (DetectedLight)  prepared.rs (Prepared)  resampler.rs (Resampler)
                             light_worker.rs (LightWorker)
                             − detected_frame.rs  − calibration_notes/
```

Dependencies, top-level modules, each after what it uses:

```
math, memory, scratch_pool, recycled, error, config_field, progress
image  ⇄ io                          the kept cycle
noise → image, math
frame → image, noise, memory
frame_store → frame, image, io, memory
ingest → frame, frame_store, image, io, memory, progress
bg_mesh → image, math        star_detection → bg_mesh, image, math, noise
registration → star_detection, image, math, frame_store        (frame_store → registration is cut)
stack_product → image, io
combine → frame, frame_store, ingest, image, io, math, memory, noise, registration, stack_product
calibration → combine, frame, image, ingest, io, noise, bg_mesh
drizzle → registration, combine, ingest, frame_store, image, memory
pipeline → calibration, star_detection, registration, drizzle, combine, ingest, frame_store, memory
```

`pipeline → drizzle` is new: the registered run's `Resampler::Drizzle`. No edge returns.

# Part 2 — Plan

Each batch leaves the gate green and rewrites its callers in `lens` in the same change. An item
done is deleted.

## Batch 1 — What a FITS file says

`lumos/src/io/fits/`, `lumos/src/image/metadata/`, `lumos/src/calibration/calibration_pass/`,
`lumos/src/calibration/dark_scale.rs`, `lumos/src/star_detection/star_detector/stages/prepared_frame/`.

- [ ] `io/fits/decode/pixels/mod.rs:120-131`, `io/fits/decode/plan.rs:138-151` — B1. `DATAMAX`
      under `FitsDataMax::DataMaximum` decides the float scale and flags nothing; `SATURATE` gives
      the level; `FitsDataMax::SaturationLevel` keeps today's reading. Field
      `FitsLoadConfig::data_max`.
- [ ] `image/metadata/observation_header.rs:76-86`, `image/metadata/derived_facts.rs:41-44` —
      `Saturation { Flagged, Level(f32) }` replaces `data_max`, `saturation_level()` and
      `saturation_flagged`; the three consumers (`calibration_pass:78`, `dark_scale:113`,
      `prepared_frame:305-314`) match on it. `io/fits/metadata/mod.rs:300-309` writes the level
      as `SATURATE` and no `DATAMAX`.
- [ ] `io/fits/metadata/mod.rs:326-338,404-426` — B8. `ROWORDER` written as the source's, with
      `BAYERPAT` in that convention; `LUMROWO`, `SOURCE_ROW_ORDER` and `read_declared_row_order`
      deleted.
- [ ] `io/fits/fits_load_config.rs` — `FitsFloatScale::FullScale(f64)`.
- [ ] `io/fits/decode/plan.rs:373-380`, `io/raw/unpacked_raw.rs:336-342` — P16. `preview` reads
      a three-plane cube as RGB whatever `cube` says, and clamps a mosaic's preview on one rule
      for both containers.
- [ ] Tests: `DATAMAX` equal to the frame's maximum with no pixel clipped flags nothing; a
      `SATURATE` level flags; a bottom-up source written and read back, at an even and an odd
      height and with `XBAYROFF`/`YBAYROFF` set, its `ROWORDER`, pattern and samples unchanged;
      `preview` of a third-party RGB cube under the default context.

## Batch 2 — One planner

`lumos/src/memory/`, `lumos/src/frame_store/`, `lumos/src/ingest/`,
`lumos/src/combine/frame_cache/loader/`, `lumos/src/pipeline/light_shape.rs`,
`lumos/src/pipeline/light_files.rs`, `lumos/src/drizzle/drizzle_shape.rs`.

- [ ] `memory/run_shape.rs` (new) — `RunShape<N>`, `PlannedTier`. `RunShape::tier` runs
      `RunPlan::of`, the disk check and the beside bytes; `RunStorage::tier` and
      `StorageTier::holding_beside` become its private steps.
- [ ] `stack_product/run_report.rs` — `MemoryReport` and `RunReport::memory`, the first of the
      four groups; `PlannedTier::report` makes it and `CacheCore::memory` carries it.
- [ ] `frame_store/file_set.rs:128-133,261-264` — `SetPeek` becomes `FilesPeek` with
      `kept_write_bytes()`; `gain_bytes(&self, uncached)`'s parameter is renamed `gain_bytes_each`.
- [ ] `drizzle/drizzle_shape.rs:51-77`, `drizzle/drizzle_run.rs:112-116` — B2, B3. The drizzle
      fills a `RunShape` from the whole `FilesPeek`; `DrizzleSource` deleted.
- [ ] `combine/frame_cache/loader/mod.rs:74-100`, `pipeline/light_shape.rs:83-101`,
      `pipeline/light_files.rs:94-134` — each fills a `RunShape`; the three copies of
      `uncached × (frame + gain)` deleted.
- [ ] `memory/stage.rs:12-17`, `frame_store/run_storage.rs:24-27` — `_bytes` on every byte field
      of `Stage`, `RunWrites`, `FilesPeek`, `LightShape`; `+` on byte sums after a saturating
      product becomes `saturating_add` (`drizzle_shape.rs:140`).
- [ ] `ingest/frame_origin.rs` (new) — `FrameOrigin`; `combine::stack::CombineSource` deleted,
      the combine's log line reads the origin and the decoder kind.
- [ ] `ingest/frame_admission.rs:34,97-114` — B7. `first_facts` deleted; `check_facts` reads
      `facts[0]`.
- [ ] `frame_store/storage_tier.rs:19,34`, `combine/frame_cache/mod.rs:589-603` —
      `planned_peak_bytes` and `prior_overcommit` leave the tier for the `MemoryReport` in
      `CacheCore`; the combine raises its `overcommit_bytes` by its own stage's.
- [ ] `frame_store/file_set.rs:138-144` — `Loaded { image, cached: Option<Cached> }`.
- [ ] `frame/admitted.rs:19-24`, `frame/frame_stats/mod.rs`, `ingest/frame_admission.rs:88-95` —
      the spread is always measured; `Spreads`, `measuring_spread` and the `Option` on
      `FrameStats::spread` deleted.
- [ ] `frame_store/decode_cache/mod.rs:25-31` — `DecodeCache::open` removes temporaries whose
      process is gone and files of a stem no commit names: one `read_dir` a run.
- [ ] Tests: a file drizzle of gain-carrying FITS on the resident tier with no cache; a partly
      cached TIFF set's plan charges the held frame; a load whose frame 0 is reused and whose
      frame 2 mismatches stops before frame 3 decodes; a killed run's temporary is gone after the
      next open.

## Batch 3 — The combine's types

`lumos/src/combine/`, `lumos/src/stack_product/`, `lumos/src/calibration/master_role.rs`,
`lumos/src/calibration/master_stack.rs`, `lumos/src/pipeline/registered_set.rs`,
`lumos/tests/public_api.rs`, `lens/src/astro/config/stacking.rs`, `lens/src/astro/nodes/stacking.rs`.

- [ ] `combine/stack_config/{combine.rs,combine_method.rs,small_n.rs}` — `Combine` as declared;
      `CombineMethod`, `SmallN::none`, `SmallN::falls_back`,
      `StackConfigError::RejectingSmallNFallback` deleted; `StackConfig::flat` rewritten.
- [ ] `combine/rejection/mod.rs` — `Rejection::None` and `Default` deleted with their five arms;
      `Rejection::small_n` added; `MIN_FRAMES_FOR_REJECTION` and `MIN_FRAMES_FOR_GESD` move to it.
- [ ] `combine/stack/mod.rs:60-140`, `combine/stack/mean_combine.rs:53-69` — three reducers
      chosen once; the per-pixel `falls_back`, `rejection == Rejection::None` and the private
      `Fallback` mirror deleted; `mean_combine.rs` becomes `rejected_mean.rs`.
- [ ] `combine/stack/mean_combine.rs:62-68`, `combine/reducer/combined_sample.rs:85-96` — B9. A
      pixel of one or two samples takes the weighted mean; a median fallback of three or more
      states the model variance times the median's small-sample factor.
- [ ] `combine/stack_config/weighting.rs`, `combine/stack_config/mod.rs:93-105,156-190` —
      `Weighting::Manual`, `for_survivors`, `validate`'s `frame_count`, the three manual-weight
      errors and their `renumbered` arms deleted; `StackConfig: Copy`.
- [ ] `combine/stack_config/mod.rs:325-372`, `combine/stack/mod.rs:39-55` — `CombinePlan` and
      `FrameSet` deleted; `FrameCache`'s constructors and `StackConfig::combine` take
      `&StackConfig`; `master_stack.rs:86-95` stops repeating `normalization` and `quality`.
- [ ] `combine/rejection/{sigma_clip_config,linear_fit_clip_config,winsorized_clip_config,
      trim_config,gesd_config}.rs` — the positional `new`s deleted; `TrimBounds`;
      `GesdConfig::outliers_fraction`.
- [ ] `combine/reducer/combined_sample.rs:45-65`, `combine/rejection/mod.rs:116-123` — P14. The
      inverse variance of a band method is multiplied by `SigmaBounds::truncated_variance`, the
      dispersion by its inverse squared; both docs state the clipped mean's variance and the
      figures it was checked against.
- [ ] `combine/reducer/combined_sample.rs:45-60`, `math/sum` — the three quality sums accumulate
      in `f64`; `weighted_mean_f32` returns its `WeightedSums` so `Σw` is summed once.
- [ ] `combine/normalization/mod.rs:178-196` — P15. After the fit the norms are re-expressed on
      the frame of smallest `gain·σ`.
- [ ] `combine/frame_cache/mod.rs:350-352,572-574`, `image/flags/flag_counts.rs:94` — row tasks
      return `RowTallies`, folded by `reduce`; `AtomicFlagCounts` deleted.
- [ ] `combine/frame_cache/mod.rs:469-506`, `combine/stack/mod.rs:90,122` — one frame-major
      `Vec<SlotTerms { norm, weight, noise }>` resolved once from one `NormalizedStats`; the three
      per-sample `Option` matches leave the gather.
- [ ] `stack_product/mod.rs:61-64`, `stack_product/coverage.rs`, `stack_product/fits/` —
      `WeightPlane`, `Coverage::{Full, PerPixel}`; `fill_where` turns `Full` into a plane before
      it fills (B11); `CombineOutput::coverage` says "not asked" and "full" apart; `LUMWGTK`
      written and required only with the `WEIGHT` extension, `LUMCOVU` a logical.
- [ ] `combine/frame_cache/mod.rs` (`process_chunked`), `combine/stack/mod.rs` — `Combined`:
      the combine sums each drizzled frame's drop weight into `depth` as it gathers.
- [ ] `stack_product/run_report.rs`, `stack_product/fits/report_keywords.rs`,
      `pipeline/calibration_notes/mod.rs:59`, `drizzle/drizzle_run.rs:314` — the three groups
      beside `memory`; `CalibrationNotes::report(&self, masters) -> CalibrationReport` and the
      drizzle return theirs whole; one keyword table a group, an optional group present when all
      its keywords are; `PRODUCT_VERSION` raised.
- [ ] `frame_store/stored_frame.rs` — `StoredFrame { admitted: Admitted, .. }` in place of the
      unpacked `source_stats` and `facts`.
- [ ] Tests: a Monte Carlo that holds both variance planes to the realized variance of a clipped
      mean at k 2.5 and 3; the inverse variance of a one- and a two-sample pixel against the
      mean's; `outliers_fraction` shown to matter by an outcome; the run report's tallies equal
      between tiers.

## Batch 4 — Calibration

`lumos/src/calibration/`, `lumos/src/frame/capture_condition*.rs`, `lumos/src/noise/mrs_noise.rs`,
`lumos/src/image/cfa/cfa_lattice.rs`, `lens/src/astro/nodes/calibration/`.

- [ ] `frame/capture_conditions/mod.rs:13-21`, `frame/capture_condition.rs` — P1. `SensorSetting`;
      `CaptureCondition::{Gain, Iso, Offset, Binning}` agree by equality.
- [ ] `calibration/dark_match/mod.rs:48-103`, `calibration/calibration_masters/mod.rs:550-584` —
      a stated setting that differs refuses a bias or a dark (`DarkMismatch::Setting`); binning
      refuses a flat; an unstated side clears `DarkComparison::settings_compared` or
      `CalibrationOutcome::bias_settings_compared`; a filter that differs between flat and light
      is counted. `CalibrationReport` gains `unverified_settings` and `filter_differences`.
- [ ] `calibration/master_stack.rs:95-108`, `combine/frame_cache/mod.rs:270-276` — every role's
      frames share their settings; the master's header takes them from
      `CaptureConditions::shared`.
- [ ] `calibration/dark_scale.rs:17,85-103` — P2. The fit is taken when its slope is positive
      and `standard_error / slope < u`, with `u = 0.12 · max(|ΔT|, TEMPERATURE_TOLERANCE)` where
      a temperature compared and `u = 1/3` where none did; the exposure ratio otherwise.
      `MIN_FIT_PIXELS` deleted as the gate.
- [ ] `calibration/cosmic_ray/noise_model/mod.rs:58-66` — P3. A `Pedestal::Kept` level leaves
      the sky; an unknown pedestal with no bias removed refuses `NoiseEstimation::Measured`
      without a gain.
- [ ] `calibration/defect_map/mod.rs:354-360` — P4. `MAX_HOT_SHARE = 0.01` of a colour's
      photosites, the cut raised to the quantile that holds it; `DefectSummary` says when it was.
- [ ] `image/cfa/cfa_lattice.rs:151-153`, `calibration/defect_map/mod.rs:284-288` — B5.
      `median` returns `Option<f32>`.
- [ ] `calibration/calibration_masters/mod.rs:344-386` — B6. `from_images` returns
      `FlatDarksWithoutFlats`; `build` keeps its early count check alone.
- [ ] `calibration/calibration_masters/mod.rs:63-195`, `calibration/master_cache.rs`,
      `calibration/error.rs:222-252` — B4. `MastersConfig`, `MasterCache` as a `&dyn`,
      `MasterKey` composed by lumos, `MasterBuildError` without `E`; the generic shell calls a
      non-generic `build_from`.
- [ ] `calibration/calibration_masters/mod.rs:45-57,270-280,589-657` — `Additive`;
      `MastersBuilder` replaced by `assemble(MasterParts)`, which holds the map-with-its-master
      rule the bundle reader checks apart today.
- [ ] `calibration/calibration_outcome.rs` — `DarkOutcome`.
- [ ] `calibration/defect_map/mod.rs`, `calibration/sensor.rs:19,66-68` — `DefectMap::cfa_type`
      and its lattice; `CfaLattice::new` runs once a map, not twice a light.
- [ ] `calibration/calibration_masters/mod.rs:87` — `read_bundle(path, &CancelToken)`.
- [ ] `calibration/calibration_set.rs` — `CalibrationSet::map`.
- [ ] `noise/mrs_noise.rs:22,439-443` — P17. `LAYER_SIGMA` derived from
      `wavelet::white_noise_sigma`; `WHITE_NOISE_RESPONSE` measured again.
- [ ] `calibration/cosmic_ray/mod.rs:90-123`, `calibration/cosmic_ray/mono.rs:139` —
      `CosmicRayDetector`, kept by a worker from one light to the next; the
      `&dyn Fn(usize) -> PixelBg` a pixel becomes an enum of the two layouts;
      `image.flat_gain().cloned()` becomes a split borrow. `pipeline/align_stack_run.rs` gives
      each worker slot one.
- [ ] Tests: a dark of another ISO refused, an unstated one counted; a fit of a hundred weak
      pairs refused; a defect with a fully masked stencil; the measured model on a light with a
      kept pedestal; a failing cache store leaves the build's result; the bundle's tests move
      beside `bundle/`; `sigma_threshold_affects_detection` and
      `hot_detection_sigma_threshold_picks_its_tier` merge.

## Batch 5 — Star detection

`lumos/src/star_detection/`, `lumos/src/bg_mesh/`, `lens/src/astro/config/stacking.rs`.

- [ ] `star_detector/stages/detect/mod.rs:201`, `star_detection_config/extraction_config.rs:66,81`,
      `star_detection_config/mod.rs:106` — P5. `max_area: Option<usize>`, `None` in the default
      and the presets.
- [ ] `star_detector/stages/filter/mod.rs:35`, `centroid/mod.rs:218` — P6.
      `FilterRejection::Saturated` deleted; `saturated` set by `BitBuffer2::any_in_square` over
      the core; the FWHM estimate and `median_fwhm()` skip saturated stars.
- [ ] `star_detection_config/fwhm_config.rs` — `MatchedFilter`, `Psf`; unit suffixes on the
      FWHM and the angle.
- [ ] `star_detection_config/bg_config.rs` — `refinement: Option<IterativeRefinement>`.
- [ ] `star_detection_config/mod.rs:65-125` — the presets' docs say which sampling each is for:
      a long focal length spreads a star over more pixels, not fewer.
- [ ] `star_detector/detection_result.rs`, `star_detector/mod.rs:86-122` — `ExtractionCounts`
      returned by `Candidates` as it stands; `median_fwhm`, `median_snr` deleted;
      `DetectionResult::median_fwhm()`.
- [ ] `bg_mesh/tile_stats/mod.rs:16` — P8. Every pixel of a tile; `MAX_TILE_SAMPLES` and
      `sample_ordinal` deleted.
- [ ] `bg_mesh/tile_stats/mod.rs:224`, `bg_mesh/mod.rs:161-200`, `bg_mesh/color_mesh/mod.rs:56` —
      the mesh is told which statistics to produce, so a σ-only pass fits no plane;
      `fill_bad_tiles`'s vectors live on `MeshWorkspace`; `ColorMesh::measure` takes a workspace.
- [ ] `star_detector/stages/prepared_frame/mod.rs:242-257` — P9. The channel weights read
      `mosaic_noise()` where the metadata states it.
- [ ] `star_detection/detection_filter/mod.rs:294` — the unfiltered first pass borrows the
      measurement plane.
- [ ] `centroid/measure_grid.rs:15,29`, `centroid/stamp_metrics.rs:85` — P10. Flux and SNR over
      a circle of radius `APERTURE_FWHM · fwhm_px`, unrounded, each pixel weighed by its exact
      area inside it and `npix` their sum; `Star::flux` and `Star::snr` name the aperture;
      `MAX_STAMP_RADIUS` follows `MAX_FWHM`. Test: a star's SNR at FWHM 3.99 and 4.01 differs by
      under 1 %.
- [ ] `bg/bg_estimate.rs`, `bg/sky_noise.rs:65`, `refined_bg.rs:57` — `BgEstimate { level,
      noise: NoiseMap }`; the two tile spreads named apart.
- [ ] `labeling/union_find.rs:15`, `labeling/labeler.rs` — a plain union-find a strip over a
      split parent table; `stages/detect/mod.rs:141` sums its count.
- [ ] `labeling/` — `ComponentData::label` deleted (its index plus one); one run type for
      `Run`, `(u32, Run)` and `LabelRun`.
- [ ] `threshold_mask/threshold_params.rs:68` — `trait NoiseRows`, with one production impl,
      becomes that type.
- [ ] `bit_buffer2/mod.rs` — one constructor of a cleared buffer, one bit read.
- [ ] Tests: a Moffat β 2.5 star of peak SNR 5000 at FWHM 4 is detected; a saturated pixel
      beside the peak sets `saturated`; each preset parameter shown to change an outcome; a frame
      smaller than a tile.

## Batch 6 — Registration

`lumos/src/registration/`, `lumos/src/pipeline/frame_registrar.rs`,
`lumos/src/pipeline/align_stack_config.rs`, `lens/src/astro/config/stacking.rs`.

- [ ] Test first: catalogs with a 0.03 px floor the formal σ does not explain; the share of
      bright stars the final fit keeps decides P12.
- [ ] `final_fit/fit_catalogs.rs:98-127,188` — P12. `σ_excess²` added to a pair's variance,
      estimated a fit; `FinalFit::scale` reports it.
- [ ] `model_search.rs:95-101`, `final_fit/mod.rs:49-55` — P11. Under `WithSip(Auto, _)` the
      candidates are the plain models and the SIP ones; RANSAC runs once a `TransformType` and
      serves both; a SIP candidate short of pairs is one failed candidate.
      `every_model_failing_reports_every_reason` is rewritten for it.
- [ ] `distortion/sip/mod.rs:315-408` — P13. `SipSystem::partial_out`, a closed-form linear part
      a `LinearModel`, one solve for the terms; `fit_rotation`, `MAX_SECANT_STEPS`, the `1e-6`
      and `FixedLinearFit::angle_derivative` deleted.
- [ ] `transform_model.rs:13-54` — `ModelChoice`, `TransformModel::{Plain, WithSip}`;
      `most_general` and `sip` in two arms each.
- [ ] `registrar.rs:12-21`, `registration_config/mod.rs:159-162`, `register.rs:41-95` —
      `Registrar` owns its config, `checked` and `new`; `reference` and `ReferenceCatalog`;
      `RegistrationConfig::registrar` deleted. The pipeline takes the catalog once and fails the
      run on its error.
- [ ] `warp_transform/mod.rs:20-40,69`, `registration_result/mod.rs:24-86`,
      `final_fit/mod.rs:60-69` — private fields and `From<Transform>`; the result built once;
      `quality_score` and its two constants deleted; `rms_error` computed once.
- [ ] `resample/warp_config.rs:78,135-137`, `resample/mod.rs:139-143` —
      `WarpConfig::border_value`, `validate`, the release assert and
      `AlignStackConfigError::Warp` deleted.
- [ ] `registration_config/mod.rs:16-17,48-49,108-109`, `ransac/ransac_config.rs:18-23` —
      `MAX_STARS`; `max_rotation_rad`, `max_rms_error_px`; `local_optimization:
      Option<NonZeroUsize>`; `min_stars` deleted; `ScaleRange::checked`.
- [ ] `registration_config/mod.rs:243-252` — `mosaic` renamed `wide_scale`, its doc saying what
      it changes.
- [ ] `ransac/sampling/mod.rs:79,94-97` — a guided phase runs no more iterations than its pool
      holds distinct samples.
- [ ] `ransac/mod.rs:57-62,174-181` — `RansacResult::inliers` becomes a count; the scratch is
      swapped, not taken, so the loop allocates nothing after its first improvement.
- [ ] `distortion/sip/mod.rs:5-6`, `distortion/mod.rs:16-17`, `triangle/triangle_config.rs:28-29`,
      `resample/warp_config.rs` — docs: the coefficients are normalized about `sip.origin`; which
      tree match recovery queries; the warp preserves surface brightness.
- [ ] `frame_store/frame_buffers.rs:49-78` — `impl FrameBuffers for WarpResult` moves beside
      `WarpResult`; `frame_store` stops naming `registration`.
- [ ] Tests: two seeds draw differently; a fit that reaches `MAX_PASSES`; `max_stars` at the
      bound; a target of another size through `WarpConfig::warp`.

## Batch 7 — Decode policy and the image types

`lumos/src/io/`, `lumos/src/ingest/`, `lumos/src/image/`, `lumos/src/frame_store/`,
`lumos/src/frame/frame_info/`.

- [ ] `io/load_ctx.rs:25-40`, `ingest/ingest_config.rs:36-66`, `io/decoder/decode_params.rs`,
      `frame_store/decode_cache/cache_key.rs:33-37` — `DecodeConfig`; the cache key hashes
      `DecodeConfig::digest()`; `decode_threads: NonZeroUsize`, crate-visible.
- [ ] `ingest/ingest_run.rs:40-46`, `frame_store/storage_params.rs` — `IngestRun<'a>` borrows
      the config; `StorageParams` deleted.
- [ ] `io/error.rs:16-154` — `ImageError { path, kind }`; the three hand-flattened mappings
      deleted; every cause in `source`.
- [ ] `image/frame.rs` (new), `image/linear_image.rs:23-33,154-185,303-353`,
      `image/cfa/cfa_image.rs:36-48,114,201-229`, `image/stackable_image.rs:22-52` — `Frame<S>`,
      `FrameParts<S>`, `Samples`; `StackableImage` deleted, its users bound on `S: Samples`;
      `set_flags` asserts in release for both; `CfaImage`'s setters and `demosaic` public as
      `LinearImage`'s are.
- [ ] `io/fits/linear.rs:72-85`, `io/fits/cfa/mod.rs:69-88` — one writer tail for the flags
      extension, the gain extension and the checksum.
- [ ] `image/intensity.rs:22-33` — one of `Channels` and `IntensityBand`.
- [ ] `frame/frame_info/mod.rs:16-18,57` — `cfa_type: Option<CfaType>`.
- [ ] `image/flat_gain/gain_bins.rs` — `GainBins::of` selects its seven quantiles instead of
      sorting a copy of each node plane.
- [ ] Tests: a decode option added to `DecodeConfig` changes the cache key by construction (a
      compile-time property; one test that two configs differing in each field digest apart).

## Batch 8 — One registered run

`lumos/src/pipeline/`, `lumos/src/drizzle/`, `lumos/src/progress/progress_monitor.rs`,
`lumos/tests/`, `lumos/examples/full_pipeline.rs`, `lens/src/astro/nodes/stacking.rs`. Needs
batches 2, 3, 4, 6 and 7.

- [ ] `drizzle/drizzle_config.rs:24-79`, `drizzle/drizzle_kernel.rs`,
      `drizzle/accumulator/kernel_plan.rs:58` — `DrizzleSampling`, `DropShape`, `DrizzleScale`,
      `Pixfrac`; `KernelPlan::new` a total match; `InvalidLanczosSampling` and the two range
      checks deleted.
- [ ] `drizzle/output_grid.rs:35` — B10. The size in `f64`, from the `DrizzleScale` the mapping
      uses.
- [ ] `pipeline/align_stack_config.rs:118-140` — `Resample`; `AlignStackConfig::warp` becomes
      `resample`.
- [ ] `pipeline/align_stack_config.rs:126-129,225-244,300-306`, `pipeline/error.rs` —
      `cosmic_ray` moves to `CalibratedLights`; `linear()` and `CosmicRayOnLinearLights` deleted.
- [ ] `pipeline/mosaic_preparation.rs`, `pipeline/light_files.rs:141-155`,
      `pipeline/pipeline_frame/mod.rs` — `MosaicPreparation::prepare` takes a `MosaicLight` and
      returns a `CalibratedLight`, `LightFiles::prepare` a `PreparedLight`: under
      `Resample::Drizzle` a colour mosaic stays
      `Prepared::Mosaic`, and its stars are detected on a green proxy made for the detection;
      `PipelineFrame::Resident` holds a `Prepared`.
- [ ] `pipeline/align_stack_run.rs:47-56,304-312,412-424` — `LightWorker` a slot in both
      schedules; `PassWorker` and the separate `detectors` and `warped` vectors deleted.
- [ ] `pipeline/frame_registrar.rs:107-170` — `park` takes a `Resampler`; `Resampler::Drizzle`
      holds a `Drizzler` whose `OutputSums` are rebuilt from the buffers the tier returns
      (`drizzle/accumulator/mod.rs:132`, `drizzle/drizzle_run.rs:278-282` allocate them a frame
      today).
- [ ] `pipeline/light_shape.rs` — the output descriptor and the stored layout come from
      `Resample::resampled`.
- [ ] `pipeline/registered_set.rs:86-103` — the fill gate after the combine under
      `Resample::Drizzle`; the report's groups assembled from what each stage returned.
- [ ] `drizzle/drizzle_run.rs`, `drizzle/drizzle_shape.rs`, `drizzle/drizzle_stack_config.rs`
      (new) — `DrizzleStackConfig` and its three entries; `DrizzleRun` fills a `RunShape`, drizzles
      through the recycling `Drizzler`, reads `Combined::depth` in place of the depth it sums a
      frame today (`drizzle_run.rs:224-262`), and gates by `DrizzleConfig::gate`;
      `drizzle_shape.rs` deleted.
- [ ] `pipeline/light_files.rs:35-60` — `Preparation` deleted: `LightFiles::calibrated` and
      `LightFiles::linear`.
- [ ] `pipeline/detected_frame.rs`, `pipeline/align_stack_run.rs:40-100,215-225` —
      `DetectedLight` with `try_map`; `DetectedFrame`, the three rebuild methods, `LightIntake`
      and the `unzip` of frames from their diagnostics deleted.
- [ ] `pipeline/frame_registrar.rs:44-65`, `pipeline/registered_set.rs:43-72` — `ParkedLight {
      frame, outcome, calibration }`; `Parked` and the match that mirrors it into
      `FrameRegistration` deleted.
- [ ] `pipeline/calibration_notes/`, `pipeline/frame_parking.rs:32,72,80` — deleted:
      `CalibrationReport` collects the returned outcomes, and `FrameParking::hold` returns whether
      it wrote.
- [ ] `pipeline/align_stack_result.rs:13,28,36` — `lights` private with `lights()`;
      `reference()`, `stacked_count()`, `dropped()` as an iterator; the `reference` field and
      `registered()` deleted.
- [ ] `combine/normalization/norm_fit.rs:85-96` — the common domain is ANDed into a
      `BitBuffer2` a worker as each frame is stored, and merged after the stage: no coverage plane
      is read back for it.
- [ ] `progress/progress_monitor.rs` — `From<CancelToken>`; `lens` stops spelling
      `ProgressCallback::default()`.
- [ ] Tests: RAW mosaics through `calibrate_align_stack_paths` under `Resample::Drizzle` against
      the same lights drizzled with the warps in hand; drizzle at scales 1.5, 1.1 and 0.5;
      `drizzle_cfa_paths`; a Point sampling has no `pixfrac` to set.

## Batch 9 — Scratch ownership

`lumos/src/scratch_pool/`, `lumos/src/star_detection/resources.rs`,
`lumos/src/star_detection/star_detector/`. Needs batch 5.

- [ ] `star_detection/resources.rs:36-67` — the frame planes become named owned fields; stage
      functions take disjoint borrows; `heap_bytes` follows from the struct.
- [ ] `scratch_pool/mod.rs:83-215` — `Pooled<'p, T>` borrows its pool; `Arc`, `generation` and
      the stale-return branch deleted; `Pooled::loose` fixtures own a pool.
- [ ] `star_detection/tests/pool_working_set.rs` — deleted with `PEAK_FLOATS`, `PEAK_SURFACES`,
      `PEAK_BITMASKS`.

## Batch 10 — Test time

`lumos/tests/memory_budget/`, `lumos/src/pipeline/tests/`.

- [ ] `tests/memory_budget/star_field.rs:106` — the fixture's `sync_all` deleted: no assertion
      reads the disk.
- [ ] `tests/memory_budget/registered.rs` — `a_flat_divided_run_stays_within_its_budget`
      (1.06 s) split into its two table rows; `a_spilled_run_that_finds_its_reference…` (1.06 s)
      at `SIDE` 128 with `SPILLED` derived again, as the cosmic-ray test runs.
- [ ] `pipeline/tests/mod.rs:891-934` — `calibrated_lights_on_disk_stack_as_held_ones_do_*`
      (1.01 s and 0.99 s) on a smaller field: each runs four full stacks.
