# Open issues

- The comment on `[profile.mcp]` in the root `Cargo.toml` says a panic "ends the session and not
  the server"; `app/src/mcp/session_thread.rs` ends the server with the session, as
  `darkroom/AGENTS.md` ("The process is the session") states.
- No gate lane and no CI step runs the workspace's doctests: `gate-test` passes `--tests`, and
  there is no `gate-doctest` as `palantir` and `fits-well` have. `common/src/introspect/mod.rs`
  and `lumos/src/star_detection/star_detection_config/mod.rs` hold runnable examples, and `lumos`
  holds `no_run` ones that are never compiled.
