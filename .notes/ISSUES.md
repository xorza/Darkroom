# Open issues

- **The release build of the darkroom binary and bench does not link on
  macOS.** `ort-sys`'s CoreML objects (`host_utils.mm.o`, `model.mm.o`)
  reference `___isPlatformVersionAtLeast`, which the link does not resolve, so
  `cargo drive bench` fails before any case runs.
