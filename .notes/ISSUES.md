# Issues noticed in passing

- `scenarium/src/execution/executor/mod.rs:264` — a node reached exactly as
  cancellation fires gets `prepare_node`'s `StampError::Cancelled` turned into
  `RunError::ResourceUnavailable { message: "the run was cancelled" }`, so a
  cancelled run reports one node as failed on a resource it could read.

- `scenarium/src/execution/engine/mod.rs:46` — the `plan` field carries a
  leftover doc comment describing per-run filesystem identities (a field that
  no longer exists there) stacked above its own one-line doc.

- `scenarium/src/execution/error.rs:27` — `Error::EventLambdaPanic` is
  constructed only in `worker/task.rs:316`, while its type is documented as the
  error of the engine's `Result`-returning entry points.

- `scenarium/src/execution/engine/mod.rs:97` — `outcome.clear()` runs here and
  again at the top of `Executor::run`.

- `imaginarium/Cargo.toml:36` — `aligned-vec` is declared but never referenced
  anywhere in the crate; `imaginarium/AGENTS.md:91` still documents it as
  providing `AVec` 16-byte alignment.

- `darkroom/src/gui/theme.rs:302` — the inherent `impl ThemeChoice` lives in
  `gui::theme` while the type is declared in `core::theme_pref`, making `core`
  and `gui` mutually dependent.

- `lumos/src/io/image/linear.rs:18` — the I/O-layer `linear` module imports
  `StackableImage` from `stacking::frame_store`, inverting the io → stacking
  layering.
