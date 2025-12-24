# Scenarium

Scenarium is a collection of tools and libraries for building node based data processing pipelines.
The core graph and utilities are written in Rust.

## Repository Layout

- **common** – shared utilities used across the workspace
- **graph** – the main graph library
- **deprecated_code** – older editors and experimental code
- **test_resources** – sample assets used by tests

## Maintenance

Workspace dependencies are kept at their latest minor versions (patch versions left open).
Common utilities receive periodic safety and ergonomics updates.
Shared utilities favor standard library functionality over custom helpers.
Logging uses the tracing ecosystem.
Runtime validation catches invalid bindings earlier.
Some tests run on the Tokio async runtime.
Option-based lookups are preferred with explicit `expect` at call sites.
Synchronization uses Tokio primitives; parking_lot is not used.
Async contexts use Tokio locks with awaits where possible.
Blocking helpers are removed in favor of async/await or non-blocking try_lock.

## License

This project is licensed under the terms of the MIT license.
See [LICENSE](LICENSE) for more information.
