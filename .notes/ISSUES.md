# Open issues

- **The darkroom binary does not link on macOS in any LTO profile** —
  `release`, `bench` and `release-dev`. `ort-sys`'s CoreML objects
  (`host_utils.mm.o`, `model.mm.o`) use `@available`, so they reference
  `___isPlatformVersionAtLeast`. std defines
  it as a weak `#[rustc_std_internal_symbol]`, and LTO, fat or thin, removes
  it from the link, as no Rust code calls it; without LTO it links. Seen with
  rustc 1.99.1, 1.100.0-beta.5 and 1.101.0-nightly (2026-10-21). `cargo
drive bench` fails before any case runs.

- **One worker can hold two MRS tile scratches at once.** `NoisePlane::coefficients`
  runs `tile_coefficients`, whose `wavelet::atrous_smooth` is a parallel loop,
  while the worker holds a `TileScratch`. A worker that waits on that loop can
  take another tile and make a second scratch, which `mrs_noise::heap_bytes`,
  one scratch a worker, does not count.

- The installed cargo-drive now marks FitsReader and FitsWriter as "[shape differs by feature]" and lists no feature-gated method. So .notes/API_SURFACE.md does not record the compressed writes. The cause is the tool, not fits-well.

- **lumos's memory plan counts a compressed image's decode as two times its
  stored data unit.** `FitsHduDescription::plan` (`lumos/src/io/fits/decode/plan.rs`)
  adds `2 × padded_data_bytes` to the peak for a `CompressedImage`, a figure no
  measurement backs. A section read holds the compressed rows and heap of the
  tiles it touches and one decoded tile for each decode thread, which the figure
  does not count.
