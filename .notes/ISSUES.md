# Open issues

- darkroom never checks `ImageHandle::is_resident`. After the window host recovers from a lost GPU device, every image darkroom registered paints nothing: the viewer checker tile (`darkroom/src/gui/pane/image_viewer/mod.rs`, `draw_checker`) and every preview thumbnail and full texture in `darkroom/src/gui/state/preview_store`, which keeps no texels or source to upload again once a full texture is resident. A full texture also keeps the size `Ui::max_image_dimension` capped it to on the lost device.
- A node opens a path of another system as this system reads it, so on Windows `/data/a.fit` from a document saved on Linux or macOS reads `C:\data\a.fit` if the current drive holds that file, where the load report calls the path foreign.
- `statrs` calls std's libm in `Normal` and `StudentsT`, which `lumos`'s normal scores and sigma bounds read, so those can still differ by a last bit per system.
