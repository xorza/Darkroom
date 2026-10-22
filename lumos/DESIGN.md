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

- **A warp reads which taps hold data off the flag bytes.** A window near an excluded pixel builds
  its lanes of 1 and 0 from the bytes (`Validity`), the same values the f32 validity plane held, so
  every sample is the same bits. Refused: the validity plane, 4 B a pixel per warp slot for a copy
  of the flags; a select of the weights in place of the product, whose `+0.0` against `0 × w =
  −0.0` under a negative lobe changes the sums' bits.
- **A dilation grows the columns a strip at a time.** The vertical van Herk pass runs on a strip
  of `STRIP_WIDTH` columns down the whole height in place, so a worker holds two strips' block
  ORs whatever the reach, and a warp's flag scratch drops from 8 B a pixel to 2. Refused: two
  whole planes of block ORs, 48 MB at 24 MP; bands of rows, whose scratch grows with the window,
  and a warp's window grows with its stretch, which no plan knows. A strip writes its own columns
  of each row through a raw pointer, as the warp writes its rows.
- **A run reads each file once.** `FileSet` reads each source's identity and its cache commit
  once, before any decode. The plan peeks the first file the cache lacks, and that file's decode
  takes what the peek read: a camera RAW's whole file, or the frame itself for a format with no
  header that answers. When the cache holds every file, the shape comes from frame 0's commit,
  with no decoder, as none runs. The first stage prices the read until its frame takes it.
  Refused: a `FrameInfo` in each commit, as a keep does not know what its decoder held, and the
  plan needs it only for the files it will decode.
- **A RAW file goes once its sensor data is unpacked.** The mosaic route frees the file
  (`libraw_recycle_datastream`) before it allocates the frame, so a decode peaks at the raw buffer
  and the larger of the file and the frame (`DecoderBytes::peak`). LibRaw's own processing reads
  the file again, and keeps it.
- **A digest is taken of bytes in RAM, never of a map read back.** A keep hashes the slice it
  writes; a reuse into RAM copies each plane first and hashes the copy, and skips the samples an
  image does not carry. Only a mapped reuse hashes its maps, whose pages the combine reads next.
- **One writer decides what a stored frame keeps.** `FrameWrite` writes every frame a sink
  holds, the run's scratch or a kept frame's files in the decode cache, and `FrameQuality::keeps`
  and the set's `SetKind` decide its flags and samples, for the resident tier and a parked
  reference too. Refused: a writer per tier and one for the cache, which disagreed on whether a
  reference gathers samples no normalization of a registered set reads.
- **A tier hands back the buffers it writes.** `StorageTier::store` takes a frame's buffers by
  value and returns them on the spilled tier, so a warp worker refills the set in its slot; the
  resident tier moves them into the frame. Refused: a pool of warp sets left through an unsafe
  `into_inner` on the resident tier.
- **A light the decode cache keeps parks where it lies.** On the spilled tier, a linear light's
  decode returns the cache's maps beside its image, and parking takes them, the reference's
  too, rather than write the same bytes to the scratch. A light written for its parking writes
  its flat gain with its planes, as no plan prices a gain held in RAM. A mosaic changes in its
  preparation, and still spills.
- **A flag plane travels with its counts.** A stored image keeps the `FlagCounts` its writer
  had, and a commit records them, so a reference's mask, a kept frame's quality and whether its
  flags stay are decided without a read of the plane. Refused: the `Carries` tag, one
  consequence of the counts; a copy of the plane to count it.
