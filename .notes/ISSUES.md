# Open issues

- **imaginarium: five `atomic` NEON cases slower than before the `simd` port on an M2.**
  `contrast_brightness/cpu/neon/rgb_f32` (+19 %), `affine_warp/cpu/neon/bilinear/rgba_f32`
  (+14 %) and `rgba_u16` (+7 %), `convert/cpu/neon/mono_u8/rgb_u8` (+10 %) and `rgb_u8/rgba_u8`
  (+11 %), against master `dbfb50926`, ABBA order, three rounds. x86 has no case slower beyond
  its cross-build noise.

- **darkroom harness: no wake for a preview conversion in flight.** After the sample project's
  run, with a preview still converting, a wait on `OffscreenSignals::take_wake` with no frames
  between saw no wake for 30 s, in one run of four.
