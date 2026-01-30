//! Simple benchmarking utilities for use in tests.
//!
//! This crate provides a lightweight benchmarking framework that runs as regular tests
//! but measures execution time with proper warmup and statistics.

use std::fs::{OpenOptions, create_dir_all, read_to_string};
use std::hint::black_box;
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub use bench_macros::quick_bench;

// ANSI color codes
mod colors {
    pub const RESET: &str = "\x1b[0m";
    pub const BOLD: &str = "\x1b[1m";
    pub const CYAN: &str = "\x1b[36m";
    pub const YELLOW: &str = "\x1b[33m";
    pub const GREEN: &str = "\x1b[32m";
    pub const RED: &str = "\x1b[31m";
    pub const DIM: &str = "\x1b[2m";
}

/// A simple bencher for measuring execution time in tests.
#[derive(Debug)]
pub struct Bencher {
    name: String,
    warmup_time: Option<Duration>,
    time: Option<Duration>,
    warmup_iters: Option<u64>,
    iters: Option<u64>,
    output_dir: Option<PathBuf>,
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
        use colors::*;
        write!(
            f,
            "{CYAN}{BOLD}[BENCH]{RESET} {BOLD}{}{RESET}:\n{YELLOW}{:?}{RESET} {DIM}(min: {:?}, max: {:?}, median: {:?}, {} iters){RESET}",
            self.name, self.mean, self.min, self.max, self.median, self.iterations
        )
    }
}

impl Default for Bencher {
    fn default() -> Self {
        Self {
            name: String::new(),
            warmup_time: Some(Duration::from_secs(1)),
            time: Some(Duration::from_secs(5)),
            warmup_iters: None,
            iters: None,
            output_dir: None,
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

    /// Set the warmup time in milliseconds.
    #[must_use]
    pub fn with_warmup_time_ms(mut self, ms: u64) -> Self {
        self.warmup_time = Some(Duration::from_millis(ms));
        self
    }

    /// Set the benchmark time in milliseconds.
    #[must_use]
    pub fn with_bench_time_ms(mut self, ms: u64) -> Self {
        self.time = Some(Duration::from_millis(ms));
        self
    }

    /// Disable warmup time limit (use only iteration count).
    #[must_use]
    pub fn without_warmup_time(mut self) -> Self {
        self.warmup_time = None;
        self
    }

    /// Disable bench time limit (use only iteration count).
    #[must_use]
    pub fn without_bench_time(mut self) -> Self {
        self.time = None;
        self
    }

    /// Set the maximum number of warmup iterations.
    #[must_use]
    pub fn with_warmup_iters(mut self, iters: u64) -> Self {
        self.warmup_iters = Some(iters);
        self
    }

    /// Set the maximum number of benchmark iterations.
    #[must_use]
    pub fn with_iters(mut self, iters: u64) -> Self {
        self.iters = Some(iters);
        self
    }

    /// Set the output directory for benchmark results.
    #[must_use]
    pub fn with_output_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.output_dir = Some(dir.into());
        self
    }

    /// Run a labeled benchmark variant without consuming self.
    ///
    /// This allows running multiple benchmark variants in a single test function:
    /// ```ignore
    /// b.bench_labeled("scalar", || scalar_impl());
    /// b.bench_labeled("simd", || simd_impl());
    /// ```
    pub fn bench_labeled<F, R>(&self, label: &str, f: F) -> BenchResult
    where
        F: FnMut() -> R,
    {
        let labeled_name = format!("{}/{}", self.name, label);
        let bencher = Bencher {
            name: labeled_name,
            warmup_time: self.warmup_time,
            time: self.time,
            warmup_iters: self.warmup_iters,
            iters: self.iters,
            output_dir: self.output_dir.clone(),
        };
        bencher.bench(f)
    }

    /// Run the benchmark.
    ///
    /// Runs warmup iterations until warmup_time is reached or warmup_iters is hit (whichever comes first),
    /// then runs benchmark iterations until bench_time is reached or iters is hit (whichever comes first).
    ///
    /// At least one of time or iterations must be set for both warmup and bench phases.
    pub fn bench<F, R>(self, mut f: F) -> BenchResult
    where
        F: FnMut() -> R,
    {
        assert!(
            self.warmup_time.is_some() || self.warmup_iters.is_some(),
            "Either warmup_time or warmup_iters must be set"
        );
        assert!(
            self.time.is_some() || self.iters.is_some(),
            "Either bench_time or iters must be set"
        );

        #[cfg(debug_assertions)]
        println!(
            "\n{}{}⚠️ WARNING:{} DEBUG MODE - benchmarks should be run with --release\n",
            colors::YELLOW,
            colors::BOLD,
            colors::RESET
        );

        // Warmup: run until warmup_time is reached or warmup_iters is hit
        let warmup_start = Instant::now();
        let mut warmup_count = 0u64;
        loop {
            if let Some(time) = self.warmup_time
                && warmup_start.elapsed() >= time
            {
                break;
            }
            if let Some(max) = self.warmup_iters
                && warmup_count >= max
            {
                break;
            }
            black_box(f());
            warmup_count += 1;
        }

        // Timed runs: run until bench_time is reached or iters is hit
        let mut times = Vec::with_capacity(1000);
        let bench_start = Instant::now();
        loop {
            if let Some(time) = self.time
                && bench_start.elapsed() >= time
            {
                break;
            }
            if let Some(max) = self.iters
                && times.len() as u64 >= max
            {
                break;
            }
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
            name: self.name.clone(),
            iterations: times.len(),
            total,
            mean,
            min,
            max,
            median,
        };

        println!("\n{result}");

        // Write to file and compare with previous if output_dir is set
        if let Some(dir) = &self.output_dir {
            let bench_dir = dir.join("bench-results");
            if let Err(e) = create_dir_all(&bench_dir) {
                eprintln!("Failed to create bench-results directory: {e}");
            } else {
                let file_path = bench_dir.join(format!("{}.txt", self.name));

                // Read previous result for comparison
                let comparison = if let Some(prev_mean) = Self::read_previous_result(&file_path) {
                    let diff = result.mean.as_secs_f64() - prev_mean.as_secs_f64();
                    let pct = (diff / prev_mean.as_secs_f64()) * 100.0;
                    let sign = if diff >= 0.0 { "+" } else { "" };
                    let (indicator, color) = if pct < -5.0 {
                        ("faster", colors::GREEN)
                    } else if pct > 5.0 {
                        ("SLOWER", colors::RED)
                    } else {
                        ("same", colors::DIM)
                    };
                    println!(
                        "  {}vs previous:{} {:?} -> {:?} ({sign}{:.1}%) {}{}{}",
                        colors::DIM,
                        colors::RESET,
                        prev_mean,
                        result.mean,
                        pct,
                        color,
                        indicator,
                        colors::RESET
                    );
                    Some(format!(
                        "vs_previous: {:?} -> {:?} ({sign}{:.1}%) {indicator}",
                        prev_mean, result.mean, pct
                    ))
                } else {
                    None
                };

                // Create parent directories for the file if needed (for labeled benchmarks with '/')
                if let Some(parent) = file_path.parent() {
                    let _ = create_dir_all(parent);
                }

                // Overwrite result file
                match OpenOptions::new()
                    .create(true)
                    .write(true)
                    .truncate(true)
                    .open(&file_path)
                {
                    Ok(mut file) => {
                        let mut content = format!(
                            "name: {}\nmean: {:?}\nmin: {:?}\nmax: {:?}\nmedian: {:?}\niterations: {}\n",
                            self.name,
                            result.mean,
                            result.min,
                            result.max,
                            result.median,
                            result.iterations
                        );
                        if let Some(cmp) = comparison {
                            content.push_str(&cmp);
                        }
                        if let Err(e) = file.write_all(content.as_bytes()) {
                            eprintln!("Failed to write benchmark result: {e}");
                        }
                    }
                    Err(e) => eprintln!("Failed to open benchmark results file: {e}"),
                }
            }
        }

        result
    }

    /// Read the previous result from a benchmark file.
    fn read_previous_result(file_path: &PathBuf) -> Option<Duration> {
        let content = read_to_string(file_path).ok()?;

        // Find the "mean:" line and parse its value
        content
            .lines()
            .find(|line| line.starts_with("mean:"))
            .and_then(|line| {
                line.strip_prefix("mean:")
                    .map(str::trim)
                    .and_then(parse_duration)
            })
    }
}

/// Parse a duration string like "123.456ms" or "1.234s"
fn parse_duration(s: &str) -> Option<Duration> {
    let s = s.trim();
    if let Some(ms) = s.strip_suffix("ms") {
        ms.parse::<f64>()
            .ok()
            .map(|v| Duration::from_secs_f64(v / 1000.0))
    } else if let Some(us) = s.strip_suffix("µs").or_else(|| s.strip_suffix("us")) {
        us.parse::<f64>()
            .ok()
            .map(|v| Duration::from_secs_f64(v / 1_000_000.0))
    } else if let Some(ns) = s.strip_suffix("ns") {
        ns.parse::<f64>()
            .ok()
            .map(|v| Duration::from_secs_f64(v / 1_000_000_000.0))
    } else if let Some(secs) = s.strip_suffix('s') {
        secs.parse::<f64>().ok().map(Duration::from_secs_f64)
    } else {
        None
    }
}

#[cfg(test)]
mod tests;
