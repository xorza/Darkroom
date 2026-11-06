# Open issues

- darkroom never checks `ImageHandle::is_resident`. After the window host recovers from a lost GPU device, every image darkroom registered paints nothing: the viewer checker tile (`darkroom/src/gui/pane/image_viewer/mod.rs`, `draw_checker`) and every preview thumbnail and full texture in `darkroom/src/gui/state/preview_store`, which keeps no texels or source to upload again once a full texture is resident. A full texture also keeps the size `Ui::max_image_dimension` capped it to on the lost device.

- `cargo doc -p darkroom --no-deps --document-private-items` warns of a redundant explicit link target in `darkroom/src/gui/pane/inspector.rs`, line 3.

- A left drag inside a node's title while its rename field is open latches the group drag and moves the node, because `wid::rename` stays a drag handle while `EditableLabel` shows its `TextEdit`.
