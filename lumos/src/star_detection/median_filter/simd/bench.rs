//! The 3×3 median over one row of a frame's residual, on the scalar reference, every tier's kernel
//! and the dispatcher: production filters a demosaiced frame's rows on the global pool, a row a
//! task. Every tier is measured, the gate that picks one included.

use std::cell::OnceCell;
use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, Throughput};

use crate::bench::fixture::{Kind, Scale};
use crate::bench::scene::{FRAME, Field};
use crate::simd::Kernel;
use crate::simd::isa_tier::IsaTier;
use crate::star_detection::bench::Stages;
use crate::star_detection::median_filter::simd::MedianRow;

pub(crate) fn atomic(c: &mut Criterion) {
    let stages = OnceCell::new();
    let rows = || {
        let measure = stages
            .get_or_init(|| Stages::of(Field::Typical))
            .frame
            .measure
            .pixels();
        let y = FRAME.height / 2;
        let row = |y: usize| &measure[y * FRAME.width..(y + 1) * FRAME.width];
        [row(y - 1), row(y), row(y + 1)]
    };
    let tiers: Vec<IsaTier> = IsaTier::supported().collect();
    let mut group = Kind::Atomic.group(c, "detection/median_filter", Scale::Sample);
    group.throughput(Throughput::Elements(FRAME.width as u64));
    let mut output = vec![0.0; FRAME.width];
    group.bench_function(BenchmarkId::new("row/scalar", FRAME.width), |b| {
        let [above, cur, below] = rows();
        b.iter(|| {
            MedianRow {
                above: black_box(above),
                cur,
                below,
                output: &mut output,
            }
            .run_scalar();
        });
    });
    for &tier in &tiers {
        group.bench_function(
            BenchmarkId::new(format!("row/{}", tier.id()), FRAME.width),
            |b| {
                let [above, cur, below] = rows();
                b.iter(|| {
                    tier.run(MedianRow {
                        above: black_box(above),
                        cur,
                        below,
                        output: &mut output,
                    });
                });
            },
        );
    }
    group.bench_function(BenchmarkId::new("row/dispatch", FRAME.width), |b| {
        let [above, cur, below] = rows();
        b.iter(|| {
            MedianRow {
                above: black_box(above),
                cur,
                below,
                output: &mut output,
            }
            .dispatch();
        });
    });
}
