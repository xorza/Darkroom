# Scenarium

Scenarium is a collection of tools and libraries for building node based data processing pipelines.  
The core graph and utilities are written in Rust and exposed through Tauri+Svelte desktop editor.

## Repository Layout

- **common** – shared utilities used across the workspace
- **graph** – the main graph library
- **ScennariumEditor.Svelte+Tauri** – a Svelte + Tauri based desktop editor

## Building

This repository is organised as a Rust workspace.  The native tools require a recent Rust toolchain.

```bash
# build all Rust crates
cargo build --workspace
```

```bash
#run the Svelte+Tauri editor
cd ScennariumEditor.Svelte+Tauri && cargo tauri dev
```

## License

This project is licensed under the terms of the MIT license.
See [LICENSE](LICENSE) for more information.
