//! The deblend of every component of a cluster's detection plane, one after another on one set of
//! buffers, as each of the detector's tasks runs its share: by local maxima, the default, and by
//! the multi-threshold tree the crowded-field preset picks.

use std::cell::OnceCell;

use criterion::{BenchmarkId, Criterion, Throughput};

use crate::bench::fixture::{self, Scale};
use crate::bench::scene::{FRAME, Field};
use crate::star_detection::bench::Stages;
use crate::star_detection::config::detection_config::{Deblend, DetectionConfig};
use crate::star_detection::deblend::component::Component;
use crate::star_detection::deblend::deblend_buffers::DeblendBuffers;
use crate::star_detection::deblend::local_maxima::LocalMaximaParams;
use crate::star_detection::deblend::multi_threshold::MultiThresholdParams;
use crate::star_detection::labeling::LabelMap;
use crate::star_detection::labeling::labeler::Labeler;

pub(crate) fn benches(c: &mut Criterion) {
    let field = Field::Dense;
    let labelled = OnceCell::new();
    let labelled = || {
        labelled.get_or_init(|| {
            let stages = Stages::of(field);
            let labels =
                Labeler::default().label(&stages.mask, stages.config.detection.connectivity);
            Labelled { stages, labels }
        })
    };
    let mut group = fixture::group(c, "kernel/detection/deblend/frame", Scale::Frame);
    group.throughput(Throughput::Elements(FRAME.pixel_count() as u64));
    group.bench_function(BenchmarkId::new("local_maxima", field.name()), |b| {
        let Labelled { stages, .. } = labelled();
        let config = &stages.config.detection;
        let Deblend::LocalMaxima { min_prominence } = DetectionConfig::default().deblend else {
            unreachable!("the default configuration deblends by local maxima")
        };
        let params = LocalMaximaParams {
            min_separation: config.deblend_min_separation,
            min_prominence,
        };
        let (mut buffers, mut regions) = (DeblendBuffers::default(), Vec::new());
        b.iter(|| {
            regions.clear();
            for component in labelled().components(config) {
                params.deblend(&component, &mut buffers, &mut regions);
            }
        });
    });
    group.bench_function(BenchmarkId::new("multi_threshold", field.name()), |b| {
        let Labelled { stages, .. } = labelled();
        let config = &stages.config.detection;
        let Deblend::MultiThreshold {
            n_thresholds,
            min_contrast,
        } = config.deblend
        else {
            unreachable!("the crowded-field preset deblends by the multi-threshold tree")
        };
        let params = MultiThresholdParams {
            n_thresholds,
            min_contrast,
            min_separation: config.deblend_min_separation,
            min_area: config.min_area,
            connectivity: config.connectivity,
        };
        let (mut buffers, mut regions) = (DeblendBuffers::default(), Vec::new());
        b.iter(|| {
            regions.clear();
            for component in labelled().components(config) {
                let floor = stages
                    .plane
                    .noise
                    .threshold_at(component.peak().pos, config.sigma_threshold);
                params.deblend(&component, floor, &mut buffers, &mut regions);
            }
        });
    });
}

/// A field's detection stages and the components of its mask.
#[derive(Debug)]
struct Labelled {
    stages: Stages,
    labels: LabelMap,
}

impl Labelled {
    /// The components the detector deblends: those of at least `min_area` pixels.
    fn components<'a>(
        &'a self,
        config: &'a DetectionConfig,
    ) -> impl Iterator<Item = Component<'a>> + 'a {
        self.labels
            .components()
            .iter()
            .filter(|data| data.area >= config.min_area.max(1))
            .map(|data| Component::new(data, &self.stages.plane.values, &self.labels))
    }
}
