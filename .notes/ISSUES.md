# Open issues

- **imaginarium: four `atomic` NEON cases slower than before the `simd` port on an M2.**
  `affine_warp/cpu/neon/bilinear/rgba_f32` (×1.089, 238 → 260 µs) and `rgba_u16` (×1.084,
  266 → 288 µs), `convert/cpu/neon/rgb_u8/rgba_u8` (×1.105, 0.37 → 0.41 µs) and
  `mono_u8/rgb_u8` (×1.050, 0.24 → 0.25 µs): medians against `dbfb50926`, master `ad4b6baa7`,
  ABBA order, three rounds, on an idle M2. `contrast_brightness/cpu/neon/rgb_f32` is now
  ×0.604. x86 has no case slower beyond its cross-build noise.
- **darkroom: the lib test binary crashed with SIGSEGV on the ubuntu runner.** CI run 40434805558
  (master `4b3e7399b`, lavapipe) ended `cargo test -p darkroom --lib` with "signal: 11, SIGSEGV"
  before any test reported; the next run, 40437898986, did not crash. Locally it never crashed.
