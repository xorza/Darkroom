# common

Cross-crate contracts and small shared utilities: typed IDs, serialization,
introspection, cancellation, file discovery/publication, the debug-build
switch, and numerical extension traits. Pure leaf crate — every other
workspace member depends on it, and it depends on nothing in-tree beyond its
own `common-derive` proc-macro.
