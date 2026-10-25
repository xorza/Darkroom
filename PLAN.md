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

### 2. Preview metadata

- [ ] `PreviewImage` becomes the enum; lens's `Image` keeps a raster's
  metadata. Proof: a test loads a FITS and a TIFF through "Load Astro Image"
  and reads the header and the provenance back from its output. The issue
  "Load Astro Image outputs an image with default metadata" is deleted from
  `.notes/ISSUES.md`.

### 3. Geometry and the grid in `common`

- [ ] `Size2us`, `Vec2us` and `URect` (as `Rect2us`) move from
  `lumos/src/math/` to `common` with their tests, public at its root. Proof:
  the moved tests pass in `common`.
- [ ] `Buffer2` holds a `Size2us`: `new`, `new_default`, `new_filled`,
  `from_fn` and `reshape` take `impl Into<Size2us>`, and `size()` replaces
  `Size2us::from(&buffer)`. Every caller is rewritten. Proof: the `Buffer2`
  tests, with sizes that are not square, so a transposed size fails.
- [ ] Every `width, height` pair in imaginarium becomes a `Size2us`:
  `ImageDesc`, `Preview`, the drawing canvas, the bench scene (`sky`), the
  test helpers (`create_test_image`) and each fn that takes the pair. Proof:
  the tests of each.

### 4. One CPU detection, one bench policy

- [ ] `SimdTier` moves to `common`, with `dispatch` and `at_or_below` public
  for imaginarium. lumos's `Avx2Fma::detect` imports it from `common`.
  `common`'s crate doc names CPU features. Proof: its test passes in
  `common`.
- [ ] `common::bench` holds `Kind`, `Scale` and `fresh_copies` behind
  `internals`, which turns on an optional `criterion`. The fixtures of
  imaginarium and lumos keep only their own parts. The reason of the root's
  `criterion` bans names `common::bench::Scale`. Proof: each crate's bench
  target lists the ids it had, and runs each case once.

### 5. lumos's cycles

- [ ] The calibration bundle moves to `calibration`, and the product file to
  `stack_product`. Each type reads and writes through its own methods;
  `lumos::writer`, `LoadCtx::calibration_masters` and `LoadCtx::product` go.
  Proof: the module graph of the research note, run again, has no edge from
  `io` to `calibration` or `stack_product`; the bundle and product round-trip
  tests pass through the methods.
- [ ] `lumos::decode` moves into `io::decoder`. Proof: the frame-cache tests
  pass, and the decode version is the same digest as before.

### 6. `libraw`

- [ ] A workspace crate `libraw` takes `io/raw/libraw.rs`, the codec facts,
  the sensor layout, LibRaw's error codes and the raw color data, with their
  tests. lumos depends on it, and on `libraw-sys` only through it. Proof: the
  RAW tests pass; `rg 'unsafe' lumos/src/io/raw` finds nothing.

### 7. Names

- [ ] `drawing` moves to `lumos::internals::visual`; `Color` goes, and `Rgb`
  replaces it. Proof: the drawing tests pass in lumos; imaginarium has no
  `drawing` module.
- [ ] The `glam` re-export goes, and lens depends on `glam`.
- [ ] `Image` becomes `InterleavedImage` and `ImageDesc` becomes
  `InterleavedDesc`; the aliases `CpuImage`, `RawImage` and `LensImage` go.
- [ ] `Transform` becomes `AffineWarp`, and `GpuTransformPipeline` becomes
  `GpuAffineWarpPipeline`.
- [ ] `L` becomes `Mono` in `ChannelCount`, in the `ColorFormat` constants, in
  `ImagePixels` and in lumos's `LinearPixels`; `grey` becomes `mono` in names.
- [ ] `FileFormat` becomes `RasterFormat` with serde; lumos's mirror goes.
  Proof: a provenance round-trips through its RON form.
- [ ] `GpuContext` becomes `GpuCtx`; `ALL_FORMATS` becomes `ColorFormat::ALL`;
  byte counts take `len` (`SampleType::byte_len`, `ColorFormat::byte_len`,
  `InterleavedDesc::byte_len`, `row_byte_len`); the rest of the merge research
  note's list follows. Then compare imaginarium's words in
  `cargo drive vocabulary` with the workspace's, and rename each word that has
  a second meaning. Proof: no word of the listing has two meanings.

### 8. Both layouts in imaginarium

- [ ] `PlanarImage`, `PlanarDesc` and `ChannelLayout` move to imaginarium; the
  layout conversions move with them. lumos's `LinearPixels`,
  `ImageDimensions` and `ChannelLayout` go, and lumos keeps `MAX_SIDE` as its
  decode limit. Proof: the conversion tests move along; a round trip of each
  of the nine formats through the planes is exact for the formats planes hold.
- [ ] `InterleavedImage::without_alpha`, and no lossy `From`. Proof: an RGBA
  image refuses a `TryFrom` into planes; `without_alpha` keeps the color
  channels exactly.

### 9. Errors

- [ ] `Error` splits into `CodecError`, `DescError`, `PlanesError` and
  `GpuError`. Each case holds fields, not a message, and a foreign error as
  its own type. `InterleavedDesc` and `PlanarDesc` have private fields, `new`
  and `checked`. `InterleavedImage::new_black` returns the image.
  `RasterFormat::from_path` returns an `Option`. lumos's `ImageError` and
  lens's `DecodeError::InvalidImage` take the new types. Proof: a test for
  each case that the code can return, with its fields; the descriptor tests
  take zero sides and an overflowing size.

### 10. Layouts

- [ ] lumos's `image/metadata/` and `image/flags/`. Proof: `cargo drive check`;
  no file moves without its tests.
- [ ] imaginarium's `interleaved/`, `planar/`, `color/` and `codec/`. Proof:
  `cargo drive check`; no file moves without its tests.

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
