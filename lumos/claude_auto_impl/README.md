# Project Name

> Auto-implemented using Claude Code autonomous runner

## Quick Start

```bash
# Make script executable
chmod +x run_implementation.sh

# Edit SPEC.md to define your project
vim SPEC.md

# Start autonomous implementation
./run_implementation.sh
```

## Project Structure

```
.
├── SPEC.md                 # Implementation specification (task list)
├── run_implementation.sh   # Autonomous runner script
├── src/
│   ├── lib.rs             # Public API
│   ├── scalar/            # Baseline implementations
│   │   └── README.md      # Scalar implementation notes
│   ├── simd/              # SIMD optimizations
│   │   └── README.md      # SIMD optimization notes
│   └── gpu/               # GPU optimizations
│       └── README.md      # GPU optimization notes
├── benches/               # Benchmarks
├── tests/                 # Integration tests
├── docs/                  # Documentation
│   ├── RESEARCH.md        # Best practices research
│   └── API_DESIGN.md      # API design document
└── logs/                  # Implementation logs
```

## Implementation Status

| Phase | Status | Progress |
|-------|--------|----------|
| Research & Design | ⏳ Pending | 0/5 |
| Core Implementation | ⏳ Pending | 0/8 |
| SIMD Optimizations | ⏳ Pending | 0/9 |
| GPU Optimizations | ⏳ Pending | 0/9 |
| Quality Assurance | ⏳ Pending | 0/7 |

## Benchmark Results

<!-- Auto-populated during implementation -->

| Implementation | Time (ns) | vs Baseline | Status |
|---------------|-----------|-------------|--------|
| Scalar | - | baseline | ⏳ |
| SIMD | - | - | ⏳ |
| GPU | - | - | ⏳ |

## Usage

```rust
// Example usage will be added during implementation
```

## License

<!-- Add your license -->
