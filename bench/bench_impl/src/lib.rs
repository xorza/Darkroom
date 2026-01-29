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

mod examples;

/// A simple bencher for measuring execution time in tests.
#[derive(Debug)]
pub struct Bencher {
    name: String,
    iterations: usize,
    warmup_iterations: usize,
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

    /// Set the output directory for benchmark results.
    #[must_use]
    pub fn with_output_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.output_dir = Some(dir.into());
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
            name: self.name.clone(),
            iterations: times.len(),
            total,
            mean,
            min,
            max,
            median,
        };

        println!("\n{result}\n");

        // Write to file and compare with previous if output_dir is set
        if let Some(dir) = &self.output_dir {
            let bench_dir = dir.join("bench-results");
            if let Err(e) = create_dir_all(&bench_dir) {
                eprintln!("Failed to create bench-results directory: {e}");
            } else {
                let file_path = bench_dir.join(format!("{}.txt", self.name));

                // Read previous result for comparison
                if let Some(prev_mean) = Self::read_previous_result(&file_path) {
                    let diff = result.mean.as_secs_f64() - prev_mean.as_secs_f64();
                    let pct = (diff / prev_mean.as_secs_f64()) * 100.0;
                    let sign = if diff >= 0.0 { "+" } else { "" };
                    let indicator = if pct < -5.0 {
                        "faster"
                    } else if pct > 5.0 {
                        "SLOWER"
                    } else {
                        "same"
                    };
                    println!(
                        "  vs previous: {prev_mean:?} -> {:?} ({sign}{:.1}%) {indicator}\n",
                        result.mean, pct
                    );
                }

                // Overwrite result file
                match OpenOptions::new()
                    .create(true)
                    .write(true)
                    .truncate(true)
                    .open(&file_path)
                {
                    Ok(mut file) => {
                        let content = format!(
                            "name: {}\nmean: {:?}\nmin: {:?}\nmax: {:?}\nmedian: {:?}\niterations: {}\n",
                            self.name,
                            result.mean,
                            result.min,
                            result.max,
                            result.median,
                            result.iterations
                        );
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
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_bencher_new() {
        let b = Bencher::new("test_name");
        assert_eq!(b.name, "test_name");
        assert_eq!(b.iterations, 10);
        assert_eq!(b.warmup_iterations, 1);
        assert!(b.output_dir.is_none());
    }

    #[test]
    fn test_bencher_with_iterations() {
        let b = Bencher::new("test").with_iterations(50);
        assert_eq!(b.iterations, 50);
    }

    #[test]
    fn test_bencher_with_warmup() {
        let b = Bencher::new("test").with_warmup(5);
        assert_eq!(b.warmup_iterations, 5);
    }

    #[test]
    fn test_bencher_with_output_dir() {
        let b = Bencher::new("test").with_output_dir("/tmp/bench");
        assert_eq!(b.output_dir, Some(PathBuf::from("/tmp/bench")));
    }

    #[test]
    fn test_bencher_chaining() {
        let b = Bencher::new("chained")
            .with_iterations(20)
            .with_warmup(3)
            .with_output_dir("/tmp");
        assert_eq!(b.name, "chained");
        assert_eq!(b.iterations, 20);
        assert_eq!(b.warmup_iterations, 3);
        assert_eq!(b.output_dir, Some(PathBuf::from("/tmp")));
    }

    #[test]
    fn test_bench_returns_result() {
        let result = Bencher::new("simple_bench")
            .with_iterations(5)
            .with_warmup(1)
            .bench(|| {
                let mut sum = 0u64;
                for i in 0..100 {
                    sum += i;
                }
                sum
            });

        assert_eq!(result.name, "simple_bench");
        assert_eq!(result.iterations, 5);
        assert!(result.min <= result.mean);
        assert!(result.mean <= result.max);
        assert!(result.min <= result.median);
        assert!(result.median <= result.max);
    }

    #[test]
    fn test_bench_result_display() {
        let result = BenchResult {
            name: "test_display".to_string(),
            iterations: 10,
            total: Duration::from_millis(100),
            mean: Duration::from_millis(10),
            min: Duration::from_millis(8),
            max: Duration::from_millis(12),
            median: Duration::from_millis(10),
        };

        let display = format!("{result}");
        assert!(display.contains("test_display"));
        assert!(display.contains("10ms"));
        assert!(display.contains("8ms"));
        assert!(display.contains("12ms"));
        assert!(display.contains("10 iters"));
    }

    #[test]
    fn test_parse_duration_milliseconds() {
        assert_eq!(
            parse_duration("123.456ms"),
            Some(Duration::from_secs_f64(0.123456))
        );
        assert_eq!(parse_duration("1ms"), Some(Duration::from_secs_f64(0.001)));
        assert_eq!(
            parse_duration("0.5ms"),
            Some(Duration::from_secs_f64(0.0005))
        );
    }

    #[test]
    fn test_parse_duration_seconds() {
        assert_eq!(parse_duration("1s"), Some(Duration::from_secs(1)));
        assert_eq!(parse_duration("2.5s"), Some(Duration::from_secs_f64(2.5)));
    }

    #[test]
    fn test_parse_duration_microseconds() {
        assert_eq!(
            parse_duration("1000µs"),
            Some(Duration::from_secs_f64(0.001))
        );
        assert_eq!(
            parse_duration("500µs"),
            Some(Duration::from_secs_f64(0.0005))
        );
        assert_eq!(
            parse_duration("1000us"),
            Some(Duration::from_secs_f64(0.001))
        );
    }

    #[test]
    fn test_parse_duration_nanoseconds() {
        assert_eq!(
            parse_duration("1000ns"),
            Some(Duration::from_secs_f64(0.000001))
        );
        assert_eq!(
            parse_duration("500ns"),
            Some(Duration::from_secs_f64(0.0000005))
        );
    }

    #[test]
    fn test_parse_duration_invalid() {
        assert_eq!(parse_duration("invalid"), None);
        assert_eq!(parse_duration("123"), None);
        assert_eq!(parse_duration("ms"), None);
        assert_eq!(parse_duration(""), None);
    }

    #[test]
    fn test_parse_duration_with_whitespace() {
        assert_eq!(
            parse_duration("  100ms  "),
            Some(Duration::from_secs_f64(0.1))
        );
        assert_eq!(
            parse_duration("\t50ms\n"),
            Some(Duration::from_secs_f64(0.05))
        );
    }

    #[test]
    fn test_bench_writes_result_file() {
        let temp_dir = std::env::temp_dir().join("bench_test_write");
        let _ = fs::remove_dir_all(&temp_dir);

        let _result = Bencher::new("test_file_write")
            .with_iterations(3)
            .with_output_dir(&temp_dir)
            .bench(|| std::thread::sleep(Duration::from_micros(100)));

        let file_path = temp_dir.join("bench-results/test_file_write.txt");
        assert!(file_path.exists(), "Result file should exist");

        let content = fs::read_to_string(&file_path).unwrap();
        assert!(content.contains("name: test_file_write"));
        assert!(content.contains("mean:"));
        assert!(content.contains("min:"));
        assert!(content.contains("max:"));
        assert!(content.contains("median:"));
        assert!(content.contains("iterations: 3"));

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_read_previous_result() {
        let temp_dir = std::env::temp_dir().join("bench_test_read_prev");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        let file_path = temp_dir.join("test_bench.txt");
        fs::write(
            &file_path,
            "name: test_bench\nmean: 100ms\nmin: 90ms\nmax: 110ms\nmedian: 100ms\niterations: 5\n",
        )
        .unwrap();

        let result = Bencher::read_previous_result(&file_path);
        assert_eq!(result, Some(Duration::from_secs_f64(0.1)));

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_read_previous_result_missing_file() {
        let file_path = PathBuf::from("/nonexistent/path/bench.txt");
        let result = Bencher::read_previous_result(&file_path);
        assert_eq!(result, None);
    }

    #[test]
    fn test_read_previous_result_invalid_format() {
        let temp_dir = std::env::temp_dir().join("bench_test_invalid");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        let file_path = temp_dir.join("invalid.txt");
        fs::write(&file_path, "some random content\nno mean here\n").unwrap();

        let result = Bencher::read_previous_result(&file_path);
        assert_eq!(result, None);

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_bench_overwrites_result_file() {
        let temp_dir = std::env::temp_dir().join("bench_test_overwrite");
        let _ = fs::remove_dir_all(&temp_dir);

        let _result1 = Bencher::new("test_overwrite")
            .with_iterations(2)
            .with_output_dir(&temp_dir)
            .bench(|| std::thread::sleep(Duration::from_micros(50)));

        let file_path = temp_dir.join("bench-results/test_overwrite.txt");
        let _content1 = fs::read_to_string(&file_path).unwrap();

        let _result2 = Bencher::new("test_overwrite")
            .with_iterations(3)
            .with_output_dir(&temp_dir)
            .bench(|| std::thread::sleep(Duration::from_micros(50)));

        let content2 = fs::read_to_string(&file_path).unwrap();

        assert!(content2.contains("iterations: 3"));
        assert!(!content2.contains("iterations: 2"));
        assert_eq!(content2.matches("name:").count(), 1);

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_bench_result_statistics() {
        let result = Bencher::new("stats_test")
            .with_iterations(10)
            .with_warmup(2)
            .bench(|| {
                std::thread::sleep(Duration::from_millis(1));
            });

        assert!(result.total >= Duration::from_millis(10));
        assert!(result.mean >= Duration::from_millis(1));
        assert!(result.min <= result.max);
        assert!(result.median >= result.min);
        assert!(result.median <= result.max);
    }

    #[test]
    fn test_default_bencher() {
        let b = Bencher::default();
        assert!(b.name.is_empty());
        assert_eq!(b.iterations, 10);
        assert_eq!(b.warmup_iterations, 1);
        assert!(b.output_dir.is_none());
    }
}
