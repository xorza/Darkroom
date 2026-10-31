# Open issues

- darkroom never reads `Ui::device_restored`. After the window host recovers from a lost GPU device, every image darkroom registered paints nothing: the canvas dot tile (`darkroom/src/gui/pane/graph_ui/canvas_bg.rs`), the viewer checker tile (`darkroom/src/gui/pane/image_viewer/mod.rs`, `draw_checker`) and every preview thumbnail and full texture in `darkroom/src/gui/state/preview_store`, which keeps no texels or source to upload again once a full texture is resident. A full texture also keeps the size `Ui::max_image_dimension` capped it to on the lost device.
