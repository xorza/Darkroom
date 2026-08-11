# Issues

`io::raw::bench::bench_bayer_rcd_demosaic` and `bench_bayer_rcd_quality_vs_libraw` panic with "No
Bayer test file found in test_data/raw_samples/" when the sample data is absent, rather than being
gated behind the `real-data` feature the way the other dataset-dependent benches are. A full
`::bench::` run reports two failures on any machine without the dataset.
