# Issues noticed in passing

Ordered by impact: wrong behaviour first, then structure, then cosmetics.

- `scenarium/src/execution/executor/mod.rs:408` — `needs_invoke` maps every
  `StampError` from `restamp_and_hydrate` to `RunError::ResourceUnavailable`,
  including `StampError::Cancelled`, so a node reached exactly as cancellation
  fires is reported as having failed on a resource it could read, with the
  message "the run was cancelled".

- `lumos/src/io/image/` ↔ `lumos/src/stacking/` are mutually dependent: `io`
  implements the `StackableImage` trait declared at
  `stacking/frame_store/mod.rs:126` (`linear.rs:18`, `cfa.rs:22`, and two test
  modules), while `stacking/product.rs` and `stacking/frame_store/mod.rs` import
  `io::image`'s `LinearImage`, `LinearPixels`, `LoadContext`, and `ImageError`.

- `scenarium/src/execution/engine/mod.rs:99` — `outcome.clear()` runs here and
  again at the top of `Executor::run` (`executor/mod.rs:131`).

- `palantir` has 52 unresolved intra-doc links across the crate
  (`cargo doc -p palantir --no-deps --document-private-items`), naming types that
  moved or were renamed — `Ui`, `InputState`, `crate::scene::cascade::Cascade`,
  and others.
