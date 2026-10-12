# Open issues

## imaginarium: the README names a quickbench dev-dependency that does not exist

`imaginarium/README.md` says the benchmark harness lives in `../quickbench/` as a
`dev-dependency`. `imaginarium/Cargo.toml` has no such dependency, and darkroom no longer has a
`quickbench` submodule.
