# Open issues

- **The `editor_frame` bench graph overlaps its own nodes.** In `darkroom/src/gui/app/session/bench.rs`, `ROW` (220) is shorter than the `Transform` and preview node heights, so each chain's tall nodes cover the chain below, and `COLUMN` (280) equals the node width, so nodes of one chain touch edge to edge. Most wires sit under nodes, and the right ~45% of the bench window is empty canvas.

- **The `editor_frame` bench publishes previews in a layout lumos does not.** `stacked_frame` in `darkroom/src/gui/app/session/bench.rs` builds an interleaved `lens::Image` and calls it "a stacked frame as lumos publishes one", but the lumos nodes publish `PlanarCpu` images. `prepare_drawable` reads a planar image through `lens::Image::interleaved`, which repacks the whole image into a new allocation first, so the `preview` scenario does not time that repack.
