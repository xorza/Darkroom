# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

`rust-outline` is a standalone dev tool: it emits a Markdown structural outline
of Rust source — type definitions with their fields (type + visibility) and
`impl`/`trait` blocks with method signatures. It is **not** a Scenarium
workspace member; its `Cargo.toml` declares its own empty `[workspace]` so
`cargo build` at the repo root never compiles its `syn`/`prettyplease` deps.
Root `../../CLAUDE.md` holds the workspace-wide Rust style and tooling rules;
this file covers only what's specific to the tool.

## Commands

```
./outline <PATH> [-o OUT.md] [--tests]     # wrapper: build --release once, then run
cargo run --release -- <PATH> [-o OUT.md]  # same, without the wrapper
cargo clippy --all-targets -- -D warnings
cargo fmt --all
```

- `PATH` — file or directory (default `.`). Directory scans respect `.gitignore`
  and skip hidden dirs (`.tmp`, `target`, `.git`) via the `ignore` crate.
- `-o OUT` — write Markdown to a file; default is stdout.
- `--tests` — include `#[cfg(test)]` modules, skipped by default.

## How it works

Single file, `src/main.rs`. `main` → `parse_args` → `collect_rs_files` (walks
with `ignore`, sorts for deterministic output) → per file `syn::parse_file` →
`walk_items` recurses the AST and appends Markdown; parse failures are logged to
stderr and skipped, never fatal.

Rendering is deliberately syntactic, not semantic — types print as written in
source, which is what an outline wants. The trick throughout: reconstruct a
minimal `syn` item and run it through `prettyplease`, then `collapse` whitespace
to one line:

- `render_type` wraps the type in a synthetic `type Ty = <ty>;` and strips the
  scaffolding back off.
- `render_sig` wraps the signature in a synthetic `fn … {}` and trims the body.
- `header_of` / `impl_header` render the item and cut at the first `{` or `;`,
  so generics, bounds, lifetimes and `where` clauses survive but bodies don't.
- `vis_prefix` renders visibility via `quote` and tightens token spacing
  (`pub ( crate )` → `pub(crate)`).

Attributes (docs, `#[derive]`, `#[cfg]`) are stripped before rendering to keep
the outline clean; `#[cfg(test)]` on a `mod` is the one attribute inspected, to
decide whether to skip it (`is_cfg_test`).

## Conventions

- Keep it a single self-contained `main.rs`; no submodules, no clap (args are
  parsed by hand).
- The output format is load-bearing for humans skimming a crate — changes to
  heading levels or bullet shape should stay stable and diff-friendly.
- Keep it detached from the workspace: never add it to root `members`, and keep
  the empty `[workspace]` in `Cargo.toml`.
