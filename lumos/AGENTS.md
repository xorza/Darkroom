# Lumos

Astronomical stacking pipeline: **load / decode (RAW, FITS) → calibrate →
detect stars → register → combine (stack or drizzle)**, then an optional
non-linear **stretch**, strictly after all linear-domain work. CPU only:
vector kernels written once over the `simd` crate's `Isa` (AVX2+FMA, NEON, a
portable fallback) and rayon. Pixels are **planar** f32, one plane per
channel, normalized to `[0, 1]`.

## Scope

The **most precise and the fastest** stacking pipeline, growing toward a
**science data product**: the linear master plus per-pixel quality planes
(coverage, weight, variance) a downstream tool measures it with — photometry,
source extraction, error bars.

- **The stacked master comes first.** A science extra must stay simple and
  ride on data the pipeline already computes. Machinery that serves neither
  the image nor its measurement is removed, not carried.
- **Thin-plate-spline distortion and drizzle stay**, even without a
  production caller.
- **Precision outranks speed.** Never trade the stacked result's accuracy for
  throughput.

## Memory

- **The memory budget is soft: it steers caching and spilling, and never refuses
  work.** A run reads the memory it plans for once, at its entry — the
  caller's override, else what the system reports available — and plans with
  a share of it whether frames stay resident or spill to the run's scratch,
  and how wide each stage runs: it trades memory for disk I/O. A stage that
  needs more than the budget runs anyway; it logs its overcommit and
  continues.
- **No decode or stage is refused for its size.** Nothing compares what a file
  or a stage needs with the budget or with the system's memory. An allocation
  the system cannot back stops the process. Refused: a ceiling of a share of
  the available memory or of the physical RAM, which blocks files the machine
  can load.

## I/O

- **A codec reads a reader and writes a writer; a path stays at the edge.**
  FITS images, calibration bundles, stack products and rasters decode over
  `Read + Seek` and encode over `Write + Seek`, and report path-free errors.
  The loaders, the pipeline's entry points and the save fns open the paths and
  name them in the error, as the `image` crate splits `ImageReader::new(reader)`
  from `open`. Tests decode from bytes. Refused: paths
  through the codecs, which tie every test to a temporary file.
- **A path-taking entry point stays a method of its config**, as
  `StackConfig::stack_paths`: the frame cache reads files for it either way.
  Refused: moving these entry points into `pipeline`, which needs free fns or a
  type of its own for them.

## API and types

- **One entry per source, a method of its config.** `align_stack_images`, `align_stack_paths`
  and `calibrate_align_stack_paths` stay distinct constructors of a run, named by their source
  ([C-CTOR](https://rust-lang.github.io/api-guidelines/predictability.html#c-ctor)); they share
  their run, their planner and their argument order. Refused: one entry over an input enum.
- **`LinearImage` and `CfaImage` are `Frame<PlanarImage>` and `Frame<MosaicPlane>`**, one set of
  accessors over a sealed `Samples` trait, as the `image` crate's
  [`ImageBuffer`](https://docs.rs/image/latest/image/struct.ImageBuffer.html) stands under
  `RgbImage` and `GrayImage`. Refused: two structs with the same three companions, whose checks
  had drifted apart.
- **`DecodeConfig` is the one statement of how a file decodes**: `LoadCtx`, `IngestConfig`, the
  decode cache's key and a master's recipe all read it, and the key digests it whole, so a new
  option keys the cache by construction.
- **`ImageError` is a path and its `LoadError`**: one case per decoder, its own error kept whole,
  and the outcomes a caller tells apart whichever decoder met them — a cancel, a refusal as
  scientific input, nulls the policy refuses — one case each.
- **Kept: `image` ↔ `io`.** The one top-level cycle, through `write_fits` and `write_raster`: a
  type and its codec, placed by the I/O decisions above.
- **Kept: the calibration bundle** (`read_bundle`, `write_bundle`), the masters' public file
  form beside the per-role cache, with no caller in the workspace.

## Loading

- **`DATAMAX` is the array's maximum**, "the maximum valid physical value represented by the
  array" ([FITS keyword dictionary](https://heasarc.gsfc.nasa.gov/docs/fcg/standard_dict.html)):
  it decides a float HDU's scale and flags nothing by default. A saturation level is read from and
  written to `SATURATE`, SExtractor's default `SATUR_KEY`, and taken as given ("the level at which
  arises saturation"); a load that reads `DATAMAX` as MaxIm DL's saturation level
  ([header definitions](https://cdn.diffractionlimited.com/help/maximdl/FITS_File_Header_Definitions.htm))
  takes 0.95 of it, the RAW decoder's margin below an ADC ceiling. `SATURATE` wins where both are
  stated; a written file carries no `DATAMAX`.
- **`ROWORDER` states the rows as stored**, the keyword being the "order of the rows in image
  array" ([Siril, FITS orientation](https://siril.readthedocs.io/en/stable/file-formats/FITS.html)),
  with `BAYERPAT` in that convention; a frame of no source order is written `TOP-DOWN`. Refused:
  `TOP-DOWN` always with the truth in a keyword only lumos reads; reordering rows on write, which
  mirrors a master against its sub-frames.
- **`preview` reads a three-plane FITS cube as RGB** whatever `FitsCubeInterpretation` says, and
  clamps a mosaic's preview to `[0, 1]` from either container, recorded as `clipped`, through one
  `LoadCtx::demosaic_preview`; `linear` and `cfa` keep refusing an undeclared cube.

## Calibration

- **A sensor setting both sides state must agree** for a bias or a dark — gain, ISO, offset,
  binning — as [Siril's calibration](https://siril.readthedocs.io/en/latest/preprocessing/calibration.html)
  asks; a side that states nothing counts as unverified, and a filter differing between flat and
  light is recorded, not refused. Refused: recording a stated difference and calibrating anyway.
- **Dark current changes 12 % a degree**, doubling every 5.5–6.3 °C (Widenhorn et al., Proc. SPIE
  4669, 2002). A dark-scale fit replaces the exposure ratio where its relative standard error is
  below that ratio's uncertainty — `12 % · max(|ΔT|, 1 °C)` where a temperature compared, a third
  where none did — as PixInsight accepts its scale on a noise criterion
  ([master frames](https://pixinsight.com/tutorials/master-frames/)). Refused: a pixel count as the
  gate.
- **A master cache keys by what lumos composes**: `MasterKey`, a blake3 digest of each frame's
  name and identity, the recipe and the subtracted masters' keys, read before the stack. The cache
  is a `&dyn MasterCache` that never fails a build — a miss on load, a warning on store — as
  Siril's and PixInsight's process containers keep masters by their inputs.
- **The cosmic-ray pass is an option of the calibrated lights**, so linear lights cannot ask for
  it, and a worker keeps its `CosmicRayDetector`'s planes from one light to the next.

## Detection

- **A saturated star is kept and flagged**, by any saturated pixel in its core, as SExtractor's
  `FLAGS` bit 4 ([flagging](https://sextractor.readthedocs.io/en/latest/Flagging.html));
  registration matches on it and fits without it. Refused: dropping it, which keeps the brightest
  stars out of the pattern match.
- **Flux and SNR are measured over a circle of radius `1.75·FWHM`**, each boundary pixel weighed
  by its exact area inside, as photutils'
  [`CircularAperture`](https://photutils.readthedocs.io/en/stable/api/photutils.aperture.CircularAperture.html)
  `exact` method, so neither figure steps with the FWHM. Refused: a whole-pixel square.
- **A region has no maximum area by default**, as SExtractor ships `DETECT_MAXAREA 0`. Refused: a
  limit derived from the FWHM, which needs a profile to turn brightness into area.
- **A background tile reads at most 1024 samples**, its median and MAD to about 3 % and 4 % at one
  σ, which the mesh's spline and median filter average further. Refused: every pixel, four times
  the statistics' cost for a precision no stage downstream resolves.
- **Kept: two separations.** `deblend_min_separation` says which peaks are two stars;
  `duplicate_min_separation` drops the fainter of two stars nearer than it, photutils'
  [`DAOStarFinder`](https://photutils.readthedocs.io/en/stable/api/photutils.detection.DAOStarFinder.html)
  `min_separation`.

## Registration

- **`Auto` under SIP fits the plain models beside the corrected ones**, RANSAC run once a
  transform type, and the GRIC (Torr 1998, made for nested models) charges the correction two
  parameters a term, so a frame too sparse for the correction registers by a plain model. Refused:
  SIP candidates alone, which drop a light of 8–20 pairs a plain model fits.
- **A position σ the catalogs miss is an excess variance the final fit estimates**: each pass
  solves for the `σ_e²` that puts the median of `dᵢ/√(σᵢ² + σ_e²)` at the Rayleigh median
  `√(2 ln 2)` and weighs each pair by `1/(σᵢ² + σ_e²)`, 0 where the formal σ explains the
  residuals, as Astrometry.net and SCAMP add a systematic floor in quadrature to each centroid's
  error.
- **A SIP fit's rotation is closed form**: with the terms partialled out, the weighted Procrustes
  angle `atan2(Σ r̃ × t̃, Σ r̃ · t̃)` (Schönemann 1966), then one solve for the terms.
- **A `ReferenceCatalog` owns its stars**, checked and made once for every target; a reference no
  light can register to fails the run as its own error. A `Star` is `Copy`, and a borrow would tie
  the catalog to the frame they were moved out of.
- **The warp keeps surface brightness**, sampling the source at each output pixel with no
  Jacobian, as Siril, PixInsight's StarAlignment and DSS register; a pixel outside the source's
  footprint reads 0 with no coverage.

## Combine

- **`Combine` is three cases**: `Median`, `Mean`, `RejectedMean { rejection, small_n }`, the
  reducer chosen once, not per pixel. Refused: `Mean(Rejection::None)`, a "no rejection" spelled
  inside the rejection.
- **No manual weights.** When a caller needs one, the weight rides on the frame, as IRAF
  `imcombine`, ccdproc and PixInsight hold it per frame. Refused: a `Vec` in the configuration.
- **The variance planes state the clipped mean's asymptotic variance**, `σ²/(n·Z·tv)` for a band
  of ±k (Huber, *Robust Statistics*, §3.2). At ten frames it overstates the realized variance by
  3–19 %, where the survivors' own variance reads 6–12.5 % low: overstating is the conservative
  side. Refused: a finite-sample table, which holds for Gaussian frames alone.
- **A small-N pixel of one or two samples takes the weighted mean**, whose variance is exact; of
  three or more it keeps the median and states `f(n)·mean(vᵢ)/n`, `f` the Gaussian median's
  small-sample factor, tabulated to 32 samples and checked against quadrature. Refused: 0, which a
  consumer reads as no data.
- **The norms' reference is the frame least noisy once normalized**, `min Σ(g·σ)²`, the frame a
  weighting by `1/(g·σ)²` ranks first. Refused: the offset from paired sample medians, which
  widens the offset's error from 0.0003σ to 0.005σ.
- **`RunShape` lives in `frame_store`**: it joins a plan to the storage the run writes, and in
  `memory` it would make the two depend on each other.

## Ownership kept

- **The scratch pool's leases own a share of it** (`Arc` with a generation), so a stage keeps them
  in the structs it hands the next one; its owner alone reshapes it, and a stale or reshaped lease
  is dropped on return. Refused: a lease that borrows its pool, a lifetime on every detection
  stage and fixture for no behaviour.
- **`Parked` holds a stored frame exactly when the light is stacked.** Refused: an
  `Option<StoredFrame>` beside the light's outcome, which could disagree.
