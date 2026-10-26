# Proposal: one heavy build at a time, and no heavy build around drive

A temporary proposal for work in `drive` and in the agent setup. When it lands, each decision
moves to the doc comment of the code it governs, to drive's README, to the `optimize-perf` skill
or to `~/.claude/CLAUDE.md`, and this file is deleted.

## Problem

Two fat-LTO builds at once fill this machine's 27 GiB. zram keeps its pages in RAM, and
systemd-oomd kills only at 90 % of 55 GiB swap or at 80 % pressure for 30 s, so the machine
freezes before anything is killed. Evidence, read on 2026-10-26:

- The transcripts hold about 250 `cargo drive bench` calls and about 260 raw
  `cargo bench --no-run` calls. A limit inside drive alone sees half the builds.
- Agents start LTO builds in parallel: fits-well session `88606a66` ran base and cand
  `cargo bench --no-run` with `&` and `wait`, two times.
- `cargo drive bench <filter>` links all three bench binaries of this workspace each time, about
  200–250 s for 3 units. One full bench build here peaked at 11.4 GiB of system-wide used RAM.
- The crashes themselves are not in the logs: journald starts on 2026-10-23 and its four reboots
  were a manual `sudo reboot`, and drive writes a run's journal line only when the run ends.

## Decisions

- **Heavy steps hold one lock per user and machine.** A step that builds under a profile other
  than `dev` and `test` runs only while it holds the lock: `bench`, `package`'s release build, and
  a project command step whose cargo arguments run `bench` or name `--release`, `-r` or
  `--profile`. Refused: a RAM cap per run (cgroup, Job Object), which kills a build that would
  have fit after the other and needs code per OS. The cap is the safety net outside drive, not
  its scheduler. Refused for now: admission by measured peak, as campfire's `ram-gate.sh` does,
  because drive measures no peak yet.
- **Dev-profile steps stay outside the lock.** `check`, `test` and `gate`'s dev build of the
  benches do not wait ten minutes behind a bench run. Their peak is not measured; the parallel
  builds in the transcripts were LTO builds.
- **The profile name says heavy, not its LTO setting.** Drive does not read `[profile.*]`, and a
  custom profile can inherit `release`. A lock on `release-dev` costs a wait. A missed LTO build
  costs a freeze.
- **The lock is `std::fs::File::lock` on `build.lock` in the journal's folder.** It is `flock`
  on Unix and `LockFileEx` on Windows, in std since Rust 1.89. The OS frees it when the holder
  dies, also by `SIGKILL` or the OOM killer, so no lock is stale. The folder is
  `platform::journal_dir()`, so no new platform code. Without that folder the run fails, and does
  not build unguarded. Refused: a PID file, which a crash leaves stale and whose check and write
  race. Refused: a file in `/tmp`, which other users share and can plant.
- **The lock covers the whole step, the bench run too.** `cargo bench` builds, then runs. While
  it measures, no other heavy build starts, as the skill already asks. Cost: a `package` waits
  for a long bench run.
- **A waiting run names the holder.** The holder writes its PID, its command and its root into
  the lock file. A waiter prints `waiting for the build lock: pid …, cargo drive bench, <root>`
  once, then blocks, and the journal records each step's wait. An agent then sees a wait, not a
  hang.
- **A child of the holder does not lock again.** The holder sets `DRIVE_BUILD_LOCK` for its
  children, and a drive that sees it does not take the lock. Without it,
  `cargo drive lock -- cargo drive bench` deadlocks, because two opens of one file conflict also
  in one process tree. Pass keys hold only `RUST*` and `CARGO*` variables, so it changes no pass.
  A shell that exports it by hand skips the lock, so the hook refuses a command that names it.
- **`cargo drive lock -- <program> [args]` runs any command under the lock.** This is for the
  builds drive does not plan (a frame-pointer `RUSTFLAGS` build, a root without a drive table such
  as `rhai`) and for a bench binary's measured run. It needs no root and no drive table. Refused:
  pass-through flags on `drive bench`, which grow by one flag for each new need.
- **`cargo drive bench --no-run` builds and prints each bench binary.** It prints one line per
  target, name and path, from cargo's `compiler-artifact` messages. `--out <dir>` copies each
  binary to `<dir>/<target>`, because the next build writes to the same path: the hash in the
  file name does not change with the source. This is the build that `optimize-perf` needs, so the
  skill has no reason to call cargo itself, and the `^no-case$` filter trick goes away.
- **`--bench <name>` picks the targets, and the package set stays.** The `-p` list keeps every
  member with a bench, so features unify as before and a case measures the same code. Only the
  linked targets change: one fat-LTO link instead of three. Check before it lands: the binary's
  file-name hash, which covers its features and profile, is the same under `--bench imaginarium`
  and `--bench '*'`. If it differs, this decision falls. Refused: `-p` per target, which unifies
  features differently and so builds different code.
- **A user-level hook refuses heavy cargo outside drive.** A `PreToolUse` hook on Bash, for every
  repository, denies a command where `cargo` runs `bench` or names `--release`, `-r` or a
  `--profile` other than `dev` and `test`, unless that invocation is `cargo drive …` or follows
  `cargo drive lock --`. It also denies a command that names `DRIVE_BUILD_LOCK`. Its message names
  the drive command to use. It also reads inside quotes, so `ssh host 'cargo bench'` is denied and
  `ssh host 'cargo drive lock -- cargo bench'` is not. It errs to denial: a false hit costs one
  retry, a miss costs a freeze. Refused: rewriting the command through `updatedInput`, because the
  agent must see what runs and shell text is fragile to rewrite. It guards against forgetting and
  is not a sandbox: a script that runs cargo inside passes.

## Plan

The code read for this plan is `~/Projects/drive` at `2c6fd5a`. `origin` is at `dcb1db7`, and
the installed driver is 0.2.5 from `914ee3f`. Pull first, and run drive's own gate after each
batch.

1. **drive: the lock.**
   - `src/build_lock.rs`: `BuildLock`, which owns the open file, takes the lock, writes the
     holder line and reads it for a waiter.
   - `Step` gains `weight: Weight` (`Light`, `Heavy`). `Step::new` sets `Light`, and the planner
     sets `Heavy` in `bench` and in `package`'s build. Project command steps take it from
     `Weight::of_cargo(&argv)`, the one rule that the planner and the tests share.
   - `Runner::execute` takes the lock before it starts a heavy step, unless `DRIVE_BUILD_LOCK` is
     set. It sets that variable on the child and keeps the lock until the child ends. The wait
     goes in `Outcome` and the journal as `waited_seconds`.
   - `error.rs`: one variant for no journal folder, one for a failed open or lock.
   - Tests: a `Weight::of_cargo` table (`bench`, `build --release`, `run -r`,
     `--profile release-dev`, `--profile=bench` are heavy; `test --benches`, `build --benches`,
     `clippy --all-targets` are light). Two `BuildLock`s conflict, and the second gets it when the
     first drops. A set variable skips the lock. The planner's bench and package steps are heavy,
     and every other step is light.
2. **drive: `cargo drive lock -- <program> [args]`.**
   - `Task::Lock`, run in `main` before any root loads, the program's exit code passed on, one
     journal line.
   - Tests: the parse, the exit codes of `true` and `false`, and no deadlock under a holder.
3. **drive: `bench --no-run`, `--bench <name>`, `--out <dir>`.**
   - The CLI options come before the criterion arguments. `--out` requires `--no-run`. A
     `--no-run` with criterion arguments is an error. `Task::Bench` holds a struct, and build-only
     against run is an enum, not a `bool`.
   - `Planner::bench`: `--bench <name>` for each target picked, else `EVERY_BENCH`, with the same
     `-p` list. A name that no member has fails with the list of bench targets. `--no-run` makes
     a `Kind::Check` step with `JSON`, so its diagnostics are read.
   - `capture.rs`: `Event::CompilerArtifact` collects each bench target's `executable` into the
     report. The outcome prints `name path` for each, and copies them when `--out` is given.
   - Tests: the parse, the exact argv of `bench`, `bench --bench a --bench b` and
     `bench --no-run`, a fixture of artifact lines, and the unknown target error.
   - On darkroom, compare the binary's file-name hash under `--bench imaginarium` and
     `--bench '*'`.
4. **Docs and versions.**
   - drive's README `Use` block gets `bench --no-run [--bench NAME] [--out DIR]` and
     `lock -- CMD`. Each decision above goes to the doc comment of its code.
   - Release drive 0.2.6, and raise `version` to `0.2.6` in the tables of darkroom, palantir,
     fits-well and campfire. An older driver then refuses, and does not hand `--no-run` to
     criterion.
   - darkroom `AGENTS.md`: the two new commands in the Verification block, and one line in
     Benches: a bench or release build waits for any other one on the machine.
5. **Agent side.**
   - `~/.claude/hooks/heavy-cargo.py` and its `PreToolUse` entry in `~/.claude/settings.json`,
     matcher `Bash`. `--self-test` runs a table of commands with the verdict expected for each.
   - `optimize-perf` Builds bullet: build with
     `cargo drive bench --no-run --bench <target> --out bins/<label>` in the background, measure
     with `cargo drive lock -- taskset -c <core> <binary> …`, and put any other heavy build behind
     `cargo drive lock --`. Remove "start another build only when the peaks fit", because the lock
     now runs one at a time. Keep "measure the first build's peak, and lower `-j` if one build
     alone does not fit".
   - `~/.claude/CLAUDE.md` Benching, "A bench build fits in RAM": the same change.
6. **End-to-end check on this machine.**
   - `cargo drive bench --no-run` in two worktrees at once: the second prints the holder and
     starts when the first ends, and its journal line shows the wait.
   - `cargo drive lock -- cargo drive bench --no-run` ends without a deadlock.
   - `kill -9` on the holder: the waiter starts at once.
   - In a Claude session, a raw `cargo bench --no-run` is refused with the drive command to use.
