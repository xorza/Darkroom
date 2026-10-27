# Open issues

- **imaginarium: five `atomic` NEON cases slower than before the `simd` port on an M2.**
  `contrast_brightness/cpu/neon/rgb_f32` (+19 %), `affine_warp/cpu/neon/bilinear/rgba_f32`
  (+14 %) and `rgba_u16` (+7 %), `convert/cpu/neon/mono_u8/rgb_u8` (+10 %) and `rgb_u8/rgba_u8`
  (+11 %), against master `dbfb50926`, ABBA order, three rounds. x86 has no case slower beyond
  its cross-build noise.
- **darkroom: the lib test binary crashed with SIGSEGV on the ubuntu runner.** CI run 40434805558
  (master `4b3e7399b`, lavapipe) ended `cargo test -p darkroom --lib` with "signal: 11, SIGSEGV"
  before any test reported; the next run, 40437898986, did not crash. Locally it never crashed.
