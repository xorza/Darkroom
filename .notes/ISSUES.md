# Open issues

- darkroom never checks `ImageHandle::is_resident`. After the window host recovers from a lost GPU device, every image darkroom registered paints nothing: the viewer checker tile (`darkroom/src/gui/pane/image_viewer/mod.rs`, `draw_checker`) and every preview thumbnail and full texture in `darkroom/src/gui/state/preview_store`, which keeps no texels or source to upload again once a full texture is resident. A full texture also keeps the size `Ui::max_image_dimension` capped it to on the lost device.
- `statrs` calls std's libm in `Normal` and `StudentsT`, which `lumos`'s normal scores and sigma bounds read, so those can still differ by a last bit per system.
- The load report calls a path on Windows with a root and no drive, `\x`, or a drive and no root, `C:x`, a path of another system.
