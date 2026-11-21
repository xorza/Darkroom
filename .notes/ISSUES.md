# Open issues

- **`a_settled_frame_records_without_allocating` failed on Windows CI** (run 44929076090, commit
  `56ffaac81`): "settled frame 2 performed 9 heap operations". It passed on macOS and Linux in
  the same run and on Windows at `d9108120f`. The audited frames render the status bar's
  resident-memory readout, which `ProcessMemory` refreshes once per second of the stepped UI
  clock from the live process.
- **The image cache record serializes a shared flat gain by value** (`lens/src/image/codec/mod.rs:98,169-177`):
  a gain held as `FrameGain::Shared(Arc<FlatGain>)` is written whole into each image's record and
  read back as that image's own `FrameGain::Own`, so images one flat divided no longer share it.
