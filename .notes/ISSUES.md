# Open issues

- darkroom never checks `ImageHandle::is_resident`. After the window host recovers from a lost GPU device, every image darkroom registered paints nothing: the canvas dot tile (`darkroom/src/gui/pane/graph_ui/canvas_bg.rs`), the viewer checker tile (`darkroom/src/gui/pane/image_viewer/mod.rs`, `draw_checker`) and every preview thumbnail and full texture in `darkroom/src/gui/state/preview_store`, which keeps no texels or source to upload again once a full texture is resident. A full texture also keeps the size `Ui::max_image_dimension` capped it to on the lost device.
- The graph canvas pans by an exact `TranslateScale`, so a pan off the physical pixel grid at zoom 1, as a zoom to the pointer that returns to 1 leaves, paints nodes, wires and text on fractional pixels.
- The document title in the menu bar is centered on the window whatever the menu titles take, so in a window 500 logical px wide it paints over the "View" menu title.
