# Plan: imaginarium in the workspace, and the crate seams

imaginarium is a member crate, as lumos is. Each crate gets one
responsibility, and each image type one home: `common` holds the grid and its
geometry, imaginarium holds pixels in both layouts, format crates hold a file
format's bytes, and lumos holds the astronomical science. Facts and counts:
`docs/research/2026-10-25-imaginarium-merge.md` and
`docs/research/2026-10-25-crate-seams.md`. When the plan lands, each lasting
decision moves into `AGENTS.md`, `imaginarium/AGENTS.md` or the doc comments
of the code it governs, and this file is deleted.

## Decisions

### The crates

- **The layers.** `common`: the grid, its geometry, I/O and threads.
  `imaginarium`: pixels, in either layout, their sample formats, the raster
  codecs and the generic ops. `fits-well` and `libraw`: a file format's bytes
  and its own semantics, with no science. `lumos`: the astronomical science,
  that is the frame model (pixels with metadata, flags and flat gain), the
  decoders that map a format into it, the stages, and the products with their
  files. `lens`: the nodes. `darkroom`: the editor.
- **imaginarium is a member crate.** Its planar `From`/`TryFrom` impls start
  parallel work, and no clippy ban can name a trait impl. As a member, it calls
  `common::par` and `common::fs` itself. Refused: bans and wrappers as for
  fits-well, which leave those impls open.
- **Its history comes along.** `git subtree add` of its `main` keeps the 130
  commits for `git log` and `git blame`. Refused: a flat copy in one commit,
  which keeps the history only in the GitHub repository.
- **RAW, FITS and the metadata stay in lumos.** The decoders build 23 of
  lumos's science types as they decode (sample domain, pedestal, provenance,
  quality flags, flat gain), measure the mosaic's noise, and read calibration
  masters and stack products. In imaginarium they would make it an astronomy
  crate and take lumos's noise statistics and demosaic with them. The format
  layer is what moves out: FITS has fits-well, and RAW gets `libraw`.
  Refused: the decoders and the metadata in imaginarium; a new crate for the
  frame model and its decoders, which needs lumos's cycles out first and makes
  many crate-private items public.
- **A safe `libraw` crate beside `libraw-sys`.** The `-sys` and safe-wrapper
  pair Rust uses for a C library. `libraw` holds every `unsafe` call into
  LibRaw: open from memory, the cancel callback, unpack, the sizes, the color
  data, the codec facts and the processed image. It depends on `common` alone.
  `lumos::io::raw` keeps the mapping into `CfaImage`: the CFA pattern, the
  black levels and the flags they set, the saturation, the white balance and
  the metadata. Refused: the FFI in lumos, which mixes soundness and science in
  one module.
- **The root's lanes cover imaginarium.** The drive table has the CPU-only
  lint set `{ all-features-but = ["imaginarium/wgpu"] }` and
  `bench = "--features imaginarium/wgpu"`. The root's profiles, `clippy.toml`
  and `.cargo/config.toml` replace the crate's own.
- **Not now.** A split of lumos into crates: its module graph has cycles,
  which come out first; then a split is judged on measured build times. A
  generic `Frame<P>` for `LinearImage` and `CfaImage`: they share five
  accessors, and their flat gains differ in shape (per pixel or per mosaic
  color).

### Shared primitives

- **The 2D grid and its geometry live in `common`.** `Buffer2`, `Size2us`,
  `Vec2us` and `URect` know nothing of images, and lumos, lens, darkroom and
  `libraw` use them. Refused: a new crate for four types, which `common`'s leaf
  rule already admits; leaving them where they are, which makes lens and
  darkroom depend on lumos for a size.
- **`URect` becomes `Rect2us`.** One suffix for the three geometry types.
- **`Buffer2` holds a `Size2us`.** Its constructors and `reshape` take a size,
  and `size()` replaces lumos's `From<&Buffer2<T>> for Size2us`. `width()` and
  `height()` stay as accessors. Refused: a `width, height` pair, which a call
  site can transpose and the compiler cannot see (the doc of `Size2us`).
- **One CPU detection, two kernel designs.** `SimdTier` moves to `common`, and
  lumos's `Avx2Fma::detect` reads it there. Refused: one kernel framework.
  lumos's `simd::Isa` has `f32` and `f64` lanes only, and imaginarium's kernels
  pack `u8` and `u16` and shuffle bytes. A merge rewrites every imaginarium
  kernel with no measured gain.
- **One bench policy.** `Kind`, `Scale` and `fresh_copies` move to
  `common::bench`, behind `internals`, from two copies that differ only in
  doc comments. Each crate keeps its own scenes and id parts. Refused:
  palantir's fixture, which a standalone crate cannot share.

### The image types

- **imaginarium holds both pixel layouts.** `InterleavedImage` gets
  `PlanarImage`: lumos's `LinearPixels`, `f32` planes, one or three.
  `ImageDimensions` becomes `PlanarDesc` and `ChannelLayout` moves with it;
  `ImageDesc` becomes `InterleavedDesc`, so the two descriptors pair. The
  conversions between the layouts move from lumos to imaginarium. lumos's
  `LinearImage` and `CfaImage` hold imaginarium's planes, with lumos's
  metadata, flags and flat gain. lumos keeps its own decode limit,
  `MAX_SIDE`, which comes from its `i32` lanes. Refused: an imaginarium image
  with a metadata field, which would carry FITS keywords into the raster ops.
- **The interleaved image is `InterleavedImage`.** `imaginarium::Image` takes
  the new name. It pairs with `PlanarImage` and with lens's
  `Pixels::InterleavedCpu`, and it removes three aliases. Refused: a new name
  for `lens::Image`, which is what a wire carries and what the editor calls an
  image.
- **A layout change that loses data is a named method.** `From` and `TryFrom`
  are lossless. Today `From<&Image> for LinearImage` drops an alpha channel
  without a word. A drop becomes `InterleavedImage::without_alpha`, which
  each caller names: lumos's raster decode, which records it in the
  provenance already, and lens's crossing into the planar domain.
- **A preview holds its metadata once.** `PreviewImage` becomes an enum:
  `Planes(LinearImage)`, whose metadata it keeps, or
  `Interleaved { image, metadata }`. lens's `Image` keeps the metadata of a
  raster too, so "Load Astro Image" stops dropping it. Today both paths lose
  it.
- **The GPU backend stays behind `wgpu`.** No workspace crate turns it on
  today, but lens's `Image` reserves a device variant for it. The CPU-only
  lint set and the bench build keep it built and linted. Refused: deletion,
  about 1 500 lines with their tests, which a device path would write again.

### Files and errors

- **Reads and writes take a reader or a writer.** `Image::read_file` and
  `Image::save_file` are removed, and their bans go with them. Callers open
  through `common::fs`.
- **A file sits with the type it stores.** The calibration bundle moves to
  `calibration`, and the product file to `stack_product`. The FITS extensions
  both of them write (`plane_hdu`, `flags_extension`, `gain_extension`,
  `lumos_extension`) stay in `io::fits`. That removes the `io` →
  `calibration` → `combine` → `io` cycle. Each type reads and writes through
  its own methods, as the Code rules want of a fn that takes one of its types:
  `LinearImage::write_fits`, `CalibrationMasters::write_bundle`,
  `StackProduct::read_fits`, and the rest. `lumos::writer` goes. `LoadCtx`
  keeps the frame decoders, which pick a format from the extension.
- **The identity of a decode lives with the decoders.** `lumos::decode`
  (`DecoderKind`, `DecodePins`, `DecodeParams`, `fnv1a`) moves into
  `io::decoder`, beside the `Decoder` trait whose `KIND` it names. One home
  for "decode", where there are two today.
- **One raster-format enum.** imaginarium's `FileFormat` becomes
  `RasterFormat`, the name lumos's provenance gives the same three formats,
  and derives `Serialize` and `Deserialize`. lumos's mirror enum and its
  `From` go. This adds `serde`, a workspace dependency already, to
  imaginarium. Refused: the mirror, which exists only because imaginarium was
  standalone.
- **Errors are cases, each in its domain.** The crate-wide `Error`, with six
  `String` variants, splits: `CodecError` for decode and encode, `DescError`
  for a descriptor, `PlanesError` for a planar conversion, `GpuError` for the
  device. Each keeps the foreign error as its own type. `InterleavedDesc` and
  `PlanarDesc` are valid by construction: `new` panics, `checked` returns the
  `DescError`. So `InterleavedImage::new_black` returns no `Result`.
  `RasterFormat::from_path` returns an `Option`, as `from_extension` does.
  Refused: one `Error` of messages, which a caller cannot match on and whose
  name every crate has.

### Names and layout

- **The one-channel word is `mono`.** `ChannelCount::Mono`,
  `ColorFormat::MONO_U8` and the rest, as lumos's `ChannelLayout::Mono`
  (`mono` 739 times in lumos, 0 in imaginarium). `luma` keeps its one meaning,
  the Rec. 709 weighted sum. Refused: `L`, the PIL mode name, which no other
  crate uses; `gray`/`grey`, which the crates spell both ways.
- **The resample op is `AffineWarp`.** `imaginarium::Transform` and
  `lumos::Transform` are both public. lumos calls the resample through a
  transform a warp (`WarpConfig::warp`), and lumos keeps `Transform` for the
  transform. lumos's crate-wide `Warp` already names a warp to run, so the
  narrower use takes the qualified name, as the Names rule says.
- **No re-export of `glam`.** lens depends on `glam` and names `glam::Vec2`.
  `glam` is a workspace dependency already.
- **`drawing` moves to lumos's test code, and `Color` becomes `Rgb`.** Only
  lumos tests and `lumos::internals::visual` draw. The circles and crosses
  move to `lumos::internals::visual`, and lumos's `Rgb` replaces
  imaginarium's `Color`, the same `f32` triple. Refused: `drawing` behind
  imaginarium's `internals`, which keeps test code for one crate in another.
- **lumos's `image` module groups by subject.** `image/metadata/` takes the
  header, the derived facts and what they hold (provenance, domain,
  calibration state, dark comparison, temperatures, mosaic noise).
  `image/flags/` takes the pixel flags, the quality flags, the flag counts and
  the quality map. `cfa/` and `flat_gain/` stay. The frame types stay at the
  top: `linear_image`, `preview_image`, `stackable_image`.
- **imaginarium's layout follows its responsibilities.** `interleaved/` (the
  image, its descriptor, its storage and the format conversion), `planar/`,
  `color/` (formats, samples, luma), `codec/` (the raster formats, decode and
  encode), `ops/` and `gpu/`.

## Plan

Each batch ends with `cargo drive check` and `cargo drive test` green from the
root. Each item states its proof. The order: the defect first; then the
primitives that later batches build on; then lumos's cycles, which stand
alone; then the names, so the planar layout arrives under its final names;
then the errors over both descriptors; the file moves last, as they collide
with every other edit.

### 10. Layouts

- [ ] lumos's `image/metadata/` and `image/flags/`. Proof: `cargo drive check`;
  no file moves without its tests.
- [ ] imaginarium's `interleaved/`, `planar/`, `color/` and `codec/`. Proof:
  `cargo drive check`; no file moves without its tests.
- [ ] "dimensions" leaves lumos's names. A name that holds a `PlanarDesc`
  (`dimensions`, `dims`, `input_dims`, a field or a parameter) becomes `desc`,
  as `PlanarDesc` and `desc()` say; one that holds a `Size2us`
  (`DefectMap::dimensions`, `DetectionResources`, `bg_mesh`) becomes `size`.
  Each file goes by the type of each name, not by text. Proof:
  `cargo drive vocabulary` lists no `dimensions` in a workspace crate.

## Changed

- Batch 1, the lanes: no `cross-lint`. The lane lints every feature, and
  `ort-sys` has no `x86_64-apple-darwin` binaries for lumos's `ml`, in
  rc.15 or on its `main` of 2026-10-23. `imaginarium/AGENTS.md` gives the
  one-crate clippy for the x86 kernels instead.
- Batch 1, the bench proof: `cargo drive bench --test` fails to link, as
  `.notes/ISSUES.md` records. imaginarium's own bench target ran instead: its
  391 ids equal those of `93cae6e`, and each case passed once.
- Batch 1, dependencies: `strum` and `tokio` stay, which the plan wrongly
  called unused (`BlendMode`'s derives, the GPU readback). `strum`'s `derive`
  replaces `strum_macros`. lumos names `glam`'s `f64`, which imaginarium's
  default features gave it before.
- Batch 2, the cache codec: lens's disk-cache codec wrote an image's pixels
  alone, so a cached image lost its metadata, its flags and its flat gain.
  Version 5 writes a pinned bitcode record, one variant per layout, of the
  metadata and a planar image's flat gain, and the flag plane after the planes;
  the record names the layout, so no byte of the header can disagree with it. For it, `PixelFlags::from_bytes`,
  `LinearImage::set_flags`, `LinearImage::set_flat_gain`, `FlatGain::covers`
  and `ImageDimensions::checked` are public, with release asserts on the
  setters. lens's memory figure for a planar image counted its samples alone;
  `LinearImage::ram_bytes`, as `CfaImage` has, adds the flag plane and a gain
  the image holds alone, and a gain read back from the cache is the image's
  own.
- Batch 3, constructors: `ImageDesc::new` and `Preview::new` take a
  `Size2us`, not `impl Into<Size2us>`: both are `const fn`, which cannot call
  `Into`. `Buffer2`'s constructors take the sugar. `from_fn` keeps its
  `(x, y)` closure, the order of `Buffer2`'s indexing. The bench scene's
  `WIDTH` and `HEIGHT` became one `FRAME`, and `LinearImage::size` joined its
  `width` and `height`, as `CfaImage` has one. The pairs outside imaginarium
  went too: lens's `Image::from_planes`, three test helpers, and the errors
  that carried a width and a height (`DimensionsError::SidePastMax` and
  `SampleCount`, `BundleError::DefectExtentOutOfRange`,
  `DrawablePreviewError::EdgeTooLong`) now take a `Size2us`.
- Batch 4, the bench policy: `Scale`'s variants are named for the time one
  iteration takes, `Micros`, `Millis` and `Slow { iteration }`, where they were
  `Sample`, `Frame` and `Set`. In `common`, which knows nothing of images,
  "frame" and "set" named nothing, and "frame" means an exposure in lumos and
  a pass of the render loop in the editor. The proof listed each crate's bench
  ids in the test profile: the bench profile's fat-LTO link has no measured
  peak, which a bench build needs before it starts.
- Batch 5, the third edge: `io::fits::lumos_extension` named the product's
  `WeightKind` and constants. It now holds `WeightUnits`, the units a weight
  plane is in, which `WeightKind::units` gives; the quality-plane format tags
  moved there from the product file; and the product's two error cases carry
  the value they expected. So no `io` module names `stack_product`. The
  `io::fits` items the moved files read are `pub(crate)`. The frame types'
  write methods make `image` depend on `io`, beside `io` on `image`: a cycle
  inside the frame layer, which a split of lumos keeps in one crate. Refused:
  the `impl` blocks in `io`'s files, which breaks one struct per file with all
  its impls; free functions, which `lumos::writer` was.
- Batch 5, the product's failures: `io`'s `FitsRefusal` held five cases of the
  product file alone. They are `ProductRefusal` beside the product file, as
  the bundle has `BundleError`, and `StackProduct::read_fits` returns a
  `ProductError`: a frame-layer `ImageError`, or a refusal with its path.
- Batch 6, the crate's surface: `libraw` gives LibRaw's state as its own
  value types (`SensorLayout`, `ImageData`, `ColorData`, `Capture`,
  `CodecFacts`), where lumos read `libraw_data_t` itself, and takes
  `ProcessParams`, where lumos wrote `libraw_output_params_t`. Its
  `LibrawError` holds what LibRaw refuses; lumos's `RawError` wraps it and
  keeps the science refusals, and checks the raw pitch and the processed
  colors itself. `RawDecoder::classify` stays in lumos: which codecs are lossy
  is lumos's reading of LibRaw's facts. `LibrawCode` is `libraw`'s, and lumos
  no longer exports it.
- Batch 7, drawing: the moved circles and crosses take `RGB_F32` alone. Every
  caller draws on an RGB overlay, and the mono path, which wrote a color's
  luma, had none. The overlay colors are constants beside them.
- Batch 7, `AffineWarp`: its module, its example and its bench ids
  (`atomic/affine_warp/…`, `integration/affine_warp/…`) take the name too, and
  a value of it is a `warp`. lens's "Transform" node keeps its label and id,
  which a saved graph binds to.
- Batch 7, `Mono`: the format names are `Mono u8` and the rest, which the
  editor's format menu and labels show. An RGB value with `R = G = B` is a
  neutral, where imaginarium said grey. lumos's one-channel `gray` and
  `grayscale` took `mono` too (`LinearImage::is_mono`,
  `ImageDimensions::is_mono`, the test helpers); `gray` stays for a neutral
  color.
- Batch 7, `RasterFormat`: imaginarium's variant order, `Png`, `Jpeg`,
  `Tiff`, replaces the order of lumos's mirror.
- Batch 7, the vocabulary: three imaginarium words had a second meaning. The
  Rec. 709 sum of encoded samples is `luma`, as darkroom's `luminance` is the
  WCAG relative luminance; the GPU-to-CPU copy is a readback, as darkroom's
  download fetches a model file; the contrast kernels that walk a row as one
  run of samples are `contiguous`, as `flat` is a flat field in lumos. The
  shaders' uniforms take `byte_len`, `sample_byte_len` and `channel_count`
  with the Rust side.
- Batch 8, `PlanarImage`: its planes are private, so no caller builds three
  planes of two sizes; `From` a plane or three checks them. lumos's
  `ImageParts` keeps its `ArrayVec`, which `into_mono` and `into_rgb` fill,
  so imaginarium takes no `arrayvec`. `DescError` comes now, with
  `PlanarDesc`; batch 9 gives it to `InterleavedDesc` too.
- Batch 8, the side limit: `lumos::frame_desc` holds `MAX_SIDE`, `checked`
  (a `PlanarDesc` or a `FrameDescError`, which replaces `DimensionsError`) and
  the decode byte counts `ImageDimensions` held. `LinearImage::new` and
  `CfaImage::from_plane` assert the limit, which `ImageDimensions` asserted on
  every construction. The four readers of untrusted sizes (the drizzle grid,
  a flat gain record, a kept frame, lens's cache) check it with `checked`,
  which is public for lens.
- Batch 8, names: `dimensions()` returning the descriptor is `desc()`, as
  `InterleavedImage::desc`, on `LinearImage`, `CfaImage`, `StackableImage`
  and the frame types; `DefectMap` and `DetectionResources` keep
  `dimensions()` for their `Size2us`. `LinearImage::channels` is
  `channel_count`. `LinearImage::color_format` went:
  `PlanarDesc::color_format` and `From<PlanarDesc> for InterleavedDesc` say
  it.
- Batch 8, the alpha drop: `LinearImage` takes a `PlanarImage`, and its lossy
  `From` impls went. lumos's preview decode and both lens crossings name
  `without_alpha`; lens's borrowed crossing of an image with alpha copies it
  first, as the drop takes the image. The linear decode refuses alpha, as it
  did.
- Batch 9, a fifth error: `ByteLenError` is what `new_with_data` returns for
  bytes of another length, which is none of a codec, a descriptor, planes or
  the device. `from_samples`, which only the decoders call, asserts its
  samples: the codecs promise them.
- Batch 9, `DescError`: `TooLarge { size, pixel_byte_len }` is the overflow
  case of both descriptors, and `PlanarDesc` now checks its byte count, where
  batch 8 checked its samples and left `byte_len` able to overflow.
  `InterleavedDesc::new` stays `const` and panics with a fixed message per
  rule.
- Batch 9, `CodecError`: a color type no format holds keeps its foreign type
  (`image::ColorType`, `tiff::ColorType`); a TIFF sample type no format holds
  is a `TiffSampleType`, a closed set; `RasterFormat` displays its name for
  `Unstorable`.
- Batch 9, the callers: lumos's `WriteError::RasterExtension` and lens's
  `RasterFileError` take the case `from_path` no longer returns. lens's cache
  checks its header through `InterleavedDesc::checked`: `InvalidDesc`
  replaces `DimensionsOverflow` and `InvalidImage`. darkroom's
  `DrawablePreviewError::Empty` went, as no descriptor is empty.
- Batch 9, the proof: each `DescError`, `ByteLenError` and `PlanesError` case
  and the TIFF refusals have tests with their fields; `GpuError` has none, as
  no test can make a device fail, and neither have `ImageColor` and an
  encoder's `TooLarge`, which need a file no encoder here writes or a
  4-GB image.
