# One shape for an image on a wire

lens's `Image` and lumos's `PreviewImage` hold an image in one of two
layouts, and each puts the metadata in a different place per arm: inside the
`LinearImage` of the planar arm, beside the `InterleavedImage` of the
interleaved arm. The flags and the flat gain exist only in the planar arm.
This plan gives the wire image one symmetric shape and removes the second copy
of the either-layout type. Facts: `docs/research/2026-10-25-image-wire-shape.md`.

## Proposal

### Target shape

```rust
// lens/src/image/mod.rs
pub struct Image {
    pixels: Pixels,
    metadata: ImageMetadata,
    flags: Option<PixelFlags>,
    flat_gain: Option<FrameGain>,
}

enum Pixels {
    Planar(PlanarImage),
    Interleaved(InterleavedImage),
}

// lumos/src/image/linear_image.rs
pub struct LinearParts {
    pub pixels: PlanarImage,
    pub metadata: ImageMetadata,
    pub flags: Option<PixelFlags>,
    pub flat_gain: Option<FrameGain>,
}
```

The `Image` API, by layout:

| Operation | Planar | Interleaved |
|---|---|---|
| Borrow the samples | `planar() -> Cow<PlanarImage>` | `interleaved() -> Cow<InterleavedImage>` |
| Take from a wire | `take_planar(value) -> InvokeResult<LinearImage>` | `take_interleaved(value) -> InterleavedImage` |
| Build | `From<LinearImage>`, `from_planes` | `From<InterleavedImage>` |

`desc()`, `ram_bytes()` and `Display` read either arm the same way. The
metadata, flags and flat gain go into a `LinearImage` on the planar take and
stay behind on the interleaved take, as the imaginarium ops take samples
alone.

The image types after the plan: imaginarium's `InterleavedImage` and
`PlanarImage`, pixels alone and named by layout; lumos's `LinearImage` and
`CfaImage`, frames named by what their samples are; lens's `Image`, the one
type whose layout is known only at run time. `PreviewImage` is deleted.

### Decisions

- **The metadata, flags and flat gain sit beside the layout, not inside one
  arm.** They state what the image is — its source, its pipeline facts, the
  quality and the flat of each pixel — and none of that changes when the
  samples change layout. The arms differ only in how the samples are stored.
  This is the libvips model, where the header belongs to the image whatever
  its band format. A flag plane is per pixel and a flat gain covers a size and
  a channel count, so both are valid in either layout: an image's channels are
  its mono or RGB planes, an alpha channel left out. Refused: the shape now,
  with a frame type in one arm and pixels with metadata in the other; each arm
  the type of its own domain (`Planar(LinearImage)`, `Interleaved(InterleavedImage)`),
  which ties the metadata to one layout, so that a lossless Convert erases
  the header and a reader of the metadata matches on the layout first.
- **The pixel types stay one per layout; the run-time choice of layout is made
  once, at the wire.** lumos's kernels take planar `f32`, and imaginarium's
  take interleaved samples, one kernel per format; each op family keeps its
  layout in its types. lens's `Image` is the boundary type, as the `image`
  crate's `DynamicImage` is for its typed buffers. Refused: one `CpuImage`
  with every layout and format inside it, and a strided buffer where the two
  layouts are two stride orders (Halide, NumPy). Each moves the layout check
  from the type into every lumos and imaginarium op, at run time, and gives
  those ops nothing in return.
- **A lumos decode hands back planes.** `LoadCtx::preview` returns a
  `LinearImage` for every format, and `PreviewImage` is deleted: lumos's
  pixels are planar `f32`, and the interleaved arm was a second copy of the
  wire type. Load Image stays the interleaved decode, with no repack. A
  preview is told from a science load by the `LoadCtx` method and the
  provenance it records, as before: `PreviewImage::Planes` gave its
  `LinearImage` to any caller. Refused: `PreviewImage` made symmetric, which
  keeps two copies of one type in two crates. Cost: a raster that Load Astro
  Image decodes, and that then goes to an imaginarium node or to the
  previewer, is interleaved again there, as a FITS or camera-RAW decode is
  now. Both repacks are lossless.
- **An arm is named after the type it holds:** `Planar(PlanarImage)` and
  `Interleaved(InterleavedImage)`; a device arm, when one is added, is
  `Gpu(GpuImage)`. Both CPU types are CPU types by definition, so the `Cpu`
  suffix said nothing.
- **A `LinearImage` moves whole into its parts and back.** `LinearParts`, with
  `From` both ways, is how lens takes the planes and the three others out of a
  `LinearImage` and puts them back, with no copy. `From<LinearParts>` asserts
  what `LinearImage::new` and `set_flat_gain` assert, as
  `From<PlanarImage> for LinearImage` asserts the side limit now. Refused:
  inherent `into_parts` and `from_parts`, which shadow the `StackableImage`
  methods of those names on the same type.
- **One cache record for both layouts:** the layout, the metadata, the flat
  gain and whether a flag plane follows. The flag plane follows the samples in
  both layouts. Refused: the record enum of two arms, which is the asymmetry
  again on disk.
- **A side past `frame_desc::MAX_SIDE` is the graph's error, not a panic.**
  `take_planar` of an interleaved input checks the side limit first and
  returns `InvokeError::invalid_input`. An interleaved image comes from a file
  or a node, which is not a contract that lumos's `assert_frame` can state.
  The raster preview checks the same limit before it builds a `LinearImage`,
  with `ImageError::RasterSideTooLong`, which the science load shares: the
  limit is a frame's, not a refusal of science input, so it left
  `ScientificRefusal`.
- **`LinearImage` keeps its name.** It names what its samples are, as
  `CfaImage` does, which is a different axis from the layout names of the
  pixel types. A rename changes 116 files and no shape.

### Not in this plan

- **Which imaginarium nodes keep which metadata, flags and flat gain.** The
  new shape can carry them through Convert or Brightness / Contrast, but each
  op changes a different part: a resample moves pixels under the flags and the
  WCS, a contrast change breaks the flat gain's meaning, a conversion to `u8`
  makes the quantization σ stale. Every imaginarium node's output carries none
  of them until a separate change decides it per node.
- **One struct for the three that `LinearImage` and `CfaImage` both carry.**
  It is the shared part of the generic frame that the root `AGENTS.md` sets
  aside under "Not now", and it changes 327 reads of `.metadata`.

## Plan

Each batch leaves `cargo drive check` and `cargo drive test` green.

### 1. The metadata, flags and flat gain beside the layout

Files: `lumos/src/image/linear_image.rs`,
`lumos/src/image/flat_gain/frame_gain.rs`, `lumos/src/lib.rs`,
`lens/src/image/mod.rs`, `lens/src/image/tests.rs`,
`lens/src/image/codec/mod.rs`, `lens/src/image/codec/tests.rs`,
`lens/src/astro/nodes/runtime.rs`.

- [ ] `lumos/src/image/linear_image.rs`: add `LinearParts`, with
  `From<LinearImage> for LinearParts` and `From<LinearParts> for LinearImage`;
  export it from `lumos/src/lib.rs`. Test: a round trip keeps the plane
  allocation (pointer equal), the metadata, the flags and the gain, and a
  flag plane of another size panics.
- [ ] `lumos/src/image/flat_gain/frame_gain.rs`: add a public
  `FrameGain::own_ram_bytes` (the grid of `Own`, 0 for `Shared`, whose owner
  counts it), and use it in `LinearImage::ram_bytes`, so that lens does not
  copy the rule. Extend
  `ram_bytes_counts_the_planes_the_flags_and_an_own_gain` with it.
- [ ] `lens/src/image/mod.rs`: `Image` and `Pixels` as in the target shape.
  `From<LinearImage>` moves the parts in. `into_planar` builds the
  `LinearImage` from the planes and the three others, with no copy for a
  planar image. `into_interleaved` keeps the samples alone. `planar()`
  returns `Cow<PlanarImage>`, borrowed for a planar image, the alpha copy kept
  for an interleaved one. A private `to_planar` copies the planes and the
  three others for a shared input. `ram_bytes` is the samples plus the flag
  plane plus `own_ram_bytes`, the same sum in both layouts. A private
  constructor `debug_assert!`s that the flags and the gain cover the image.
  Delete `planes_of`. Rewrite the `Image` and `Pixels` docs for the new shape.
- [ ] `lens/src/image/mod.rs` `take_planar`: panics in `LinearImage::new` for
  an interleaved input with a side past `MAX_SIDE`. Target:
  `InvokeResult<LinearImage>`, checked with `frame_desc::checked` before the
  deinterleave; `lens/src/astro/nodes/runtime.rs` `run_on_planes` returns the
  error with `?`. Put the check in a private fn over the `InterleavedDesc`,
  and test it there: a `(MAX_SIDE + 1) × 1` descriptor returns the error, and
  a `MAX_SIDE × 1` one passes. No test builds the 1 GiB image.
- [ ] `lens/src/image/tests.rs`: the pointer tests (`into_planar` moves the
  planes, `take_planar` moves a unique input and copies a shared one) hold as
  they are. Extend
  `a_planar_input_is_taken_without_repacking_and_a_shared_one_is_cloned`: the
  input has a header object and a flag plane, and `take_planar` returns both,
  for the unique input and for the shared one.
- [ ] `lens/src/image/codec/mod.rs`: `ImageRecord` becomes one struct (a
  `SampleOrder` of `Planar` or `Interleaved`, the metadata, the flat gain,
  `flagged`), with no `large_enum_variant` expect. Encode writes the header,
  the record, the samples in their order, then the flag plane in either order.
  Decode checks the gain against the size and the channel count without alpha
  in both orders, and `PlanarFormat` and `frame_desc::checked` in the planar
  order. `VERSION` becomes 6.
- [ ] `lens/src/image/codec/tests.rs`: `a_planar_image_round_trips_whole`
  reads the new fields. Add an interleaved image with a flag plane and a
  gain to `every_interleaved_format_round_trips_pixel_exact`, and assert the
  flag plane follows the samples. Move the pin of `record_layout_is_pinned`
  to the new digest.

### 2. Fold the decisions into the documents

Files: `AGENTS.md`, `imaginarium/AGENTS.md`, `lumos/AGENTS.md`,
`.notes/TODO.md`, `PLAN.md`.

- [ ] Root `AGENTS.md`, "Crate seams": add "the metadata, flags and flat gain
  beside the layout" and "the run-time layout is chosen once, at the wire",
  each with what it refuses. In "Names", the example
  `DrawablePreview` beside `lumos::PreviewImage` names a deleted type: give
  another example, or say why `DrawablePreview` keeps its qualified name.
- [ ] `imaginarium/AGENTS.md`, "Pixels alone": add the refused `CpuImage` of
  every layout and format. "The GPU backend stays": the device variant is
  lens's `Gpu(GpuImage)` arm.
- [ ] `lumos/AGENTS.md`, "Frames and files": add "a lumos decode hands back
  planes", with the refused `PreviewImage`.
- [ ] Delete the "make symmetric enum Pixels" item in `.notes/TODO.md`, then
  delete `PLAN.md`.
