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

- A relayout requested by `Editor::apply_edit` is never issued.
  `apply_edit` runs from `App::handle_command`, after `Editor::frame` has
  already consumed `needs_relayout`, and the next `frame` resets the field to
  `false` at its top before anything reads it. Reached today only by
  `EditCommand::PickInputPath`, whose `SetInput` step happens to report
  `invalidates_cached_geometry() == false` because the picker chip only exists
  on a binding that is already `Const`.

- Five of the eight canvas tests that drive `GraphUI` through `UiHarness`
  (`frame/geometry`, `gesture/{breaker,connection,new_node,preview_drag}`) omit
  `graph_ui.scan_hits`, so they record a frame sequence production never
  performs and nothing reading `CanvasHits` is covered there.
