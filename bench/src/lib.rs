//! Simple benchmarking utilities for use in tests.
//!
//! This crate provides a lightweight benchmarking framework that runs as regular tests
//! but measures execution time with proper warmup and statistics.
//!
//! # Usage with `#[quick_bench]` attribute (recommended)
//!
//! ```ignore
//! use bench::{quick_bench, Bencher};
//!
//! #[quick_bench]
//! fn bench_something(b: Bencher) {
//!     b.bench(|| {
//!         // code to benchmark
//!     });
//! }
//!
//! #[quick_bench(iterations = 20)]
//! fn bench_with_iterations(b: Bencher) {
//!     b.bench(|| {
//!         // code to benchmark
//!     });
//! }
//! ```
//!
//! # Usage with Bencher directly
//!
//! ```ignore
//! use bench::Bencher;
//!
//! #[test]
//! fn bench_my_function() {
//!     Bencher::new("my_function")
//!         .with_iterations(10)
//!         .bench(|| {
//!             // code to benchmark
//!         });
//! }
//! ```
//!
//! Run with: `cargo test --release -- --nocapture bench_`

use std::hint::black_box;
use std::time::{Duration, Instant};

pub use bench_macros::quick_bench;

/// A simple bencher for measuring execution time in tests.
#[derive(Debug)]
pub struct Bencher {
    name: String,
    iterations: usize,
    warmup_iterations: usize,
}

/// Statistics from a benchmark run.
#[derive(Debug)]
pub struct BenchResult {
    pub name: String,
    pub iterations: usize,
    pub total: Duration,
    pub mean: Duration,
    pub min: Duration,
    pub max: Duration,
    pub median: Duration,
}

impl std::fmt::Display for BenchResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[BENCH] {}: {:?} (min: {:?}, max: {:?}, median: {:?}, {} iters)",
            self.name, self.mean, self.min, self.max, self.median, self.iterations
        )
    }
}

impl Default for Bencher {
    fn default() -> Self {
        Self {
            name: String::new(),
            iterations: 10,
            warmup_iterations: 1,
        }
    }
}

impl Bencher {
    /// Create a new bencher with the given name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Default::default()
        }
    }

    /// Set the number of timed iterations.
    #[must_use]
    pub fn with_iterations(mut self, iterations: usize) -> Self {
        self.iterations = iterations;
        self
    }

    /// Set the number of warmup iterations (not timed).
    #[must_use]
    pub fn with_warmup(mut self, warmup: usize) -> Self {
        self.warmup_iterations = warmup;
        self
    }

    /// Run the benchmark.
    ///
    /// The closure is called `iterations` times after warmup.
    pub fn bench<F, R>(self, mut f: F) -> BenchResult
    where
        F: FnMut() -> R,
    {
        #[cfg(debug_assertions)]
        println!("\n⚠️  DEBUG MODE - benchmarks should be run with --release\n");

        // Warmup
        for _ in 0..self.warmup_iterations {
            black_box(f());
        }

        // Timed runs
        let mut times = Vec::with_capacity(self.iterations);
        for _ in 0..self.iterations {
            let start = Instant::now();
            black_box(f());
            times.push(start.elapsed());
        }

        self.compute_result(times)
    }

    fn compute_result(self, mut times: Vec<Duration>) -> BenchResult {
        times.sort();

        let total: Duration = times.iter().sum();
        let mean = total / times.len() as u32;
        let min = times.first().copied().unwrap_or_default();
        let max = times.last().copied().unwrap_or_default();
        let median = times.get(times.len() / 2).copied().unwrap_or_default();

        let result = BenchResult {
            name: self.name,
            iterations: times.len(),
            total,
            mean,
            min,
            max,
            median,
        };

        println!("\n{result}\n");

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Note: #[quick_bench] examples must be in crates that depend on `bench`,
    // not inside the bench crate itself. See lumos tests for examples.

    #[test]
    fn bench_example_direct() {
        Bencher::new("example_direct")
            .with_iterations(100)
            .bench(|| {
                let mut sum = 0u64;
                for i in 0..1000 {
                    sum += i;
                }
                sum
            });
    }
}
