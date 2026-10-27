# Open issues

- **imaginarium: five `atomic` NEON cases slower than before the `simd` port on an M2.**
  `contrast_brightness/cpu/neon/rgb_f32` (+19 %), `affine_warp/cpu/neon/bilinear/rgba_f32`
  (+14 %) and `rgba_u16` (+7 %), `convert/cpu/neon/mono_u8/rgb_u8` (+10 %) and `rgb_u8/rgba_u8`
  (+11 %), against master `dbfb50926`, ABBA order, three rounds. x86 has no case slower beyond
  its cross-build noise.
- **darkroom: the lib test binary crashed with SIGSEGV on the ubuntu runner.** CI run 40434805558
  (master `4b3e7399b`, lavapipe) ended `cargo test -p darkroom --lib` with "signal: 11, SIGSEGV"
  before any test reported; the next run, 40437898986, did not crash. Locally it never crashed.
- **fits-well: `run_of_views_on_four_workers_peaks_within_its_view_bytes` fails on the macOS
  runner.** CI run 40453392481 (`ec5ca0c40`, macos-26), `tests/support/peak.rs:184`: "one
  RICE_1 i16 from stream, order 0: held 60218 B at its peak, past its 60074 B".
- **darkroom: a breaker or tie stroke through one of a wire's sample points does not cross it.**
  `segments_intersect` in `gesture/breaker_ui/scribble/mod.rs` counts proper crossings only, so a
  stroke that passes exactly through the point two sampled chords share touches both and crosses
  neither: a vertical stroke at the middle of a level wire between two ports leaves it uncut.
- **lumos: the private docs do not build with `-D warnings`.** `RUSTDOCFLAGS="-D warnings" cargo
  doc -p lumos --no-deps --document-private-items --features internals` stops at an unresolved
  link to `combine` (`combine/stack/mod.rs:1`), one to `crate::MAX_SIDE` (`image/error.rs:39`), and
  three links to `write`, both a function and a macro (`io/fits/cfa/mod.rs:60`,
  `io/fits/linear.rs:63`, `stack_product/fits/mod.rs:74`).
