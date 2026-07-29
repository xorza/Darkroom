# Scenarium

The node-graph framework: a serializable authoring model, a compile → plan →
execute pipeline, and an asynchronous worker. It depends only on `common`
in-tree. Implementation modules are crate-private — downstream crates import
public concepts directly from `scenarium`.
