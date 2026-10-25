//! A profile integrated over every pixel of a star's stamp, at the orders the fits pick, the rules
//! built at those orders, and the search for the order a profile needs.

use std::hint::black_box;

use common::bench::{Kind, Scale};
use criterion::{BenchmarkId, Criterion, Throughput};

use crate::math::pixel_quadrature::{AnalyticProfile, PixelQuadrature};

/// The orders the f32 tolerance picks at the narrowest Gaussian and Moffat the fits admit, and
/// the highest.
const ORDERS: [usize; 3] = [7, 11, 16];
const STAMP: usize = 15;
/// The f32 pixel-mean tolerance the star fits integrate to.
const TOLERANCE: f64 = 1.0 / (1u64 << 25) as f64;

pub(crate) fn atomic(c: &mut Criterion) {
    let mut group = Kind::Atomic.group(c, "math/pixel_quadrature", Scale::Micros);
    for order in ORDERS {
        group.throughput(Throughput::Elements((STAMP * STAMP) as u64));
        group.bench_function(
            BenchmarkId::new(format!("integrate/order_{order}"), STAMP * STAMP),
            |b| {
                let rule = PixelQuadrature::gauss_legendre(order);
                let centre = STAMP as f64 / 2.0 - 0.3;
                let gaussian = |x: f64, y: f64| {
                    let (dx, dy) = (x - centre, y - centre);
                    (-(dx * dx + dy * dy) / (2.0 * 1.2 * 1.2)).exp()
                };
                b.iter(|| {
                    let mut sum = 0.0;
                    for y in 0..STAMP {
                        for x in 0..STAMP {
                            sum += rule.integrate(x as f64, y as f64, black_box(&gaussian));
                        }
                    }
                    sum
                });
            },
        );
        group.throughput(Throughput::Elements(1));
        group.bench_function(BenchmarkId::new("gauss_legendre", order), |b| {
            b.iter(|| PixelQuadrature::gauss_legendre(black_box(order)));
        });
    }
    group.throughput(Throughput::Elements(1));
    for (variant, profile) in [
        ("gaussian_0.5", AnalyticProfile::Gaussian { sigma: 0.5 }),
        ("gaussian_2", AnalyticProfile::Gaussian { sigma: 2.0 }),
        (
            "moffat_1.5_2.5",
            AnalyticProfile::Moffat {
                alpha: 1.5,
                beta: 2.5,
            },
        ),
    ] {
        group.bench_function(
            BenchmarkId::new(format!("sufficient_order/{variant}"), 1),
            |b| b.iter(|| PixelQuadrature::sufficient_order(black_box(profile), TOLERANCE)),
        );
    }
    group.finish();
}
