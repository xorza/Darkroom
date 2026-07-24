# Simplifying the resource-stamp system

An investigation into whether digest computation can wait until all input
values are known, and the filesystem-only design implemented afterward.

## Verdict

The "delay digest until values are known" idea already exists where it is
needed: the executor's late restamp path handles a bound filesystem path whose
producer value was unavailable during resolution. Making that delay universal
would remove the resolver's backward cache cut and make reopening a document
execute or hydrate intermediates hidden behind a downstream cache hit.

The removable complexity was the unused custom `ResourceStamper` extension
point. It had no production consumer in the workspace and used pointer
identities to memoize stamps. The trait, opaque stamp type, registry plumbing,
compiled trait objects, custom request maps, and custom-only tests have been
removed. Resource preparation now handles filesystem paths only.

## Why eager preparation remains

Node digests are structural: a consumer folds its producers' digests rather
than their output values. This lets the resolver stamp the graph before
execution and sweep it in reverse. When a consumer is a cache hit, traversal
stops there and its upstream cone is cut without invoking or hydrating it.

Computing every digest at execution reach would encounter producers before
their consumers. It therefore could not know that a producer feeds only a
future cache hit. The same reverse sweep also derives exact output demand and
reader counts, so moving digest computation out of resolution would regress
both cache pruning and value lifetime planning.

## Current design

1. Flattening records `ExecutionInput::stamps_fs_path` for inputs declared as
   `DataType::FsPath`.
2. Before resolution, `RunResourceStamps` walks executable pure nodes and
   collects missing constant paths plus any readable bound path values.
3. A Tokio blocking task resolves those paths to file or directory identities.
   One `HashMap<String, FsPathId>` memoizes them for the run.
4. The producer-first digest pass folds those prepared identities without I/O.
5. If a bound path value is not current or resident, its consumer receives no
   digest and stays live. After its producer settles, the executor prepares the
   newly available path, re-stamps the consumer, and can still reuse its cache.

The request/result pipeline is now only a pending `HashSet<String>` and the
run's identity map. There is no separate prepared-stamp structure.

## Removed hazards

The custom subsystem keyed stamps by the addresses of stamper trait objects
and custom-value `Arc`s, with a source-port fallback for other values. Initial
collection could observe an old resident value before current digests were
stamped. A late restamp could then reuse that old entry through the source key
or through allocator pointer reuse. Removing pointer-keyed custom memoization
eliminates both stale-identity paths.

## Constraint

A pure function that dereferences mutable external state must receive that
reference through an `FsPath` input for the referent to participate in its
digest. A future non-filesystem resource should remain impure until a concrete
production use case justifies a smaller, value-based identity design.

`FsPathId` remains structural rather than being pre-hashed. Its representation
is simple and directly tested; adding another fixed-byte identity layer would
not simplify the current pipeline.
