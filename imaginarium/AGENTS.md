# Imaginarium

Pixels in two layouts — interleaved images in every supported pixel format,
and `f32` planes — with the lossless conversions between them, the raster
codecs, and the image ops on a **CPU (SIMD) and a GPU (wgpu compute)
backend**. lens runs its ops as nodes; lumos holds its frames' planes and
reads rasters through its codecs.

- **GPU is optional**, behind the `wgpu` feature (off by default). CPU paths
  compile and pass without it.
- **Every format.** A new op or shader handles the full format set, or
  declares the narrower list it supports.
- **One rounding rule.** Narrowing into an integer sample rounds to nearest,
  ties to even, and saturates; the vector kernels take it from the `simd`
  crate's clamped round, the shaders follow it, and every GPU test compares
  with the CPU result.
- **`f32` is nominal `[0, 1]` and unbounded.** 1.0 is full scale for a
  mid-point and a brightness offset; an op clamps an `f32` result only to the
  ends its `Clamp` parameter names, and an integer format saturates on store
  alone. A linear stack holds values past white and below black, and clamping
  in an op loses them. This is GIMP's float mode and the unbounded linear data
  lumos's `SampleDomain` records. Refused: a clamp to `[0, 1]` in each op,
  which made an `f32` chain lossy.
- **An op that moves black holds it by default.** Contrast/brightness and
  blend take `Clamp::Black` unless told otherwise, as Nuke's Grade node has
  its black clamp on and its white clamp off
  ([Grade](https://learn.foundry.com/nuke/content/reference_guide/color_nodes/grade.html)):
  a stars layer darkened below black otherwise takes that much from every
  pixel a later Add meets, while a highlight past white is data. A blend
  holds its color channels after the alpha mix; its modes are bare formulas.
  Refused: no clamp, as GEGL's brightness-contrast has it, which left that
  Add wrong with nothing to show it; a fixed clamp per blend mode, Add at
  white and Subtract at black, which lost a linear Add's star cores.
- **Pixels alone.** `InterleavedImage` and `PlanarImage` hold samples; lumos
  wraps the planes with its metadata, flags and flat gain. Refused: an image
  with a metadata field, which would carry FITS keywords into the raster ops.
