# Scenarium

Scenarium is the node-graph framework: a serializable authoring model, a
compile → plan → execute pipeline, and an asynchronous worker. It depends only
on `common` in-tree. `lib.rs` is the public façade; implementation modules are
crate-private, so downstream crates import public concepts directly from
`scenarium`.
