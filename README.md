# Scenarium

Scenarium is a collection of tools and libraries for building node based applications.  
The core graph and utilities are written in Rust and exposed through various front end implementations.

## Repository Layout

- **common** – shared utilities used across the workspace
- **graph** – the main graph library
- **cs_interop** – Rust FFI bindings consumed by the .NET editor
- **ScenariumEditor.NET** – Windows editor built with WPF
- **ScenariumEditor.QML** – Qt Quick (C++) editor

## Building

This repository is organised as a Rust workspace.  The native tools require a recent Rust toolchain and, for the QML editor, a Qt6 installation.

```bash
# build all Rust crates
cargo build --workspace
```

Each front end can be built using its respective build system.  The .NET editor requires the .NET SDK, while the QML editor uses CMake with Qt6.

## License

This project is licensed under the terms of the MIT license.  See [LICENSE](LICENSE) for more information.
