# Open issues

- **The darkroom binary does not link on macOS in any LTO profile** —
  `release`, `bench` and `release-dev`. `ort-sys`'s CoreML objects
  (`host_utils.mm.o`, `model.mm.o`) use `@available`, so they reference
  `___isPlatformVersionAtLeast`. std defines
  it as a weak `#[rustc_std_internal_symbol]`, and LTO, fat or thin, removes
  it from the link, as no Rust code calls it; without LTO it links. Seen with
  rustc 1.99.1, 1.100.0-beta.5 and 1.101.0-nightly (2026-10-21). `cargo
drive bench` fails before any case runs.

- **lumos's memory plan counts a compressed image's decode as two times its
  stored data unit.** `FitsHduDescription::plan` (`lumos/src/io/fits/decode/plan.rs`)
  adds `2 × padded_data_bytes` to the peak for a `CompressedImage`, a figure no
  measurement backs. A section read holds the compressed rows and heap of the
  tiles it touches and one decoded tile for each decode thread, which the figure
  does not count.
