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

- `core/edit/intent/mod.rs` and `intent/types.rs` document a `DockStep` type,
  a `build_doc_step` fn, and a `from`/`to` snapshot diff for dock ops, none of
  which exist; `core/document/dock/mod.rs`'s module doc repeats the same
  `DockStep` claim. The same two doc comments also list duplicated names —
  "`GraphIntent` / `UndoStep` / `UndoStep` / `GestureKey`" and "`commit_intent`
  / `commit_intent`".

- Five of the eight canvas tests that drive `GraphUI` through `UiHarness`
  (`frame/geometry`, `gesture/{breaker,connection,new_node,preview_drag}`) omit
  `graph_ui.scan_hits`, so they record a frame sequence production never
  performs and nothing reading `CanvasHits` is covered there.
