# Issues

- `lumos/src/testing/real_data/milky_way.rs` `milky_way_best_pipeline` — on the stack the current real-data pipeline produces, the enhanced image's intensity minimum is `-0.50153935`, just past the test's `min > -0.5` display bound.
- `lumos/src/stacking/pipeline/tests/mem_budget_probe.rs` `pipeline_stack_budget_probe` — peak heap is 3573 MB against the 2048 MB budget across four stages; the probe reports that one stage's memory is not freed before the next.
- fits-well `Wcs::from_header` probes every `PCi_j`/`CDi_j` pair the axis count allows, so a header with only `WCSAXES = 999` costs about 10⁶ keyword lookups: 0.16 s in release and 1.2 s in a debug build.
- darkroom `gui::app::session::tests::a_settled_frame_records_without_allocating` fails on the current tree: "settled frame 0 performed 1 heap operations".
- darkroom `gui::app::session::tests::a_chord_and_a_click_on_one_frame_both_reach_the_app` fails on the current tree: the frame yields `[Run(Once)]`; the Ctrl+S `Save` command is missing.
