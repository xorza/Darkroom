# Workspace review

> **Delete an item when you address it.** This file lists open findings only: no "done"
> markers, no history. When a group has no items left, delete its heading too.

Scope: production code of `common`, `quickbench`, `fits-well`, `imaginarium`, `scenarium`,
`lumos`, `lens` and `darkroom` (`palantir` is out of scope). Tests and the APIs they use were
not reviewed. Findings already listed in `lumos/.notes/*.md` are not repeated here.

Items are anchored to file paths and symbol names; line numbers, where given, go stale fast.
Groups are named after their shared root cause and ordered by severity, then benefit.
"Probe" means the claim was confirmed by running code against a scratch copy.

---

# High — wrong results, data loss, crashes on user data

# Medium — wrong in edge cases, duplicated truths, hot-path waste

# Low — duplication, dead surface, local simplifications

## Style rules not applied
Severity: Low — breaches of the standing Rust rules.

- [ ] Missing `const fn` on trivial accessors and constructors across the workspace: `lumos` `fwhm_to_sigma`, `sigma_to_fwhm`, `mad_to_sigma`, `mad_floored`, `MedianMad::sigma`, `Size2us::pixel_count`, `DMat3::{mul_mat, determinant}`, `cubic_spline_eval`, `TileStats::get`, `TileD2y::{get, get_mut}`, `stretching::mtf`, `wavelet::{reflect, max_scales}`, `QualityPlanes::resolve`, `PixelCoverage::new/contributes`, `SigmaBounds::symmetric/asymmetric`, `SmallN::none/median_below`, rejection `*Config::new`, `FrameSpill::new`, `CombinedSample::value_only`, `MasterRole::extname/prepared`, `CalibrationComponent::extname`, `ImageDimensions::{size, width, height, channels, is_grayscale, is_rgb}`, `BitPix::is_integer`, `CfaPattern::{flip_vertical, flip_horizontal, at_raw_origin, color_at}`, `CfaType::num_colors`, `SensorLayout::cropped`, `XTransPattern::color_at`, `libraw_filter_color`; `darkroom` has no `const fn` at all (`fmt_elapsed`, `fmt_bytes`, `PortTheme::radius`, `CardTheme::border_width_total`, `GestureSlot::{get, is_idle}`, `WireTint::{new, flat}`, `Chip::new`, `Badge::{control, action, marker}`).
