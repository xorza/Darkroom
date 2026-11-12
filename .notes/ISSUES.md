# Open issues

- `darkroom/src/gui/state/preview_store` never checks `ImageHandle::is_resident`. After the window host recovers from a lost GPU device, every preview thumbnail and full texture paints nothing, and the store keeps no texels or source to upload again once a full texture is resident. A full texture also keeps the size `Ui::max_image_dimension` capped it to on the lost device.
- `statrs` calls std's libm in `Normal` and `StudentsT`, which `lumos`'s normal scores and sigma bounds read, so those can still differ by a last bit per system.
- `lumos/src/calibration/defect_map/mod.rs`: `DefectMap::ram_bytes` counts a `usize` and one `f32` per hot pixel, but `HotPixels` holds three vectors per pixel (`idxs`, `excess`, `noise`), so the `noise` bytes go uncounted.
- `lumos/src/internals/visual/report.rs`: its `mod tests` carries no `#[cfg(test)]`.
