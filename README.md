# Scenarium

Scenarium is a collection of tools and libraries for building node based data processing pipelines.
The core graph and utilities are written in Rust.

## Repository Layout

- **common** – shared utilities used across the workspace
- **graph** – the main graph library

## Building

This repository is organised as a Rust workspace.  The native tools require a recent Rust toolchain.

```bash
# build all Rust crates
cargo build --workspace
```

## License

This project is licensed under the terms of the MIT license.
See [LICENSE](LICENSE) for more information.
