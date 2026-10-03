# Open issues

- **The detector's saturation level ignores a pedestal** — `star_detection/detector/stages/prepared_frame/mod.rs` (`saturation_level`): without decoder flags a pixel is saturated at 0.95 of `DATAMAX` (or of 1), whatever pedestal the samples carry. On a frame whose pedestal is large against its span, the level falls under the pedestal and every pixel reads saturated.
