# Lumos Auto Implementation

> Autonomous Claude Code runner

## Quick Start

```bash
# Or headless mode (no confirmations - use in sandbox only!)
./run_headless.sh
```

## Files

| File | Description |
|------|-------------|
| `PLAN.md` | Task checklist |
| `run_headless.sh` | Fully autonomous runner (no prompts) |
| `logs/` | Session logs and progress tracking |

## How It Works

1. Claude reads `PLAN.md` to find the next unchecked `[ ]` task
2. Reads `CLAUDE.md` for coding rules and `PLAN.md` for algorithm details
3. Implements one task at a time
4. Runs verification: `cargo nextest run && cargo fmt && cargo check && cargo clippy`
5. For optimizations: runs benchmarks, removes if <10% improvement
6. Marks task complete `[x]` and commits
7. Loops until all tasks done

Edit `PLAN.md` to:
- Add new tasks
- Reorder priorities
- Mark tasks as `[SKIPPED]` to bypass
- Adjust verification thresholds
