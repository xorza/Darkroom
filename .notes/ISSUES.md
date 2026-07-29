# Issues noticed in passing

- `scenarium/src/execution/cache/resource/tests.rs:151` —
  `directory_identity_separates_non_utf8_names` cannot run on macOS: APFS
  rejects the `b"\xff"` filename with `EILSEQ`, so the test panics in its
  `std::fs::write` setup before reaching any assertion.

- `scenarium/src/execution/executor/mod.rs:264` — a node reached exactly as
  cancellation fires gets `prepare_node`'s `StampError::Cancelled` turned into
  `RunError::ResourceUnavailable { message: "the run was cancelled" }`, so a
  cancelled run reports one node as failed on a resource it could read.

- `scenarium/src/execution/engine/mod.rs:46` — the `plan` field carries a
  leftover doc comment describing per-run filesystem identities (a field that
  no longer exists there) stacked above its own one-line doc.

- `scenarium/src/execution/cache/digest/mod.rs:7` — the module doc links
  `[node_digest]`, which lives in `cache::resource`, not this module; the same
  broken link is at `cache/slot.rs:76` as `[digest::node_digest]`.

- `scenarium/src/execution/error.rs:27` — `Error::EventLambdaPanic` is
  constructed only in `worker/task.rs:316`, while its type is documented as the
  error of the engine's `Result`-returning entry points.

- `scenarium/src/execution/engine/mod.rs:97` — `outcome.clear()` runs here and
  again at the top of `Executor::run`.
