//! The candidates of a cluster's detection plane, as the detect stage extracts them from its
//! labelled components on the global pool: every component deblended, and the regions within the
//! configured area and clear of the edge kept. By the multi-threshold tree the crowded-field preset
//! picks, and by local maxima, the default, under the preset's other settings.

use std::cell::OnceCell;

use criterion::{BenchmarkId, Criterion, Throughput};

use crate::bench::fixture::{self, Scale};
use crate::bench::scene::{FRAME, Field};
use crate::concurrency::job_scratch_pool::JobScratchPool;
use crate::star_detection::bench::Stages;
use crate::star_detection::labeling::LabelMap;
use crate::star_detection::labeling::labeler::Labeler;
use crate::star_detection::star_detection_config::extraction_config::ExtractionConfig;
use crate::star_detection::star_detector::stages::detect::extract_candidates;

pub(crate) fn benches(c: &mut Criterion) {
    let field = Field::Dense;
    let crowded = Stages::config(field).extraction;
    let labelled = OnceCell::new();
    let mut group = fixture::group(c, "kernel/detection/deblend/frame", Scale::Frame);
    group.throughput(Throughput::Elements(FRAME.pixel_count() as u64));
    for (name, deblend) in [
        ("local_maxima", ExtractionConfig::default().deblend),
        ("multi_threshold", crowded.deblend),
    ] {
        let config = ExtractionConfig {
            deblend,
            ..crowded.clone()
        };
        group.bench_function(BenchmarkId::new(name, field.name()), |b| {
            let Labelled { stages, labels } = labelled.get_or_init(|| {
                let stages = Stages::of(field);
                let labels = Labeler::default().label(&stages.mask, crowded.connectivity);
                Labelled { stages, labels }
            });
            let buffers = JobScratchPool::default();
            b.iter(|| extract_candidates(&stages.plane, labels, &config, &buffers));
        });
    }
}

/// A field's detection stages and the components of its mask.
#[derive(Debug)]
struct Labelled {
    stages: Stages,
    labels: LabelMap,
}
