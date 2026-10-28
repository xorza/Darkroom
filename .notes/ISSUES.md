# Open issues
- **Real-data tests need a file the dataset lacks.** Five `lumos` tests under `real-data` (stretch, denoise, extract_bg, color_calibration, `milky_way_best_pipeline`) assert on `lumos/test_data/lumos_data/stacked_light.tiff`, which the bundle `lumos/scripts/fetch-test-data.sh` unpacks does not hold.
- **`lumos/snapshots/raw_cfa.ron` does not parse.** `internals::characterization::raw_decode_snapshot` fails with "Expected opening `(` for struct `Snapshot`" at 2:100.
- **`detect_rho_opiuchi` finds a star with zero FWHM.** `star_detection::tests::real_data::detect_rho_opiuchi` fails on a star at (4563.9, 1313.0) with `fwhm: 0.0`.
