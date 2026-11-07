# CI proposal

This is a temporary proposal. No workflow changed for it. When a change lands, its lasting decision
moves to the `AGENTS.md` or the workflow comment that it governs. Then delete the change from this
file.

**Goal:** the same lane on every OS and in every repo, and a failure that says what broke.

## What differs now

| Item | Darkroom | palantir | fits-well |
|---|---|---|---|
| Gate | Cargo aliases | Cargo aliases | `cargo drive gate --ci` |
| Test runner | `cargo test` | `cargo test` | nextest, through drive |
| Toolchain install | `rustup` step | `dtolnay/rust-toolchain@stable` | `rustup` step |
| Action pins | Commit SHA | Tag (`@v7`, `@v2`) | Commit SHA |
| `persist-credentials: false` | Yes | No | Yes |
| Default shell | `bash` on every OS | The OS's own | The OS's own |
| `RUSTFLAGS` | None, keeps x86-64-v3 | `-D warnings`, which drops x86-64-v3 | None, keeps x86-64-v3 |
| Doc warnings | In the alias | `RUSTDOCFLAGS` in CI | Through drive |
| Job timeout | 120 min | 150 min | 60 min |
| Crash capture | Ubuntu: core, `ulimit` in the step, `gdb` installed first | Ubuntu: core, `BASH_ENV`, `gdb` installed on failure | None |
| Build apart from run | Yes (`--no-run`) | No | No |
| `drive` version | Not used | master, not pinned | Pinned by `DRIVE_REV` |
| Lavapipe install | Copy of the Windows step | Copy of the Windows step | Not needed |
| Failure artifacts | None | None | None |

**Correction:** my last change added a second crash capture to palantir, because I did not read
Darkroom's first. Darkroom's already worked on Ubuntu. This proposal replaces both with one.

## What a failure tells you now

- **A crash on Ubuntu:** a stack, with the limits above.
- **A crash on macOS or Windows:** an exit code alone. Windows shows `0xc0000374` and no stack.
- **A failed test:** the libtest text, in a log of up to 12,000 lines.
- **A failed golden test:** the files in `output/` are lost with the runner.
- **A hang:** the job timeout kills it after up to 150 min, with no stack.
- **Which adapter and driver ran:** palantir's Linux step and both Windows steps print `vulkaninfo`.
  Darkroom's Linux step and macOS print nothing.

## Proposed shape

One set of shared steps, and each workflow reads the same on every OS, with no `if` on the OS outside
the shared steps:

```yaml
defaults:
  run:
    shell: bash
steps:
  - checkout (pinned, persist-credentials false)
  - setup-rust        # toolchain, cache, prints versions
  - install-lavapipe  # Linux and Windows; macOS has Metal, so a no-op. Prints the adapter.
  - crash-capture     # arms the OS's route, and proves it with one crash
  - the gate, one step per alias, each past an earlier failure, tests through `test`
  - crash-report      # if: failure(). A stack per crash, in the log and the job summary
  - upload-artifact   # if: failure(). test_output and golden output, 7 days
```

### Crash capture on each OS

| OS | Capture | Stack | Evidence |
|---|---|---|---|
| Linux | `core_pattern`, core limit through `BASH_ENV` | `gdb` | [ci-core-dumper](https://github.com/mdavidsaver/ci-core-dumper), [cirunner](https://github.com/pjsip/cirunner) |
| macOS | CrashReporter `.ips` in `~/Library/Logs/DiagnosticReports` | Read from the report's JSON | ci-core-dumper. A local run gave a report with the Rust frames, with no sudo and no entitlement |
| Windows | [WER `LocalDumps`](https://learn.microsoft.com/windows/win32/wer/collecting-user-mode-dumps) | `cdb`, which cirunner says the runner holds | Not run yet |

macOS gets no core file. In the same local run `ulimit -c unlimited` wrote nothing to `/cores`, and
[sources](https://nasa.github.io/trick/howto_guides/How-to-dump-core-file-on-MacOS) say that a binary
needs the `get-task-allow` entitlement for one.

**Self-test on every OS:** `crash-capture` compiles one small Rust program that writes through a null
pointer, runs it, and requires a stack with the program's own frame in it. The step fails if it finds
none. So each run proves the whole route, and "no crash report" then means no crash. Nothing in the
sources does this. It costs some seconds a lane.

### Changes that need no decision

- **One shell:** `defaults.run.shell: bash` in all three repos.
- **One toolchain step:** `rustup`, in place of `dtolnay/rust-toolchain`, as Darkroom and fits-well do.
- **Pins:** commit SHA and `persist-credentials: false` in palantir, and `DRIVE_REV` in its surface job.
- **One timeout rule:** a job limit, and a step limit on each test step, so that a hang ends early.
- **Build apart from run:** `gate-test --no-run` in each repo, so each shows its own time.
- **palantir's flags:** remove the `RUSTFLAGS` env, so CI builds x86-64-v3 as Darkroom ships it. Move
  `-D warnings` for docs into the alias, as Darkroom does, so the local gate equals CI.
- **Downloads:** `curl --fail`, so a 404 stops at the download, not in `7z`, and a SHA-256 check of
  the two Windows archives. wgpu has only the retry, so this goes past its practice.
- **Apt:** `-o Acquire::Retries=3`.
- **More data:** `RUST_BACKTRACE: full` and `LVP_POISON_MEMORY: true`, as
  [wgpu's CI](https://github.com/gfx-rs/wgpu/blob/trunk/.github/workflows/ci.yml) sets. The second can
  show new failures, as it poisons lavapipe's memory.
- **Failed test names:** the job summary lists each `FAILED` test and each crash, so a person needs
  not read the log.
- **Artifacts:** upload on failure, as wgpu uploads `*-actual.png` and `*-difference.png`.

## Decisions

All decided. Each moves to the `AGENTS.md` or the workflow comment it governs when it lands.

- **The shared steps live in `xorza/rust-ci`**, a new repo of composite actions, which each repo pins
  by one SHA, and whose own CI runs the self-test on three OSes, as
  [rust-lang/simpleinfra](https://github.com/rust-lang/simpleinfra) holds shared actions. Refused:
  actions in palantir, as fits-well's copy would drift; a copy in each repo, the present problem.
- **fits-well's gate moves to cargo aliases**, the shape palantir and Darkroom took in `2ccaff05b`
  and `6764525fd`. Refused: staying on `cargo drive gate --ci`, which leaves one repo different.
- **`cargo test` everywhere.** Tests keep the shared GPU device, and the crash stack names the test
  by its frames, and by its thread name, which Linux cuts to 15 characters. Refused: nextest, which `palantir/src/gpu/test_gpu.rs` already refuses at 8 s against
  2 s, and which can hide a crash that needs two tests in one process; nextest in CI alone, as CI and
  the local gate would then differ. Cost: no JUnit, and no per-test timeout.
- **`rust-toolchain.toml` in each repo**, so CI and local builds use one compiler and a bump is a
  commit. Refused: `stable`, which can turn master red with no commit that caused it; a pin in
  `xorza/rust-ci` alone, as a local gate could then pass where CI fails. Cost: a bump in three repos.
- **A push cancels the older run on master too**, as the workflows state today. Refused: cancelling
  pull request runs alone, as [wgpu](https://github.com/gfx-rs/wgpu/blob/trunk/.github/workflows/ci.yml)
  does, for its CI minutes. Cost: a cancelled commit has no result, and a rare crash has fewer chances
  to show: 38 and 40 of the last 100 runs of palantir and Darkroom.
- **`actionlint`, `zizmor` and Dependabot.** A shared action runs the first two on each repo's
  workflows, and Dependabot keeps action SHAs current. Refused: no checks, as a workflow error then
  shows only on a run. Cost: Dependabot pull requests, and findings to fix or waive.

## Order of work

1. Get `xorza/rust-ci` green on three OSes. The repo exists, public, as another repo can use only a
   public repo's actions by default.
2. Move palantir, the repo with the open crash, and keep the old route until one run proves the new.
3. Move Darkroom.
4. Move fits-well, with its cargo aliases.
5. Add `rust-toolchain.toml` to each repo, at one version.
