# Issues noticed in passing

- `scenarium/src/execution/cache/resource/tests.rs:151` —
  `directory_identity_separates_non_utf8_names` cannot run on macOS: APFS
  rejects the `b"\xff"` filename with `EILSEQ`, so the test panics in its
  `std::fs::write` setup before reaching any assertion.

- `scenarium/src/execution/executor/mod.rs:264` — a node reached exactly as
  cancellation fires gets `prepare_node`'s `StampError::Cancelled` turned into
  `RunError::ResourceUnavailable { message: "the run was cancelled" }`, so a
  cancelled run reports one node as failed on a resource it could read.
