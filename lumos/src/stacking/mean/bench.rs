//! Benchmark module for mean stacking.
//! Run with: cargo bench -p lumos --features bench --bench stack_mean

use std::path::Path;

use criterion::Criterion;

/// Register mean stacking benchmarks with Criterion.
pub fn benchmarks(_c: &mut Criterion, _calibration_dir: &Path) {}
