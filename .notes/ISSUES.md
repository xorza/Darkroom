# Open issues

- **The darkroom binary does not link on macOS in any LTO profile** —
  `release`, `bench` and `release-dev`. `ort-sys`'s CoreML objects
  (`host_utils.mm.o`, `model.mm.o`) use `@available`, so they reference
  `___isPlatformVersionAtLeast`. std defines
  it as a weak `#[rustc_std_internal_symbol]`, and LTO, fat or thin, removes
  it from the link, as no Rust code calls it; without LTO it links. Seen with
  rustc 1.99.1, 1.100.0-beta.5 and 1.101.0-nightly (2026-10-21). `cargo
drive bench` fails before any case runs.

- **No stage prices a calibrated light's flat gain before its warp.**
  `FrameInfo::demosaiced` keeps the demosaiced channels and the flag plane,
  and `StoredLayout::decoded` charges no gain grid, so a light a flat divided
  holds its gain's nodes beside what the prepare stage and the resident
  parked set are planned at.
