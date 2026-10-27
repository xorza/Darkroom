# Open issues

- **darkroom: the lib test binary crashed with SIGSEGV on the ubuntu runner.** CI run 40434805558
  (master `4b3e7399b`, lavapipe) ended `cargo test -p darkroom --lib` with "signal: 11, SIGSEGV"
  before any test reported; the next run, 40437898986, did not crash. Locally it never crashed.
