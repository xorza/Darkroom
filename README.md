# Imaginarium

An experimental image-processing pipeline crate in Rust, with SIMD (SSE/AVX/NEON) and GPU (wgpu) backends.

**Status:** early / pre-alpha. APIs change without notice. No stability guarantees.

## Layout

Single-crate project.

- `src/` — library source: `common/` (color formats, conversion, errors), `image/` (Image + file I/O), `gpu/` (wgpu wrapper, GpuImage), `processing_context/` (ProcessingContext, ImageBuffer, pipeline cache), `ops/` (blend, contrast_brightness, transform).
- `examples/` — runnable examples.
- `test_resources/` — image fixtures used by tests.

Benchmark harness lives in a sibling crate at `../quickbench/` (referenced as a `dev-dependency` by relative path).

## Build

Requires Rust **1.88+** (uses the 2024 edition; current deps like `wgpu` 29 and `image` 0.25 need 1.88). The toolchain is pinned in `rust-toolchain.toml`.

```sh
cargo check
cargo nextest run
```

## Examples

```sh
cargo run --release --example pipeline
cargo run --release --example blend
cargo run --release --example transform
```

See `examples/` for the full list.

## Benchmarks

Benchmarks are gated behind `#[ignore]` and run via:

```sh
cargo test --release <bench_name> -- --ignored --nocapture
```

## License

Dual-licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or <http://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or <http://opensource.org/licenses/MIT>)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual-licensed as above, without any additional terms or conditions.
