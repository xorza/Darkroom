# Project-specific linting and coding-style enforcement

## Current state

`scripts/test-all.sh` runs Clippy with `-D warnings`. This promotes emitted
warnings to errors, but it does not enable allow-by-default policy lints.
There is currently no `[lints]` policy in either package manifest and no
`clippy.toml`.

Aperture's conventions are broader than Clippy's built-in lint set. The most
maintainable approach is layered:

1. Configure existing rustc and Clippy lints explicitly in `Cargo.toml`.
2. Enforce source- and filesystem-level house rules with a checked-in stable
   Rust tool.
3. Introduce Dylint only for rules that genuinely require compiler name or
   type resolution.

## Built-in rustc and Clippy policy

An initial package-level policy could include:

```toml
[lints.rust]
missing_debug_implementations = "deny"
unreachable_pub = "deny"

[lints.clippy]
items_after_test_module = "deny"
pub_use = "deny"
redundant_pub_crate = "deny"
```

There are important limitations:

- `missing_debug_implementations` covers publicly reachable types, not every
  private struct, and it accepts manual `Debug` implementations. It therefore
  cannot enforce Aperture's exact `#[derive(Debug)]` rule.
- `pub_use` also reports the intentional public API re-exports in `lib.rs`.
  Those items need narrow, reasoned allowances.
- Clippy's `absolute_paths` is broader than Aperture's rule against inline
  `crate::foo::bar::Type` paths in expressions and patterns. A source-AST
  rule can express the local policy more exactly.
- `multiple_inherent_impl` can prevent multiple inherent impl blocks, but
  that is stronger than merely requiring every inherent impl to live beside
  its struct.

`clippy.toml` configures existing Clippy lints, including disallowed names,
types, methods, macros, and path thresholds. It does not define new lint
logic. Lint levels belong in `Cargo.toml`; `clippy.toml` should be reserved for
configuration values needed by selected lints.

Clippy's pedantic and restriction groups should be audited rather than enabled
wholesale. Restriction lints intentionally limit language features and can
contradict one another, so only rules matching an explicit project policy
should be enabled.

### Sharing policy between packages

The root `aperture` package and `anim-derive` currently have separate
manifests. They can each carry a `[lints]` table, or the repository can become
an explicit workspace and define shared `[workspace.lints]`, with each member
opting in through:

```toml
[lints]
workspace = true
```

## Stable project-specific checker

Most Aperture rules are syntactic or filesystem policies and do not need
compiler internals. A checked-in Rust binary such as `tools/style-lints`,
using `syn` to parse source files, would provide one stable enforcement point.
It should emit ordinary `file:line:column` diagnostics, exit nonzero on a
violation, and have passing and failing fixtures for every rule.

Candidate rules include:

| Aperture convention | Enforcement |
|---|---|
| Every struct derives `Debug` | Inspect struct attributes in the source AST |
| No tuple returns | Reject direct tuple return syntax on functions and methods |
| No `#[cfg(test)]` on production functions | Inspect function attributes |
| No `pub(super)` or `pub(in ...)` | Inspect visibility syntax |
| No `foo.rs` beside `foo/` | Scan module paths on the filesystem |
| `Pod` requires `padding_struct` | Correlate derive and attribute lists |
| No inline long `crate::...` paths | Inspect expression and pattern paths, excluding imports |
| Inherent impls live with their struct | Build a cross-file declaration and impl index |
| No trivial accessors | Match method bodies that only expose or assign a field, including configured one-hop calls |

The checker should be invoked from `scripts/test-all.sh` so the normal
verification path cannot omit it. It should not run from `build.rs`: style
policy is a repository check, not a condition for downstream compilation.

### ast-grep alternative

ast-grep is a good lower-cost option for individual syntax rules. Its YAML
lint rules support structural matching, messages, severities, and fixes. It is
well suited to tuple returns, scoped visibility, attributes, and forbidden
path shapes. Module layout still needs a filesystem check, and complex
cross-file or semantic policies eventually become easier to maintain in a
small Rust checker than in a growing collection of negative YAML matches.

## Dylint for semantic rules

Clippy has a predetermined lint set and no project-local plugin interface.
Dylint runs user-supplied dynamic lint libraries through rustc's lint APIs,
making it the closest equivalent to custom Clippy rules.

It is appropriate when a rule must resolve what code means rather than only
how it is written, for example:

- determine whether a call resolves to an in-crate free function and require
  its owning module to remain namespace-qualified;
- find an inherent impl whose resolved type declaration is in another file;
- recognize trivial accessors through aliases, coercions, or generated code.

Dylint has a higher maintenance cost. Its libraries use compiler-internal
APIs, and rustc and `clippy_utils` provide no stability guarantees. A lint
library therefore needs a pinned compatible toolchain and periodic updates as
the compiler changes. It should be introduced only when the stable checker
cannot enforce a valuable rule accurately enough.

## Recommended rollout

1. Add explicit Cargo lint levels for uncontroversial built-in rules.
2. Audit existing violations before turning new policy lints into errors.
3. Add a stable `tools/style-lints` checker with the simplest objective rules:
   required `Debug` derives, forbidden scoped visibility, tuple returns,
   misplaced `cfg(test)`, module path conflicts, and the `Pod` attribute
   contract.
4. Run the checker in `scripts/test-all.sh` alongside fmt, Clippy, and tests.
5. Add rules incrementally, each with fixtures that pin both violations and
   allowed edge cases.
6. Leave intent-heavy policies such as comment quality, expected-versus-logic
   failures, test adequacy, and premature abstractions to review.
7. Consider Dylint only for the small remaining set that requires compiler
   resolution.

## References

- [Cargo lint configuration](https://doc.rust-lang.org/stable/cargo/reference/lints.html)
- [Cargo workspace lint inheritance](https://doc.rust-lang.org/stable/cargo/reference/workspaces.html#the-lints-table)
- [Clippy configuration](https://doc.rust-lang.org/stable/clippy/configuration.html)
- [Clippy lint groups](https://doc.rust-lang.org/stable/clippy/usage.html#clippyrestriction)
- [Clippy lint catalog](https://rust-lang.github.io/rust-clippy/master/index.html)
- [ast-grep lint rules](https://ast-grep.github.io/guide/project/lint-rule.html)
- [Dylint](https://github.com/trailofbits/dylint)
