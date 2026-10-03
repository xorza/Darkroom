# Issues

- `cargo doc -p lumos --no-deps --document-private-items --all-features` reports unresolved
  intra-doc links: `CalibrationMasters::from_files` and `Self::from_roles`
  (`lumos/src/calibration_masters/calibration_set.rs:57-58`), `StoredPlane::chunk`
  (`lumos/src/combine/cache/core.rs:4`, `:96`), `crate::frame_store::frame_spill::CachedQuality`
  (`lumos/src/frame_store/frame_quality.rs:187`), and `SampleDomain::commensurate_with`
  (`lumos/src/io/image/image_metadata.rs:70`).
