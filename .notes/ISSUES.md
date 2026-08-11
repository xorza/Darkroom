# Issues

- One test in `cargo test -p lumos --lib --tests --all-features` failed once while the machine was
  running two clippy targets and a workspace check concurrently, and has not recurred in seven
  subsequent runs. The failing name was not captured, so it is unidentified; the timing-sensitive
  candidates are the `concurrency` tests that assert bounds on how much work runs after a failure.

