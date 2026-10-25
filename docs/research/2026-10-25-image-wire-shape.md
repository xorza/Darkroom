# The image on a wire (2026-10-25)

Facts gathered for the plan that gave lens's `Image` one shape; its decisions
are in `AGENTS.md` and `lumos/AGENTS.md`. Workspace at `e9ea505c0`.

## The types

- imaginarium holds pixels alone: `InterleavedImage` (nine formats, private
  storage `ImagePixels`) and `PlanarImage` (`f32`, one plane or three, private
  storage `Planes`). Its `AGENTS.md` refuses a metadata field on either.
- lumos wraps the planes: `LinearImage` is a `PlanarImage` with `pub metadata`
  (`ImageMetadata`), `flags` (`Option<PixelFlags>`) and `flat_gain`
  (`Option<FrameGain>`) (`lumos/src/image/linear_image.rs:23`). `CfaImage`
  holds the same three beside one mosaic plane and a `CfaType`.
- lumos's `PreviewImage` (`lumos/src/image/preview_image.rs:12`) is
  `Planes(LinearImage)` or `Interleaved { image, metadata }`. lens's private
  `Pixels` (`lens/src/image/mod.rs:44`) is `PlanarCpu(LinearImage)` or
  `InterleavedCpu { image, metadata }`. `From<PreviewImage> for Image` maps
  one arm onto the other, one to one.
- So the same asymmetry is in two crates: the planar arm holds a frame type
  with metadata, flags and flat gain inside it; the interleaved arm holds a
  pixel type with metadata beside it and no flags or flat gain.

## Where each arm is made

- `Pixels::PlanarCpu`: `From<LinearImage>` (every astro node output, and
  `Image::from_planes`), `LoadCtx::preview` of a FITS or camera-RAW file, and
  the cache codec.
- `Pixels::InterleavedCpu` with default metadata: `From<InterleavedImage>`,
  the output of every imaginarium node (Brightness / Contrast, Convert, Blend,
  Transform) and of Load Image.
- `Pixels::InterleavedCpu` with decoded metadata: only `LoadCtx::preview` of a
  TIFF, PNG or JPEG (`lumos/src/io/raster.rs:134`), through Load Astro Image,
  and the codec's read-back of such an image.
- `into_interleaved` drops the metadata, and imaginarium ops take samples
  alone, so an imaginarium node's output carries no metadata, flags or flat
  gain, whatever its input carried.

## Who reads the wire image

- lens: `take_planar` (only `lens/src/astro/nodes/runtime.rs:30`),
  `take_interleaved` (Brightness / Contrast, Save Image), `interleaved()`
  (Convert, Blend, Transform), `desc()`, `ram_bytes()`, the codec.
- darkroom: `interleaved()` in the preview converter
  (`darkroom/src/gui/state/preview_store/preview_converter.rs:161`, `:179`),
  `Image::from` in tests, `Image::from_planes` in a bench.
- `PreviewImage` has no reader outside lumos's tests but lens's `From`.
- `LinearImage` is named in 116 files; its `metadata` field is read 327 times
  in lumos.

## The cache codec

- `lens/src/image/codec/mod.rs`, version 5. Its record is an enum of two
  arms: `Interleaved { metadata }` and `Planar { metadata, flat_gain,
  flagged }`, with `expect(clippy::large_enum_variant)`. The flag plane
  follows the samples only for the planar arm.

## The side limit

- `LinearImage::new` asserts `frame_desc::MAX_SIDE` (2³⁰,
  `lumos/src/image/frame_desc.rs:13`). `Image::into_planar` and
  `Image::planar` of an interleaved image build a `LinearImage` with no check
  first, so an interleaved input with a side past 2³⁰, wired into an astro
  node, panics in `assert_frame`. The codec checks the limit for its planar
  arm (`frame_desc::checked`).

## Established practice

- The Rust `image` crate: ops work on a typed `ImageBuffer<P, C>`; a decoder
  returns `DynamicImage`, an enum of one arm per pixel type, which is the type
  of a value whose format is known only at run time.
- libvips: one dynamic `VipsImage` holds the band format and the header
  fields, and an operation copies the header fields of its inputs to its
  output. The metadata belongs to the image, whatever its format. Its bands
  are always interleaved.
- OpenCV: one dynamic `cv::Mat` of any depth and channel count, interleaved;
  `cv::split` makes one `Mat` per channel. It carries no metadata.
- Halide `halide_buffer_t` and NumPy `ndarray`: one strided type, where
  planar and interleaved are two stride orders. A kernel that needs a
  contiguous layout checks the strides at run time.
