# Issues

- `resample::warp` at 1024² mono Lanczos3 measures ~7.4 ms against ~4.0 ms for its two computing
  parts benched separately (`plane::warp` ~1.5 ms, `quality::maps` ~2.5 ms). The ~3.4 ms difference
  is the per-call allocation: a zeroed 4 MB output plane, plus dropping 12 MB of image and quality
  planes. `Buffer2::new_default` also zero-fills the two quality planes that `quality::maps` then
  overwrites pixel by pixel.
