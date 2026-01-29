use crate::quick_bench;

// Time-based benchmark: runs for specified duration
// Use when you want consistent benchmark duration regardless of operation speed
#[quick_bench(warmup_time_ms = 50, bench_time_ms = 100, ignore = false)]
fn bench_time_based(b: Bencher) {
    b.bench(|| {
        let mut sum = 0u64;
        for i in 0..1000 {
            sum += i;
        }
        sum
    });
}

// Iteration-based benchmark: runs exact number of iterations
// Use when you need precise iteration count for reproducibility
#[quick_bench(warmup_iters = 100, iters = 500, ignore = false)]
fn bench_iteration_based(b: Bencher) {
    b.bench(|| {
        let v: Vec<i32> = (0..10_000).collect();
        v.len()
    });
}

// Combined limits: stops at whichever comes first
// Use for expensive operations where you want a cap on both time and iterations
#[quick_bench(
    warmup_time_ms = 50,
    warmup_iters = 10,
    bench_time_ms = 200,
    iters = 100,
    ignore = false
)]
fn bench_combined_limits(b: Bencher) {
    b.bench(|| {
        let mut s = String::new();
        for i in 0..100 {
            s.push_str(&i.to_string());
        }
        s
    });
}

// Fast operation with iteration limit to prevent excessive runs
// Use for very fast operations that would run millions of times otherwise
#[quick_bench(warmup_iters = 1000, iters = 10000, ignore = false)]
fn bench_fast_operation(b: Bencher) {
    b.bench(|| std::hint::black_box(42));
}

// Time-limited with iteration cap for expensive operations
// Use when operation is slow and you want to limit total benchmark time
#[quick_bench(warmup_time_ms = 20, bench_time_ms = 100, iters = 50, ignore = false)]
fn bench_expensive_with_cap(b: Bencher) {
    b.bench(|| {
        let mut v: Vec<i32> = (0..1000).rev().collect();
        v.sort();
        v
    });
}

// Pure iteration-based for deterministic benchmarks
// Use when comparing algorithms and need exact same iteration count
#[quick_bench(warmup_iters = 50, iters = 200, ignore = false)]
fn bench_deterministic(b: Bencher) {
    use std::collections::HashMap;

    b.bench(|| {
        let mut map = HashMap::new();
        for i in 0..1000 {
            map.insert(i, i * 2);
        }
        map
    });
}
