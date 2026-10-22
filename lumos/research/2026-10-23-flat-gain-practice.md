# A flat's gain: who holds it, and how it reaches a stack (2026-10-23)

In lumos:

- A `FlatGain` is a node grid every 4 pixels, 4 bytes a node for each channel: about 0.25 B a
  pixel for each channel, 18 MB for an RGB frame of 6000 × 4000, 5.8 % of its 13 B a pixel.
- `PreparedFlat::record` gives each light it calibrates an `Arc` clone of the master's gain: one
  grid for the set, held by the masters, which the run's plan does not count.
- A FITS frame with a `LUMGAIN` extension decodes its own copy, and a frame the decode cache reads
  back into RAM copies its gain planes. Lights one flat calibrated carry equal grids.
- The warp and the drizzle make a grid for each frame, the source's gain read through the frame's
  map (`FlatGain::warped`). `StoredLayout::warped` prices it, and the spilled tier writes it.

Practice elsewhere:

- LSST Science Pipelines: `MakeDirectWarpTask` applies each exposure's `PhotoCalib` to its masked
  image (`calibrateImage`) before the warp, and `warpExposure` warps the image and its variance
  plane together: the calibration's effect on the noise becomes a full-resolution plane for each
  frame
  ([make_direct_warp.py](https://raw.githubusercontent.com/lsst/drp_tasks/main/python/lsst/drp/tasks/make_direct_warp.py),
  [MakeDirectWarpConfig](https://pipelines.lsst.io/py-api/lsst.drp.tasks.make_direct_warp.MakeDirectWarpConfig.html)).
  The calibration is an object an exposure refers to
  ([PhotoCalib.h](https://pipelines.lsst.io/v/DM-22499/cpp-api/file/_photo_calib_8h.html)).
- JWST pipeline: `flat_field` writes a full-resolution `VAR_FLAT` into each exposure and divides
  `VAR_POISSON` and `VAR_RNOISE` by flat²; `resample` resamples every variance array onto the
  output grid, for each frame
  ([error propagation](https://jwst-pipeline.readthedocs.io/en/stable/jwst/error_propagation/main.html),
  [resample](https://jwst-pipeline.readthedocs.io/en/2.0.0/jwst/resample/main.html)). An open
  report says the variances are resampled without the science weights
  ([#8344](https://github.com/spacetelescope/jwst/issues/8344)).
- SWarp resamples each input's weight or variance map with its image (`WEIGHT_TYPE MAP_WEIGHT`)
  and coadds by weighted mean
  ([default.swarp](https://ftp.eso.org/scisoft/scisoft4/sources/terapix/swarp1.21/config/default.swarp),
  [GaBoDS](https://arxiv.org/pdf/astro-ph/0501144)).
- stcal and DrizzlePac drizzle each input on its own, weighted by exposure time or by inverse
  variance arrays of the read noise or the sky (`ivm`, `ivm-sky`)
  ([build_driz_weight](https://stcal.readthedocs.io/en/latest/api/stcal.resample.utils.build_driz_weight.html),
  [adrizzle](https://drizzlepac.readthedocs.io/en/latest/api/drizzlepac.adrizzle.drizzle.html)).
- ccdproc's `flat_correct` carries a full-resolution uncertainty array for each frame, and
  `combine` bounds its memory with `mem_limit`
  ([combine](https://ccdproc.readthedocs.io/en/latest/api/ccdproc.combine.html)).

None found computes a frame's warped noise when the combine reads it: each stores it for the
frame and warps it with the image. Each loads a calibration product once and refers to it.
