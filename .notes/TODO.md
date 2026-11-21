- move the macOS `application:openURLs:` hook (`darkroom/src/platform/macos/open_files.rs`) into palantir's winit host as `App::open_files` once palantir moves past winit 0.30 and its `objc2-app-kit` 0.2
- dropping a file on the canvas, once winit 0.31

- stacking frames (outer, inner, reference)
- auto chosen reference frame for stacking and manually specified
- visual node groups

- **A registered run that drizzles its mosaics.** Resampling is the one step of a registered run
  that varies: `AlignStackConfig::resample` as `Resample::{Warp(WarpConfig), Drizzle(DrizzleConfig)}`,
  as Siril and PixInsight offer drizzle as an option of the registered stack, so a RAW drizzle
  calibrates each light once.
  - Under `Drizzle` a colour mosaic stays one (`Prepared::{Linear, Mosaic}`) and deposits by
    colour; its stars are detected on a green proxy made for the detection alone. A demosaic before
    a drizzle is two interpolations, which `Warp` already is.
  - `LightWorker` holds a `Resampler::{Warp(Option<WarpResult>), Drizzle(Drizzler)}`; the
    `Drizzler` keeps its `OutputSums` from frame to frame, rebuilt by `recycle` from the buffers
    the spilled tier returns, as a `WarpResult` cycles.
  - `Resample::resampled(input, deposit)` gives the light shape its output descriptor and stored
    layout; the fill gate runs after the combine.
  - The depth the gate reads comes out of the combine (`Combined::depth`), which already gathers
    each frame's drop weight at each pixel. Refused: a depth plane a worker; a second pass over the
    stored frames.
  - `DrizzleStackConfig { drizzle, stack }` keeps the warps-in-hand entries (`drizzle_paths`,
    `drizzle_cfa_paths`, a generic `drizzle_images`) for a caller with its own registration or
    weight maps, on the same `RunShape`, `Drizzler` and gate.
  - With it, outcomes travel with the light: `DetectedLight` carries its calibration outcome, and
    the calibration report and the parked count are collected from what each stage returned,
    replacing `CalibrationNotes`' atomics; the normalization's common domain is ANDed per worker as
    frames are stored.
  - Tests: RAW mosaics through `calibrate_align_stack_paths` under `Drizzle` against the same
    lights drizzled with the warps in hand; scales 1.5, 1.1 and 0.5; `drizzle_cfa_paths`.
