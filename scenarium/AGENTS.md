# Scenarium

Implementation modules are private: downstream crates import every public
concept from the `scenarium` root, and test scaffolding from
`scenarium::internals`.

## Layers

```
containers ← data ← identity ← runtime ← decl ← library ← graph ← exec ← worker
                                                    ↖ builtin (decl, library, runtime)
```

- **A module imports only the layers below it**, so each is read, tested and changed apart:
  `identity` names things, `decl` holds what a declaration shows (ports, funcs, value domains,
  its saved form, port maps), `library` registers declarations, and `graph` is the authored
  document over them. An architecture invariant as matklad states one, "expressed as an absence
  of something" ([ARCHITECTURE.md](https://matklad.github.io/2021/02/06/ARCHITECTURE.md.html)).
- **One arrow returns, on purpose:** an id alias names the type it identifies as the tag of
  `common::Id<T>` (`NodeId`, `FuncId` and `CompoundId` in `identity`, `CustomTypeId` in `data`),
  a `PhantomData<fn() -> T>` that reads nothing of it. Refused: a marker type per id, a second
  name for each.
