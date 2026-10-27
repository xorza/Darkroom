# Open issues

- **imaginarium: five `atomic` NEON cases slower than before the `simd` port on an M2.**
  `contrast_brightness/cpu/neon/rgb_f32` (+19 %), `affine_warp/cpu/neon/bilinear/rgba_f32`
  (+14 %) and `rgba_u16` (+7 %), `convert/cpu/neon/mono_u8/rgb_u8` (+10 %) and `rgb_u8/rgba_u8`
  (+11 %), against master `dbfb50926`, ABBA order, three rounds. x86 has no case slower beyond
  its cross-build noise.
- **darkroom: `cargo doc` warns on `internals/harness/display.rs`.** Its module doc links
  `Display`, which the public docs do not hold (`private_intra_doc_links`).
- **darkroom harness: `the_sample_project_runs_to_its_previews` fails now and then.** In about one
  full `cargo test -p darkroom --lib` run of seven, after `ctrl+r` and a `wait_run` that returns
  `Ok`, the state reports no run for "Load Image" (`Load Image: None`). Alone, it passed every time.
