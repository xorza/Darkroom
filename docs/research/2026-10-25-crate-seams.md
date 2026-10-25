# Crate seams and image types (2026-10-25)

Facts gathered for the plan that set the crate seams; its decisions are in
`AGENTS.md`, `imaginarium/AGENTS.md` and `lumos/AGENTS.md`. Workspace at the
merge of `imaginarium-workspace-crate` into `master` (`92eed8893` and
`14b984112`).

## Sizes

- lumos: about 140 000 lines. The biggest top modules: `star_detection`
  25 k, `registration` 19 k, `combine` 16 k, `io` 15 k, `image` 12 k,
  `calibration` 12 k. lens: 5 000 lines. imaginarium: 11 000 lines.
- `lumos/src/image/` holds 33 files at its top level: pixel containers
  (`linear_pixels`, `image_dimensions`, `channel_layout`, `rgb`,
  `masked_plane`), metadata (`observation_header`, `derived_facts`,
  `image_metadata`, `image_provenance`, `fits_checksum_provenance`,
  `fits_transfer_provenance`, `sample_domain`, `sample_type_record`,
  `calibration_state`, `dark_comparison`, `temperature_difference`,
  `temperature_source`, `unverified_conditions`, `mosaic_noise`), flags
  (`pixel_flags`, `quality_flags`, `flag_counts`, `quality_map`) and the frame
  types (`linear_image`, `preview_image`, `stackable_image`).

## lumos's module graph

Production code only, tests and benches left out. Each arrow is a `crate::`
path in the first module that names the second.

- `io` → `calibration` (`io/fits/calibration_bundle`, `io/writer.rs`) and
  `io` → `stack_product` (`io/fits/product`, `io/fits/lumos_extension.rs`,
  `io/writer.rs`). `calibration` → `combine` → `io`. So `io`, `calibration`
  and `combine` form a cycle.
- `noise` → `image` (`MosaicNoiseMeasure` reads `CfaImage`, `FlatGain`,
  `PixelFlags`). `image` → `noise` is test code only. `image` → `io` is one
  doc link (`stackable_image.rs:16`).
- `decode/` (131 lines: `DecoderKind`, `DecodePins`, `DecodeParams`, `fnv1a`)
  is the identity of a decode that a frame cache keys on. Its users are
  `frame_store/decode_cache` and `calibration/master_recipe`; `io/decoder`
  names `DecoderKind` in its `Decoder` trait.

## What the decoders build

- The RAW and FITS decoders build lumos's science types directly. From
  `crate::image` they name `CfaType`, `Mosaic`, `CfaPattern`, `QualityFlags`,
  `ImageDimensions`, `SampleDomain`, `LinearImage`, `PixelFlags`,
  `ImageProvenance`, `SharedGains`, `FitsTransferProvenance`, `ChannelLayout`,
  `CfaImage`, `ImageMetadata`, `FitsChecksumProvenance`, `ObservationHeader`,
  `LinearPixels`, `FrameGain`, `DerivedFacts`, `PreviewImage`, `FlagCounts`,
  `XTransPattern` and `MarkesteijnPasses`. The RAW decode measures the
  mosaic's noise (`noise::mosaic_noise_measure`) as it decodes.
- `LoadCtx` reads `CalibrationMasters` and `StackProduct` files beside the
  frames, and its preview path runs an X-Trans demosaic.
- The LibRaw wrapper (`io/raw/libraw.rs`, 433 lines, every `unsafe` call into
  `libraw-sys`) names from lumos only `ImageDimensions`, `ChannelLayout`,
  `Size2us` and its own error and codec types. `sensor_layout.rs` names only
  `Size2us`, `Vec2us` and `RawError`.
- FITS has its format crate already: fits-well reads and writes the bytes, and
  `lumos/src/io/fits` maps them into the science types.

## Image types

- imaginarium: `Image` (interleaved, nine formats), `ImageDesc` (width, height,
  `ColorFormat`), `ChannelCount` (`L`, `Rgb`, `Rgba`), `FileFormat` (PNG, JPEG,
  TIFF), `Color` (an `f32` RGB triple for `drawing`).
- lumos: `LinearPixels` (`L` one plane, `Rgb` three, `f32`),
  `ImageDimensions` (`Size2us` and `ChannelLayout`), `ChannelLayout` (`Mono`,
  `Rgb`), `LinearImage` (metadata, `LinearPixels`, flags, flat gain),
  `CfaImage` (metadata, one plane, `CfaType`, flags, flat gain),
  `PreviewImage` (metadata and `PreviewPixels`), `PreviewPixels`
  (`Planes(LinearImage)` or `Interleaved(Image)`), `Rgb` (an `f32` RGB triple),
  `RasterFormat` (TIFF, PNG, JPEG, with serde, and
  `From<FileFormat> for RasterFormat`).
- lens: `Image` (`PlanarCpu(LinearImage)` or `InterleavedCpu(imaginarium::Image)`).
- Users: `ImageDimensions` in 99 lumos files and 4 lens files, `ChannelLayout`
  in 72 and 4, `LinearPixels` in 12 lumos files.
- `LinearPixels` names its one-plane variant `L`; lumos's `ChannelLayout`
  names the same layout `Mono`.
- The conversions between the two layouts are in lumos
  (`image/linear_image.rs`, `image/linear_pixels.rs`).
  `From<&Image> for LinearImage` maps `Rgba` to `RGB_F32`, so it drops an alpha
  channel without a word. `io/raster.rs` records the same drop as
  `ColorProvenance::UnmanagedRaster { alpha_dropped }`.
- `PreviewImage::from(LinearImage)` moves the metadata out of the planes into
  the preview, so a `PreviewPixels::Planes` holds a `LinearImage` with default
  metadata. lens's "Load Astro Image" node keeps only the pixels
  (`lens/src/astro/nodes/io.rs:54`), so its output has default metadata on
  both paths.
- `LinearImage` and `CfaImage` each implement `flags`, `flat_gain`,
  `set_flags`, `set_flat_gain` and `take_flags`, and `StackableImage` over
  them.
- `lumos::writer` holds five public free functions, one for each type it
  writes. Their read sides are methods of `LoadCtx`.
- The `drawing` functions and `Color` have callers only in lumos tests and
  `lumos::internals::visual`.
