//! The final fit over two whole catalogues from RANSAC's hypothesis: a similarity, and a
//! similarity with the SIP correction the precise and wide-field presets add.

use std::cell::OnceCell;

use criterion::{BenchmarkId, Criterion, Throughput};

use crate::bench::fixture::{self, Scale};
use crate::bench::scene::Field;
use crate::registration::bench::Pair;
use crate::registration::final_fit::{FinalFit, FitCatalogs};
use crate::registration::transform::TransformType;
use crate::registration::tuning;

pub(crate) fn benches(c: &mut Criterion) {
    let pair = OnceCell::new();
    let count = Field::Typical.stars();
    let mut group = fixture::group(c, "kernel/registration/final_fit", Scale::Sample);
    group.throughput(Throughput::Elements(count as u64));
    for (name, with_sip) in [("similarity", false), ("similarity_sip", true)] {
        group.bench_function(BenchmarkId::new(name, count), |b| {
            let pair: &Pair = pair.get_or_init(Pair::typical);
            let model = pair.model(TransformType::Similarity, with_sip);
            let catalogs = FitCatalogs::new(&pair.reference, &pair.target).expect("a catalogue");
            let hypothesis = pair.hypothesis(TransformType::Similarity);
            let radius = tuning::recovery_radius(pair.max_sigma());
            b.iter(|| {
                FinalFit::run(&catalogs, hypothesis, model, radius).expect("the pair registers")
            });
        });
    }
}
