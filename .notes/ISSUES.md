# Issues

- `imaginarium/src/ops/blend/cpu/neon.rs` — `rgba_u8_row` loads with `vld4_u8`,
  which reads 32 bytes, while its loop only guarantees 16 (`x + 4 <= width`).
  On the last row of an image that reads past the pixel buffer.

- `imaginarium/src/ops/blend/cpu/sse41.rs` — `rgba_u8_row` is scalar arithmetic
  in vector registers: it splats one channel per `_mm_set1_ps` and reads the
  result back with `_mm_cvtss_f32`, so three of every four lanes are wasted and
  four pixels take sixteen splat/extract round trips.

- `imaginarium/src/image/conversion/simd/mod.rs` — the F32→U16 element kernel is
  wired only for `L_F32 → L_U16`. `RGB_F32 → RGB_U16` and `RGBA_F32 → RGBA_U16`
  fall to the scalar reference although the kernel is channel-agnostic and every
  other element conversion lists all three channel counts.

- `imaginarium/src/ops/contrast_brightness/cpu/neon.rs:159,205` and one site
  above them fail `cargo clippy --target aarch64-unknown-linux-gnu -D warnings`
  with `clippy::chunks_exact_to_as_chunks`. The aarch64 build is not covered by
  the usual host-only verification run.
