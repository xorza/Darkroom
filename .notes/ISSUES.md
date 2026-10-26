# Open issues

- **imaginarium: five `atomic` NEON cases slower than before the `simd` port on an M2.**
  `contrast_brightness/cpu/neon/rgb_f32` (+19 %), `affine_warp/cpu/neon/bilinear/rgba_f32`
  (+14 %) and `rgba_u16` (+7 %), `convert/cpu/neon/mono_u8/rgb_u8` (+10 %) and `rgb_u8/rgba_u8`
  (+11 %), against master `dbfb50926`, ABBA order, three rounds. x86 has no case slower beyond
  its cross-build noise.

- **Tests outside the platform modules branch on the platform with `#[cfg(unix)]`.** Unix-only
  behavior is tested from the callers' test files instead of behind `common::platform`: mode
  bits in `common/src/fs/tests.rs`, change time and `chmod` in
  `common/src/fs/file_identity/tests.rs` (`a_rewrite_that_restores_the_mtime_moves_the_identity`),
  non-UTF-8 names in `scenarium/src/exec/cache/stamp_job/tests.rs`
  (`directory_identity_separates_non_utf8_names`), symlinks in
  `lumos/src/io/raw/raw_files/tests.rs` (`only_raw_named_links_are_followed`), and a non-Unicode
  pick in `darkroom/src/gui/app/mod.rs` (`a_pick_that_is_not_unicode_is_refused_whole`).
