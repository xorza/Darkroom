# Open issues

- **The image cache record serializes a shared flat gain by value** (`lens/src/image/codec/mod.rs:98,169-177`):
  a gain held as `FrameGain::Shared(Arc<FlatGain>)` is written whole into each image's record and
  read back as that image's own `FrameGain::Own`, so images one flat divided no longer share it.
