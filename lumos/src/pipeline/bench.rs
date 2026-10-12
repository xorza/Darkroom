//! A calibrated light parked on the spill tier between its detection and its registration, then
//! warped straight from its maps into the warp's buffers, as a run that spills parks every light
//! while it chooses the reference: every page of the light in the page cache.

use common::TempDir;
use criterion::{BenchmarkId, Criterion, Throughput};

use crate::bench::fixture::{self, Scale};
use crate::bench::scene::{FRAME, Field};
use crate::frame_store::run_scratch::RunScratch;
use crate::frame_store::stored_image::StoredImage;
use crate::registration::bench::Pair;
use crate::registration::registration_config::WarpParams;
use crate::registration::resample::WarpBuffers;
use crate::registration::resample::source_image::{SourceImage, SourcePlane};

pub(crate) fn benches(c: &mut Criterion) {
    let field = Field::Typical;
    let mut group = fixture::group(c, "stage/park", Scale::Frame);
    group.throughput(Throughput::Elements(FRAME.pixel_count() as u64));
    group.bench_function(BenchmarkId::new("warp_from_map/warm", field.name()), |b| {
        let dir = TempDir::new("lumos-bench-park");
        let scratch = RunScratch::create(dir.path()).unwrap();
        let light = field.render(FRAME).image;
        let parked = StoredImage::spill(&scratch, &light).unwrap();
        let flags = parked.flags();
        let source = SourceImage {
            dimensions: parked.dimensions,
            planes: parked
                .planes()
                .map(|pixels| SourcePlane {
                    pixels,
                    width: FRAME.width,
                })
                .collect(),
            flags: flags.as_ref(),
        };
        let warp = Pair::typical().similarity_warp();
        let mut buffers = WarpBuffers::new(parked.dimensions);
        b.iter(|| buffers.warp_into(&source, &warp, WarpParams::default()));
    });
}
