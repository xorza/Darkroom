//! The cosmic-ray rejection of one calibrated light, in place, on a mono and on a Bayer sensor:
//! the two layouts it detects on.

use criterion::{BenchmarkId, Criterion, Throughput};

use crate::bench::fixture::{self, Scale};
use crate::bench::scene::{FRAME, Field};
use crate::calibration::cosmic_ray::cosmic_ray_config::CosmicRayConfig;
use crate::calibration::cosmic_ray::reject_cosmic_rays;
use crate::image::cfa::cfa_pattern::CfaPattern;
use crate::image::cfa::cfa_type::CfaType;

pub(crate) fn benches(c: &mut Criterion) {
    let field = Field::Typical;
    let mut group = fixture::group(c, "kernel/calibration/cosmic_ray/frame", Scale::Frame);
    group.throughput(Throughput::Elements(FRAME.pixel_count() as u64));
    for (operation, cfa_type) in [
        ("reject/mono", CfaType::Mono),
        ("reject/rggb", CfaType::Bayer(CfaPattern::Rggb)),
    ] {
        group.bench_function(BenchmarkId::new(operation, field.name()), |b| {
            let mosaic = field.mosaic(FRAME, cfa_type);
            let config = CosmicRayConfig::default();
            b.iter_batched_ref(
                || mosaic.clone(),
                |mosaic| reject_cosmic_rays(mosaic, &config).unwrap(),
                fixture::fresh_copies(size_of_val(mosaic.data.pixels())),
            );
        });
    }
}
