# Issues

- Broken intra-doc links in darkroom: `[`InputScope`]` and `[`OutputScope`]`
  in `gui/graph_ctx/mod.rs`'s module doc, `[`CanvasHits`]` and
  `[`CanvasHits::scan`]` in `gui/node/prepass.rs`'s module doc, and
  `[`CullRegion::keeps_wire`]` in `gui/canvas/wire/mod.rs` — each names a type
  the file does not import, so `cargo doc` emits `unresolved link`.
