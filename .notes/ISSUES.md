# Open issues

- `lumos` FITS metadata (`io/image/fits/metadata/mod.rs`, `read_metadata`): the real-valued keywords other than exposure and temperature (`GAIN`, `EGAIN`, `SET-TEMP`, `FOCALLEN`, `AIRMASS`, pixel sizes) accept non-finite and out-of-range values, such as a negative gain or pixel size.
- `lumos` `stack()` of camera RAW paths as `LinearImage` (`combine/cache/loader/mod.rs`, `FramePeek::of_decoded`): the memory plan counts none of what LibRaw holds beside the frame during each decode — the whole file and the unpacked raw buffer — because `LinearImage` has no peek and frame 0 is decoded before the plan.
