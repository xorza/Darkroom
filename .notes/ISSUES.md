# Issues

- Broken intra-doc links in darkroom: `[`InputScope`]` and `[`OutputScope`]`
  in `gui/graph_ctx/mod.rs`'s module doc, `[`CanvasHits`]` and
  `[`CanvasHits::scan`]` in `gui/node/prepass.rs`'s module doc, and
  `[`CullRegion::keeps_wire`]` in `gui/canvas/wire/mod.rs` — each names a type
  the file does not import, so `cargo doc` emits `unresolved link`.

- `gui/theme/mod.rs::tests::ayu_graphite_asset_in_sync` writes
  `assets/ayu-graphite.toml` instead of comparing against it, so the test
  cannot fail and pins nothing; it also uses a CWD-relative path and mutates a
  tracked file as a side effect of `cargo test`.

- `gui/pane/graph/gesture/new_node/mod.rs` declares
  `pub(crate) mod internals { impl NewNodeUi {} }` — an empty impl in an
  otherwise empty gated module.

- `core/worker.rs::tests::drop_waits_for_worker_idle_before_runtime_shutdown`
  fails intermittently under a full `cargo test` run (seen once in four),
  panicking at `worker.rs:219`; it passes every time in isolation.
