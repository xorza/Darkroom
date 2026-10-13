# Open issues

- **lumos cosmic-ray rejection builds its detector anew for every frame.** `cosmic_ray::reject` builds a fresh `BayerDetector` or `MonoDetector` on each call from `MosaicPreparation::prepare`, so every light allocates the detector's scratch planes and masks again; the pipeline's `DetectorPool` keeps a `StarDetector` per frame in flight, and nothing keeps a cosmic-ray detector.
- **lumos `memory_budget` detection tests fail on macOS arm64 in most runs of the full test binary.** `detection::*_holds_the_same_heap_for_any_frame_count` reads peaks 48 bytes apart between three and four frames, in either direction; each passes when run by itself. Seen on `HEAD` in 8 of 10 runs.
