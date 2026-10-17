# Imaginarium

An image-processing library with **CPU (SIMD: SSE/AVX2/NEON) and GPU (wgpu
compute) backends**, one CPU/GPU image buffer, and 9 pixel formats. A
standalone crate, used in Darkroom by `lens`, `lumos` and `darkroom`.
Pre-alpha: rename, re-sign and rewrite callers freely; no compat shims.

- **GPU is optional**, behind the `wgpu` feature (off by default). CPU paths
  compile and pass without it.
- **Every SIMD kernel has a scalar reference** and a test comparing the two.
- **All 9 formats.** A new op or shader handles the full format set, or
  declares the narrower list it supports.
- **One rounding rule.** Narrowing into an integer sample rounds to nearest,
  ties to even, and saturates (`common::sample::Sample`); SIMD kernels and
  WGSL (`gpu/packed.wgsl`) follow it, and every GPU test compares with the
  CPU result.

## Verification

`cargo drive` runs every check: the driver is the `drive` submodule at the
root, the same repository in every project. Run it from the root.

```
cargo drive check          # after each change: the root's `check` lanes
cargo drive test [args]    # the tests, each time; the runner's arguments pick some: a test name
cargo drive bench [args]   # the benches; a filter, or --test to run each case once
cargo drive gate           # the full check, as CI runs it: the root's `gate` lanes, fmt --check
```

`check` takes no arguments: each lane covers the whole workspace, and the
workspace of each path dependency outside it, in its own folder. A step does
not run again when no input changed since it passed; under nextest, the tests
run only for the crates a change reaches and every crate that depends on them.
Each step prints one line; a failure prints each diagnostic once and names its
log, `target/drive/log/<step>.log`. Read the log rather than run cargo by
hand: another shape of a cargo command builds another copy of the crates.

A root's lanes are in its `Cargo.toml`, under `[workspace.metadata.drive]`,
or `[package.metadata.drive]` in a root that is one package. Its `check` list
runs fmt, lint, ship, tests and doc tests by default, and its `gate` list adds
rustdoc. `test-runner` is `nextest` (the default) or `cargo-test`, which needs
nothing installed but runs every test when a change reaches any crate. Every
lane runs `--locked`: after a change to the dependencies,
`cargo update --workspace` writes the lock.

The second lint set is the CPU-only leg; the first gate on
`not(feature = "wgpu")` adds `""` to the test sets. The plain clippy lints the
build that ships: every other target turns `internals` on through the self
dev-dependency. A change to bench code adds `cargo drive bench --test`, which runs
each case once. `.cargo/config.toml` builds x86-64 for x86-64-v3, the CPU
darkroom builds for.

`neon.rs` files and the aarch64 arms of the dispatch tables are aarch64-only,
so the x86 chain never compiles them. A change to one adds
`cargo clippy --target aarch64-unknown-linux-gnu --all-targets --all-features -- -D warnings`
(after `rustup target add aarch64-unknown-linux-gnu`; clippy does not link).
Running the NEON tests needs `qemu-user` and a runner in
`.cargo/config.toml`; without them, say the kernel was only compile-checked.

## Test code

`internals` is the one feature for what only tests and benches build; the
crate's dev-dependency on itself turns it on in every test and bench build.
Code a bench reaches is gated `#[cfg(feature = "internals")]`; a reach-in only
the crate's own tests call is gated `#[cfg(test)]`. Never
`any(test, feature = "internals")`.

## Benches

One target runs every case through `bench::run`; each driver sits in a
`bench.rs` beside the code it measures, and `src/bench/mod.rs` states the
rules. `cargo drive bench` runs them with fixed features (`wgpu`), so a case
always measures the same build. An id is `<kind>/<group>/<backend>/<rest…>`:
an `atomic` case times one kernel on the unit dispatch hands it, for the
scalar reference and each tier the CPU has; an `integration` case times a
public entry point on the 24 MP standard frame, on each backend. The backend
comes first in every case, so `'/cpu/'` opens no device. A filter selects by
id: `cargo drive bench --list`, `'^atomic/convert/'`,
`--exact integration/preview/cpu/rgb_f32`.
