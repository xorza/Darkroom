# Lumos Auto Implementation

> Autonomous Claude Code runner for lumos astrophotography stacking library

## Quick Start

```bash
# Make scripts executable
chmod +x run_implementation.sh run_headless.sh

# Start interactive implementation (with confirmations)
./run_implementation.sh

# Or headless mode (no confirmations - use in sandbox only!)
./run_headless.sh
```

## Files

| File | Description |
|------|-------------|
| `SPEC.md` | Task checklist derived from PLAN.md |
| `run_implementation.sh` | Interactive runner (prompts for confirmation) |
| `run_headless.sh` | Fully autonomous runner (no prompts) |
| `logs/` | Session logs and progress tracking |

## How It Works

1. Claude reads `SPEC.md` to find the next unchecked `[ ]` task
2. Reads `CLAUDE.md` for coding rules and `PLAN.md` for algorithm details
3. Implements one task at a time
4. Runs verification: `cargo nextest run && cargo fmt && cargo check && cargo clippy`
5. For optimizations: runs benchmarks, removes if <10% improvement
6. Marks task complete `[x]` and commits
7. Loops until all tasks done

## Implementation Status

| Phase | Tasks | Status |
|-------|-------|--------|
| Local Normalization | 5 | Not Started |
| GPU Acceleration | 12 | Partial |
| Advanced Features | 14 | Not Started |
| Quality & Polish | 4 | Not Started |

## Remaining Work (from PLAN.md)

### High Priority
- Local normalization (tile-based, PixInsight-style)
- GPU FFT for phase correlation
- GPU sigma clipping

### Medium Priority
- Comet/asteroid stacking mode
- Multi-session integration
- Astrometric solution (Gaia catalog matching)

### Lower Priority
- Real-time preview / live stacking
- Deep learning integration

## Logs

Session logs are saved to `logs/` with timestamps:
- `session_YYYYMMDD_HHMMSS.log` - interactive sessions
- `headless_YYYYMMDD_HHMMSS.log` - headless sessions
- `progress.log` - cumulative progress tracking

## Customization

Edit `SPEC.md` to:
- Add new tasks
- Reorder priorities
- Mark tasks as `[SKIPPED]` to bypass
- Adjust verification thresholds
