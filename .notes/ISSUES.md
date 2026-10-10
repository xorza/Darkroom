# Open issues

- `lumos` FITS metadata (`io/image/fits/metadata/mod.rs`, `read_metadata`): the real-valued keywords other than exposure and temperature (`GAIN`, `EGAIN`, `SET-TEMP`, `FOCALLEN`, `AIRMASS`, pixel sizes) accept non-finite and out-of-range values, such as a negative gain or pixel size.
