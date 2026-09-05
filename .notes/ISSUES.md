# Issues

- `palantir` `Slider` never reports `committed` for a click that does not
  latch a drag: `show` reads only `response.left.drag.stopped()`, so a press
  and release on the track writes the value with no commit edge behind it.
  `DragValue` may have the same gap.
- `palantir` widgets outside the colour family still write `Node` private
  fields (`flags`, `size`, `gaps`) and reach crate-private numeric helpers,
  against the "Widgets use the public API" rule the guide now carries. The
  colour widgets are the only ones on the public path.
- `palantir` image updates allocate wgpu staging memory on every call,
  including repeated updates before the next submission.
- `palantir` `ColorSurface::texel_size` returns a two-texel axis when the
  recorder's maximum image dimension is one, so registering that surface fails.
- `palantir` image handles carry texture ids scoped to their original host;
  drawing a handle in another host can sample that host's unrelated image
  with the same id.
- `palantir` `ColorSurface` treats equal 64-bit hashes as equal paint inputs,
  so distinct colours with the same hash leave the previous texture unchanged.
- `palantir` `IconRegistry` documentation describes deferred image-release
  reporting, while image handles free registered textures immediately.
- `palantir` frame-benchmark result writing treats an unreadable history file
  as empty and can overwrite its existing contents with only the newest result.
