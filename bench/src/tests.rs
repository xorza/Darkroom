use crate::quick_bench;

#[quick_bench(iterations = 5)]
fn bench_simple_addition(b: Bencher) {
    b.bench(|| {
        let mut sum = 0u64;
        for i in 0..1000 {
            sum += i;
        }
        sum
    });
}

#[quick_bench(iterations = 3)]
fn bench_vec_allocation(b: Bencher) {
    b.bench(|| {
        let v: Vec<i32> = (0..10_000).collect();
        v.len()
    });
}

#[quick_bench(iterations = 10)]
fn bench_string_concatenation(b: Bencher) {
    b.bench(|| {
        let mut s = String::new();
        for i in 0..100 {
            s.push_str(&i.to_string());
        }
        s
    });
}

#[quick_bench]
fn bench_default_iterations(b: Bencher) {
    b.bench(|| std::hint::black_box(42));
}

#[quick_bench(iterations = 5)]
fn bench_sorting(b: Bencher) {
    b.bench(|| {
        let mut v: Vec<i32> = (0..1000).rev().collect();
        v.sort();
        v
    });
}

#[quick_bench(iterations = 5)]
fn bench_hashmap_insert(b: Bencher) {
    use std::collections::HashMap;

    b.bench(|| {
        let mut map = HashMap::new();
        for i in 0..1000 {
            map.insert(i, i * 2);
        }
        map
    });
}
