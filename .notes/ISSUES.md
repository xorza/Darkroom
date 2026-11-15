# Open issues

- `darkroom/src/gui/state/preview_store` never checks `ImageHandle::is_resident`. After the window host recovers from a lost GPU device, every preview thumbnail and full texture paints nothing, and the store keeps no texels or source to upload again once a full texture is resident. A full texture also keeps the size `Ui::max_image_dimension` capped it to on the lost device.
