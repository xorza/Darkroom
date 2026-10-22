# Open issues

- **darkroom uses "chrome" in two senses.** `palantir::Chrome` is a node's
  fill, border, corner radii and shadow, and darkroom passes it to every
  panel. darkroom also names its frame surfaces chrome: `ChromeColors`,
  `Palette::chrome_fill`, the `chrome_fill` key of
  `darkroom/assets/ayu-graphite.ron`, and the "chrome bands" of
  `gui/main_window/mod.rs`.
- **Two lumos doc comments name items that no longer exist.**
  `lumos/src/image_ops/color_calibration/mod.rs:9` links
  `neutralize_background::NeutralizeBackground`, and
  `lumos/src/internals/synthetic/mod.rs:17` uses `BackgroundField`.
- **A merged worker batch can fail a run-to-node for a superseded request.**
  `WorkerTask::next_intent` merges every pending message into one
  `BatchIntent`, where the last `Update` wins but the `RunNodes` seeds of every
  message accumulate. A run-to-node followed, before the worker wakes, by an
  install of a graph without that node fails the run with `NodeSeedNotFound`
  (`scenarium/src/exec/schedule/mod.rs:409`), and the editor reports it as
  a run error.

- **The release build of the darkroom binary and bench does not link on
  macOS.** `ort-sys`'s CoreML objects (`host_utils.mm.o`, `model.mm.o`)
  reference `___isPlatformVersionAtLeast`, which the link does not resolve, so
  `cargo drive bench` fails before any case runs.
