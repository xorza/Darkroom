# Open issues

- **`darkroom/assets/ayu-graphite.ron` is not what its generator writes.**
  `ayu-graphite/darkroom/build.py` emits a `connection_broken` key and an
  `output_port` key and spells "colour" in the header; the asset has
  `wire_broken`, no `output_port`, and "color", though its header says it is
  generated and not edited by hand. `make install` there copies the generated
  file over the asset (`ayu-graphite/install.sh:67`).
- **The release build of the darkroom binary and bench does not link on
  macOS.** `ort-sys`'s CoreML objects (`host_utils.mm.o`, `model.mm.o`)
  reference `___isPlatformVersionAtLeast`, which the link does not resolve, so
  `cargo drive bench` fails before any case runs.
