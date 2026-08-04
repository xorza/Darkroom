# Issues

- `gui/pane/graph/gesture/new_node/mod.rs` declares
  `pub(crate) mod internals { impl NewNodeUi {} }` — an empty impl in an
  otherwise empty gated module.

- A node with a disk-backed cache mode (`Disk`/`Both`) whose run resolves to a
  RAM reuse never republishes its blob: `Executor::serve_reuse` and the
  `ReuseOutcome::Served` arm of `Executor::needs_invoke` both skip the
  `RuntimeCache::store_node` call that `invoke` makes. So a blob whose write
  failed, or that was removed from the store behind the engine's back, is never
  rewritten while the value stays resident — the node reports itself cached
  every run with nothing on disk, until its digest changes and it recomputes.

- `DiskStore::store` reports nothing to its caller: an encode failure is a
  `tracing::warn!`, and a value whose custom type has no registered codec
  (`codec::error::Error::UnknownType`) is not even that. A user-initiated
  flush (the header's `↓` chip) therefore cannot tell the status bar that it
  wrote nothing.

- `core/worker.rs::tests::drop_waits_for_worker_idle_before_runtime_shutdown`
  fails intermittently under a full `cargo test` run (seen once in four),
  panicking at `worker.rs:219`; it passes every time in isolation.
