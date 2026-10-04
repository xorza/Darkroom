# Open issues

- **The detector's saturation level ignores a pedestal** — `star_detection/detector/stages/prepared_frame/mod.rs` (`saturation_level`): without decoder flags a pixel is saturated at 0.95 of `DATAMAX` (or of 1), whatever pedestal the samples carry. On a frame whose pedestal is large against its span, the level falls under the pedestal and every pixel reads saturated.
- **The memory-budget test runs past 1 s** — `star_detection/tests/mem_budget.rs` (`buffer_working_set_stays_flat_in_frame_count`): 2.7 s in the debug build, most of it in the `high_resolution` preset's Gaussian fits and the `crowded_field` preset's deblending.
