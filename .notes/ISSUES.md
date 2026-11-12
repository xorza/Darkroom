# Open issues

- `darkroom/src/gui/state/preview_store` never checks `ImageHandle::is_resident`. After the window host recovers from a lost GPU device, every preview thumbnail and full texture paints nothing, and the store keeps no texels or source to upload again once a full texture is resident. A full texture also keeps the size `Ui::max_image_dimension` capped it to on the lost device.
- `statrs` calls std's libm in `Normal` and `StudentsT`, which `lumos`'s normal scores and sigma bounds read, so those can still differ by a last bit per system.
- `scenarium/src/graph/mod.rs`: the `Graph` doc and the test-only `clone_verbatim` doc say `Graph` is deliberately not `Clone`, but `Graph` derives `Clone`, which leaves `clone_verbatim` a second route to the same copy.
- `lumos/src/calibration/defect_map/mod.rs`: `DefectMap::ram_bytes` counts a `usize` and one `f32` per hot pixel, but `HotPixels` holds three vectors per pixel (`idxs`, `excess`, `noise`), so the `noise` bytes go uncounted.
- `lumos/src/image_ops/extract_bg/mod.rs` (module doc and `ExtractBg` doc) and `NeutralizeBg`'s doc place background extraction on the linear master before color calibration, while `extract_bg/bench.rs` and `extract_bg/tests/real_data.rs` run it on the stretched master.
- `lumos/src/internals/visual/report.rs`: its `mod tests` carries no `#[cfg(test)]`.
- `lumos/src/math/dmat3/tests.rs`: `identity_is_the_multiplicative_identity_and_the_default` never exercises `Default`.
