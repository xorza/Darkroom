# Open issues

- **The darkroom binary does not link on macOS in any LTO profile** —
  `release`, `bench` and `release-dev`. `ort-sys`'s CoreML objects
  (`host_utils.mm.o`, `model.mm.o`) use `@available`, so they reference
  `___isPlatformVersionAtLeast`. std defines
  it as a weak `#[rustc_std_internal_symbol]`, and LTO, fat or thin, removes
  it from the link, as no Rust code calls it; without LTO it links. Seen with
  rustc 1.99.1, 1.100.0-beta.5 and 1.101.0-nightly (2026-10-21). `cargo
drive bench` fails before any case runs.

- **"Load Astro Image" outputs an image with default metadata** — the header,
  the derived facts and the provenance of the file are lost on both paths.
  `PreviewImage::from(LinearImage)` moves the metadata out of the planes, and
  `lens/src/astro/nodes/io.rs` keeps only `PreviewPixels`.
