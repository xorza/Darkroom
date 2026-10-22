# Lumos design decisions

- **Loading through `LoadCtx`, saving through the `io` writer.** Loading is methods on
  `LoadCtx` (`linear`, `cfa`, `preview`, `cfa_info`, `product`, `calibration_masters`), and
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
- **A price sits beside what it prices, and one price serves the plan and the run.** The combine
  is priced by `CombineCost` for the plan and the frame cache alike, two chunk copies included, and
  a chunk is priced by what `ChunkCopy` copies, not the flat gain it reads where it is mapped.
  Refused: a formula per caller, which drifted (the drizzle's frames left out their flat gain).
- **A run reports the peak its plan expects, and the budget test holds a resident run to it.**
  `RunReport::planned_peak_bytes`, written as `LUMPLNB`, is what `tests/memory_budget` measures a
  resident run against, where the budget alone binds nothing. Refused: a lower bound on the same
  figure, since prices are upper bounds by design (flags and flat gain are charged to every frame),
  so a run held well below its plan is no fault.
- **A drizzle grid is refused before it is allocated.** A scale whose grid is no image's size, or
  whose one drizzled frame and depth pass the run's memory, fails the run with
  `DrizzleError::OutputSize` or `OutputOverBudget`; prices saturate rather than wrap. Refused: a
  cap on the scale, which bounds no input's size.
- **A FITS HDU is streamed from the planes it writes.** Every writer goes through `PlaneHdu`,
  which reads a chunk of rows of `io::fits::CHUNK_BYTES` from the image's own planes, blanks or
  strips each sample on the way, and hands it to fits-well's `stream_image`; the file is the one
  `write_image` writes, byte for byte. Refused: an owned `Image` staged for each HDU before the file
  opened, which held a copy of every HDU and fits-well's encoding of one, about 1.85 GB above a
  24 MP RGB product.
- **A detector holds two planes; σ and sky are surfaces.** The sky and its σ are a `MeshSurface`, the
  mesh's values and y pass, read a row or a pixel at a time by the spline's own kernel, bit for bit
  the plane it replaces; a detection plane's filters run a band of rows at a time from the
  measurement plane or the plane less its sky (`DetectionFilter`), each band computing its halo
  again into a ring of `2r + 1` rows. A detector slot drops from 5 planes to 3 at 24 MP, and
  detection runs 4–13 % faster end to end; the matched filter alone takes 3 % more, an ellipse off
  the axes 9 %, for the halo. Refused: the filters in place over column strips, which keep an
  elliptical PSF's work plane; and σ and sky planes, which a detector slot paid twice over for
  values read by row or at a peak. SEP filters a ring of rows the same way.
- **A deblend set keeps room for a star.** A set that grew past `DeblendBuffers::KEPT_PIXELS` for
  a galaxy or a nebula gives the room back after it, and a pixel is 12 bytes. Refused: keeping the
  largest component's room for the run, which a nebula of millions of pixels pinned per slot.

