# Scenarium

Scenarium is a collection of tools and libraries for building node based data processing pipelines with async-capable
function execution. Written in Rust.

## Repository Layout

- **common** – shared utilities used across the workspace
- **graph** – the main graph library
- **editor** – visual editor for building and executing graphs

## Profiling with perf

When using `perf record` with DWARF call graphs, you may encounter `addr2line: could not read first record` errors during `perf report` or `perf script`. This is caused by perf's slow external addr2line invocation on debug symbols.

**Solutions:**
- Use `perf script --no-inline` to disable inline resolution (significant speedup, minor loss of detail)
- Use alternative tools like [samply](https://github.com/mstange/samply) or [hotspot](https://github.com/KDAB/hotspot) which handle symbolication better

## License

This project is licensed under the terms of the AGPL license.
See [LICENSE](LICENSE) for more information.
