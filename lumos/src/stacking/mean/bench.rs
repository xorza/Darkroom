//! Benchmark module for mean stacking.
//! Run with: cargo bench -p lumos --features bench --bench stack_mean

use std::hint::black_box;
use std::path::Path;

use criterion::{BenchmarkId, Criterion, Throughput};

use crate::math;

/// Register mean stacking benchmarks with Criterion.
pub fn benchmarks(c: &mut Criterion, _calibration_dir: &Path) {}
