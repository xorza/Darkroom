# Issues

- `palantir` image updates allocate wgpu staging memory on every call,
  including repeated updates before the next submission.
- `palantir` `ScrollWrappers::split` writes its two wrapper nodes' sizing,
  padding, panel knobs and flags through crate-private `Node` fields, against
  the "Widgets use the public API" rule the guide carries.
- `palantir` `Scroll`, `Splitter` and `Grid` install a layout mode and a grid
  cell through `Node::set_mode`, `Node::scroll_spec` and `GridCell::set_main`,
  which the public API has no answer for.
- `palantir` `Panel`, `Grid` and `Popup` resolve their chrome and clip default
  through the crate-private `Node::resolve_container_chrome`, reached over the
  widget's `node` field.
- `palantir` `ScrollWrappers::split` drops the caller's input scope: it copies
  sense, disabled and focusable onto the outer wrapper and leaves the key
  filter behind.
- `palantir` rustdoc reports broken intra-doc links: `approx`'s module doc
  links the private `FloatHash`, `F32Ext` links the private `F32Px`, the
  `hsv` and `okhsv` docs link `RgbaF32::rgb` / `RgbaF32::linear_rgb`, and
  `ColorSwatch` links `Response::clicked`.
