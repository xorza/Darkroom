# Open issues

- **The image cache record serializes a shared flat gain by value** (`lens/src/image/codec/mod.rs:98,169-177`):
  a gain held as `FrameGain::Shared(Arc<FlatGain>)` is written whole into each image's record and
  read back as that image's own `FrameGain::Own`, so images one flat divided no longer share it.
- **No node counts a shared flat gain's RAM** (`lens/src/image/mod.rs:221`): an image's RAM adds
  its gain through `FrameGain::own_ram_bytes`, which is zero for `FrameGain::Shared`, whose owner
  is to count it; in `lens` no value owns the gain a FITS load or a copied image shares, so the
  cache's RAM figure leaves out each such grid, 1/16 of a plane per channel.
