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
- **Pixels alone.** `InterleavedImage` and `PlanarImage` hold samples; lumos
  wraps the planes with its metadata, flags and flat gain. Refused: an image
  with a metadata field, which would carry FITS keywords into the raster ops.
