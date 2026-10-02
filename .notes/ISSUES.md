# Issues

- `lumos/src/testing/real_data/milky_way.rs` `milky_way_best_pipeline` — on the stack the current real-data pipeline produces, the enhanced image's intensity minimum is `-0.50153935`, just past the test's `min > -0.5` display bound.
- `lumos/src/stacking/pipeline/tests/mem_budget_probe.rs` `pipeline_stack_budget_probe` — peak heap is 3573 MB against the 2048 MB budget across four stages; the probe reports that one stage's memory is not freed before the next.
- `lumos/src/stacking/star_detection/centroid/tests/convergence.rs` — two tests pass `fwhm_to_sigma(sigma)` (σ divided by 2.3548) to `refine_centroid` as `expected_fwhm`, where a Gaussian of that σ has FWHM σ·2.3548.
