# Lumos design decisions

- **Loading through `LoadContext`, saving through the `io` writer.** Loading is methods on
  `LoadContext` (`linear`, `cfa`, `preview`, `cfa_info`, `product`, `calibration_masters`), and
  saving is `io::writer`. Refused: load and save methods on the image types, which made the data
  layer import `io`.
- **The stacked product is one FITS file in astropy `CCDData`'s layout.** The master is the
  primary HDU, with `MASK`, `UNCERT` (`UTYPE = 'InverseVariance'`), `COVERAGE`, `WEIGHT` and
  `DISPERSION` beside it, so `CCDData.read` loads it and every plane round-trips bit for bit.
  Refused: a master-only writer, which drops the quality planes, and a layout of our own, which
  no other tool reads.
- **The product states what its weight plane sums, and scales it by that.** `StackProduct`
  carries its `WeightKind`, the primary header states it in `LUMWGTK`, and a `WEIGHT` plane of
  inverse variances takes the `1/scale²` that `UNCERT` takes, as DrizzlePac keeps an IVM `WHT` in
  its image's units. Refused: an unscaled plane with no stated kind, which a reader cannot
  interpret, and a plane normalized to relative weights, which loses its inverse variance.
- **CCD-model rejection refuses a set with a frame of unknown gain.** `RejectionScale::CcdModel`
  fails before the combine and names the first such frame, as IRAF `ccdclip` requires a gain.
  The inverse-variance plane keeps its background-only model with its `RunReport` flag, and
  cosmic-ray detection keeps `σ²/sky`. Refused: a silent fall back to the empirical scale, which
  rejects by another rule than the one configured.
- **The small-sample rule applies per pixel.** `SmallN` reads each pixel's own sample count, and
  a pixel below `min_frames` uses `SmallN::fallback`; `min_survivors` stays the floor rejection
  may not cut below. Refused: one rule for the whole stack from its frame count, which treats a
  pixel that few frames cover as fully sampled.
- **The cosmic-ray detector is built per light.** It reuses one working set across its
  iterations and the four Bayer phases, and frees it after the pass. Refused: a detector kept per
  worker, which saves the page faults, about 1 % of the pass, but holds 0.6–0.7 GB per slot
  through demosaic and star detection, so a fixed budget runs fewer lights at once; caching
  allocators and buffer reuse pay where the allocation is a large share of the work. Source:
  `research/2026-10-16-cosmic-ray-page-faults.md`.
