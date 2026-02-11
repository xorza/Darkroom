use crate::Bencher;

// Time-based benchmark: runs for specified duration
#[test]
#[ignore]
fn bench_time_based() {
    let b = Bencher::new("bench_time_based")
        .with_warmup_time_ms(50)
        .with_bench_time_ms(100)
        .with_output_dir(env!("CARGO_MANIFEST_DIR"));
    b.bench(|| {
        let mut sum = 0u64;
        for i in 0..1000 {
            sum += i;
        }
        sum
    });
}

// Iteration-based benchmark: runs exact number of iterations
#[test]
#[ignore]
fn bench_iteration_based() {
    let b = Bencher::new("bench_iteration_based")
        .without_warmup_time()
        .without_bench_time()
        .with_warmup_iters(100)
        .with_iters(500)
        .with_output_dir(env!("CARGO_MANIFEST_DIR"));
    b.bench(|| {
        let v: Vec<i32> = (0..10_000).collect();
        v.len()
    });
}

// Combined limits: stops at whichever comes first
#[test]
#[ignore]
fn bench_combined_limits() {
    let b = Bencher::new("bench_combined_limits")
        .with_warmup_time_ms(50)
        .with_warmup_iters(10)
        .with_bench_time_ms(200)
        .with_iters(100)
        .with_output_dir(env!("CARGO_MANIFEST_DIR"));
    b.bench(|| {
        let mut s = String::new();
        for i in 0..100 {
            s.push_str(&i.to_string());
        }
        s
    });
}

// Fast operation with iteration limit to prevent excessive runs
#[test]
#[ignore]
fn bench_fast_operation() {
    let b = Bencher::new("bench_fast_operation")
        .without_warmup_time()
        .without_bench_time()
        .with_warmup_iters(1000)
        .with_iters(10000)
        .with_output_dir(env!("CARGO_MANIFEST_DIR"));
    b.bench(|| std::hint::black_box(42));
}

// Time-limited with iteration cap for expensive operations
#[test]
#[ignore]
fn bench_expensive_with_cap() {
    let b = Bencher::new("bench_expensive_with_cap")
        .with_warmup_time_ms(20)
        .with_bench_time_ms(100)
        .with_iters(50)
        .with_output_dir(env!("CARGO_MANIFEST_DIR"));
    b.bench(|| {
        let mut v: Vec<i32> = (0..1000).rev().collect();
        v.sort();
        v
    });
}

// Pure iteration-based for deterministic benchmarks
#[test]
#[ignore]
fn bench_deterministic() {
    use std::collections::HashMap;

    let b = Bencher::new("bench_deterministic")
        .without_warmup_time()
        .without_bench_time()
        .with_warmup_iters(50)
        .with_iters(200)
        .with_output_dir(env!("CARGO_MANIFEST_DIR"));
    b.bench(|| {
        let mut map = HashMap::new();
        for i in 0..1000 {
            map.insert(i, i * 2);
        }
        map
    });
}
