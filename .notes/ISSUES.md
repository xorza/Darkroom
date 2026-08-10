# Issues

- `cargo clippy -p lumos --all-features --target aarch64-unknown-linux-gnu` emits 133 warnings, 132
  of them `unsafe_op_in_unsafe_fn` (E0133) on intrinsic calls inside `unsafe fn`, in
  `median_filter/simd/mod.rs` (50), `centroid/gaussian_fit/simd/neon.rs` (39),
  `centroid/moffat_fit/simd/neon.rs` (25), `threshold_mask/simd/neon.rs` (17),
  `centroid/simd.rs` (1) and `registration/resample/row/simd/neon.rs` (1). The verification chain
  builds only the host target, so no NEON code is compiled by it and `-D warnings` never sees these.
