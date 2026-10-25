- scrolview on newnode popup
- stacking frames (outer, inner, reference)
- auto chosen reference frame for stacking and manually specified
- visual node groups
- wire zips
- simd consolidation
- make symmetric enum Pixels {
  /// One `f32` plane per channel: what the `lumos` astro ops read and write.
  PlanarCpu(LinearImage),
  /// Interleaved samples, what the `imaginarium` ops take, and what their decoder stated of them:
  /// default for an op's output, which states nothing.
  InterleavedCpu {
  image: imaginarium::InterleavedImage,
  metadata: ImageMetadata,
  }
