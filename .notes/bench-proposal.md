# lumos bench inventory: proposal

Status: proposal, every decision in section 6 taken; no code written. Scope: every bench in `lumos`.

## Items

Done in this order; each is deleted when it is committed. An item that builds a group also removes
the quickbench benches that section 5 maps to it, and adds the `bench::scene` entries (3.2) it is
the first to need; the `Pipeline` scale comes with the `pipeline` tier.

4. `kernel/frame_store`: only `spill/read/cold` is left, blocked on
   [a question](bench-proposal_QUESTIONS.md).
9. The `stage` tier.
10. The `pipeline` tier.
11. quickbench removed from `lumos`: the dev-dependency, any bench left, every real-data bench
    (3.3), and the AGENTS.md text.

## 1. Audit of the current inventory

### 1.1 Counts

| Harness | Files | Benches | Run with |
|---|---|---|---|
| quickbench (`#[test] #[ignore]`) | 29 | 134 (about 200 rows with labels) | `cargo test --release --features bench <filter> -- --ignored` |
| criterion (`benches/lumos.rs`) | 11 (`math` only) | 160 | `cargo bench --features bench` |
| `Instant` timing inside a test | 1 (`internals/real_data/pipeline_bench.rs`) | 1 | `cargo test --features real-data` |

### 1.2 Problems

1. **Two harnesses and three run commands.** Results are not comparable between them, and the
   quickbench results are not saved as baselines.
2. **No naming rule.** Names mix sizes and scenes in free text: `bench_detect_6k_globular_cluster`,
   `bench_warp_lanczos3_1k_single_thread`, `bench_label_small`. Labels are free text too
   (`"map 90°"`, `"frame-clone"`, `"ours"`).
3. **No size rule.** The inputs are 256², 1000², 1024², 1500², 2000×1500, 2048², 4096², 6000×4000
   and 6144², with no reason given. Most are not a production frame size.
4. **No sampling rule.** Some benches use time windows (100–5000 ms), and some use iteration counts
   (`iters = 1`, 3, 5, 10, 12).
5. **Overlaps.**
   - Warp: 13 benches in `registration/resample` (Lanczos3 at 1k, 2k and 4k, a single-thread
     copy, 4 other methods, the quality planes, fresh against reused, RGB with SIP or a
     homography), and `pipeline::bench_parked_warp_2k` warps again.
   - Star fits: `centroid/bench.rs` has `measure_star_gaussian_fit` and
     `measure_star_moffat_fit`, and `gaussian_fit/bench.rs` and `moffat_fit/bench.rs` bench the
     same fits again at three sizes.
   - Background: `background/bench_background_estimate_6k` and `background_mesh/bench_tile_grid_*`
     measure the same mesh.
   - Detection: 3 detector sizes, and `pipeline::bench_detector_batch_reuse_1k` detects again.
   - Labeling: 4 benches, at 1k, at 4k, on a globular cluster, and a "small" one.
   - Deblend: 6 benches, three for each method, at 4k and 6k.
6. **Experiments kept as benches.** These compare two ways of doing one thing, where production
   uses only one way:
   - `ours` against six LibRaw demosaic levels;
   - LibRaw `open_file` against `open_buffer`;
   - a parked frame read from its `map` against an `owned` copy;
   - a `fresh` detector against a `reused` one;
   - `fresh` against `reused` warp planes;
   - `row_vs_col` and `elliptical_vs_circular` convolution;
   - a `frame-clone` row that times the copy a consuming API needs;
   - the `single_thread` warp and stretch.
7. **Inputs that are not production work.**
   - Calibration, FITS decode and the defect map use uniform random noise, not a sky with stars.
   - The warp uses a diagonal gradient with no stars, flags or noise.
   - Registration detects on a 1500² field.
   - Cosmic-ray rejection runs on one 1 MP frame.
   - Combine stacks 30 frames of 1 MP, or 300 frames of 256².
8. **Undocumented.** About 70 of the 134 benches have no doc line, so the reader cannot tell what
   they measure or why at that size.
9. **Real-data benches are scattered** over five places:
   - `image_ops/bench.rs`;
   - `io/raw/bench.rs`;
   - `registration/bench.rs::real_data`;
   - `calibration_masters/tests/real_data.rs`;
   - `internals/real_data/pipeline_bench.rs`.

   Two of them are timed with `Instant` inside a test.
10. **Gaps: hot paths with no bench.**
    - Frame statistics, which run for every frame.
    - Normalization.
    - The per-pixel rejection kernels, which are reached only through a whole stack.
    - The spill tier's write and read.
    - The quality planes.
    - The prepared flat.
    - Flat gain and its warp.
    - Triangle matching, RANSAC and the SIP fit.
    - The k-d tree.
    - The drizzle accumulator.
    - Color calibration.
    - The FITS CFA decode.
    - An end-to-end stack of files through the public API.

## 2. Rules every bench follows

1. **One harness.** Every bench is criterion, in the one target `benches/lumos.rs`. quickbench is
   removed from `lumos`: the dev-dependency, every `#[quick_bench]` function, and the AGENTS.md
   text.
2. **Three tiers, and each bench is in exactly one.**

   | Tier | Measures | Input | Scale |
   |---|---|---|---|
   | `kernel` | One function or kernel, one operation. | The unit production gives it: a tile, a stamp, a row, a pixel's stack of frames, a star list. | `Sample` |
   | `stage` | One pipeline stage on one production frame or set. | A 24 MP frame (see 3.1), or a set of them. | `Frame` |
   | `pipeline` | A public entry point end to end, from files. | A set of light, dark, flat and bias files. | `Pipeline` |

3. **One naming rule.** Group `<tier>/<area>/<module>`, bench id `<operation>/<variant>`,
   parameter the input (a size, a scene, a count). Example:
   `kernel/detection/centroid/fit/moffat/stamp_31`. `cargo bench -- kernel/detection` selects a
   subtree.
4. **One sampling policy for each scale**, set in one function (`fixture::group`, as now). The
   `Pipeline` scale is new: 10 samples and flat sampling, for iterations of seconds.
5. **Throughput** is the elements one iteration processes: pixels, stars, star pairs, or frames ×
   pixels.
6. **Variants are production choices only.** A variant exists where a user or the planner picks it:
   the warp method, the rejection algorithm, the drizzle kernel, the CFA pattern, the
   resident or spilled tier. Two exceptions: the SIMD tiers (`scalar`, each tier, `dispatch`),
   because they set the gates, and `cold` reads of spilled frames on Linux, where eviction is
   possible. A comparison against another library or an old method is an experiment: it goes into
   a branch, not into the standing inventory.
7. **Threads as in production.** A function that production calls inside an outer parallel loop
   (per star, per tile, per pixel) is benched on one item, sequentially. A function that is
   parallel itself is benched on the global pool. No `single_thread` variants.
8. **Setup outside the timing.** Inputs are built inside the bench closure, so a filter skips them.
   A consuming or in-place API gets a fresh copy through `iter_batched` with `fixture::in_place`, so
   no bench reports a copy as a row.
9. **A doc line on every bench**: what it measures, at what input, and why that input is the
   production one.

## 3. Inputs

### 3.1 Standard sizes

- **Frame:** 6000 × 4000 (24 MP), an APS-C camera's frame and the size the black-level, defect-map
  and FITS benches already use. Every `stage` bench uses it.
- **Kernel units:** taken from production configuration, not chosen: background tile 64², fit stamp
  15 and 31 px (a tight star and a wide one), a combine pixel of 30 and 300 frames, a row of
  6000, a star list of 500 and 3000.
- **Set:** 30 lights, the light preset's typical stack; 12 for the `pipeline` tier, so an
  iteration stays under about 10 s.

### 3.2 Scenes

One shared, seeded catalogue, `bench::scene`, built on the generators in `internals::synthetic`:

| Scene | Content |
|---|---|
| `sky` | Sky with a gradient, photon and read noise, a few hot pixels. |
| `sparse` | `sky` with 300 stars, FWHM 3.5 px. |
| `typical` | `sky` with 3000 stars, a field at moderate latitude. |
| `dense` | `sky` with a globular cluster: about 20 000 stars, with blends. |
| `*_rggb`, `*_xtrans` | The same scene as a mosaic, with cosmic-ray hits. |
| `lights(n)` | `n` exposures of `typical` at dithers of a few pixels and 0.2° of rotation, with per-frame sky and gain changes. |
| `masters` | Bias, dark and flat that match the lights. |
| `stacked_rgb` | A synthetic linear RGB master, for the image operations. |

### 3.3 Real data

No bench reads the real-data set: every bench runs on the synthetic scenes of 3.2. The current
real-data benches and the `Instant`-timed real-data test are removed.

### 3.4 What the code needs for this

`bench` must be able to use `internals::synthetic`. That module is test-only now, and parts of
`internals` use dev-only crates (`image`). So: gate `internals` on `any(test, feature =
"internals")`, set `bench = ["internals", "dep:criterion"]` as your guide says, and make the crates
`internals` needs optional dependencies that `internals` enables.

## 4. Proposed inventory

`F` = the 24 MP frame. The tiers go `kernel`, then `stage`, then `pipeline`.

### 4.1 kernel

| Group | Bench ids (`operation/variant/input`) |
|---|---|
| `kernel/math/*` | As now (160 benches), unchanged except the tier prefix. |
| `kernel/io/raw` | `decode/rggb/F` (samples and flags past LibRaw's unpack, from a synthetic 14-bit buffer). |
| `kernel/io/demosaic` | `rcd/rggb/F`, `markesteijn/xtrans_1pass/F`, `markesteijn/xtrans_3pass/F`. |
| `kernel/io/fits` | `decode/u16/F`, `decode/f32/F`, `decode/cfa_u16/F` (from a file in the page cache). |
| `kernel/calibration/apply` | `light/rggb/F` (bias, dark, flat and defects on one light). |
| `kernel/calibration/defect_map` | `build/rggb/F`, `color_sample_indices/rggb/F`. |
| `kernel/calibration/cosmic_ray` | `reject/mono/F`, `reject/rggb/F`. |
| `kernel/calibration/prepared_flat` | `prepare/rggb/F`. |
| `kernel/frame_store/frame_stats` | `measure/mono/F`, `measure/rggb/F`, `measure_split/rggb/F` (with a flat gain). |
| `kernel/frame_store/spill` | `write/F`, `read/warm/F`, `read/cold/F` (Linux only). |
| `kernel/detection/background` | `tile_stats/tile_64`, `mesh/typical/F`, `mesh/dense/F`. |
| `kernel/detection/median_filter` | `row/<tier>/6000` (scalar, each tier, dispatch). |
| `kernel/detection/threshold_mask` | `row/<tier>/6000`. |
| `kernel/detection/convolution` | `row/<tier>/6000` at the default FWHM, `gaussian/typical/F`, `matched_filter/typical/F`, `elliptical/typical/F`. |
| `kernel/detection/dilation` | `mask/typical/F`. |
| `kernel/detection/labeling` | `label/typical/F`, `label/dense/F`. |
| `kernel/detection/deblend` | `local_maxima/dense/F`, `multi_threshold/dense/F`. |
| `kernel/detection/centroid` | `moments/stamp_15`, `annulus_background/stamp_31`. |
| `kernel/detection/fit` | `gaussian/stamp_15`, `gaussian/stamp_31`, `moffat/stamp_15`, `moffat/stamp_31`, `moffat_free_beta/stamp_31`. |
| `kernel/detection/filter` | `remove_duplicates/3000`, `remove_duplicates/20000`. |
| `kernel/registration/spatial` | `kd_tree/build/3000`, `kd_tree/nearest/3000`. |
| `kernel/registration/triangle` | `match/500`, `match/3000`. |
| `kernel/registration/ransac` | `similarity/500`, `homography/500`. |
| `kernel/registration/sip` | `fit/500`. |
| `kernel/registration/row_positions` | `affine/6000`, `homography/6000`, `sip/6000`. |
| `kernel/combine/rejection` | `<algorithm>/30`, `<algorithm>/300`, over a block of 4096 pixels, for every algorithm the config offers (none, sigma clip, winsorized, linear fit, GESD, percentile, and the others). |
| `kernel/combine/normalization` | `multiplicative/F`, `global/F` (one frame against the reference). |
| `kernel/combine/quality_planes` | `standard/30/4096` (the standard planes over a pixel block). |
| `kernel/drizzle/accumulate` | `<kernel>/<geometry>/F`, the current sweep at the production frame. |

### 4.2 stage

| Group | Bench ids |
|---|---|
| `stage/ingest` | `fits_light/u16/F`, `fits_light/cfa_u16/F`. |
| `stage/calibrate` | `light/rggb/F` (apply, defects, cosmic rays, as configured by default). |
| `stage/demosaic` | `rggb/F`, `xtrans/F`. |
| `stage/statistics` | `light/rggb/F` (the frame statistics a light takes). |
| `stage/detect` | `sparse/F`, `typical/F`, `dense/F` (`StarDetector::detect`). |
| `stage/register` | `pair/typical` (two star lists to a transform). |
| `stage/warp` | `<method>/mono/F` for each method; `lanczos3/rgb/F`; `lanczos3/rgb_sip/F`; `lanczos3/rgb_quality/F`. |
| `stage/park` | `warp_from_map/warm/F`, `warp_from_map/cold/F` (Linux only). |
| `stage/combine` | `light_preset/resident/30xF`, `light_preset/spilled/30xF`, `<algorithm>/resident/30xF` for each rejection. |
| `stage/drizzle` | `default/30x{2048²}` (the drizzle of a set). |
| `stage/image_ops` | `<op>/stacked_rgb/F` for every op: STF, auto asinh, GHS, neutralize, denoise, SCNR, background extraction, HDR, local contrast, color calibration. |

### 4.3 pipeline

| Group | Bench ids |
|---|---|
| `pipeline/stack` | `fits_mono/12xF` (`lumos::stack` from FITS files). |
| `pipeline/masters` | `build/rggb/12xF` (`CalibrationMasters::build`, dark, flat and bias). |
| `pipeline/calibrate_align_stack` | `rggb/12xF/resident`, `rggb/12xF/spilled`. |
| `pipeline/drizzle` | `rggb/12xF`. |

## 5. Migration of the current benches

| Current | Becomes |
|---|---|
| `math/*` (criterion) | `kernel/math/*`, unchanged. |
| `background_mesh::bench_tile_grid_6k_{globular,with_mask}`, `background::bench_background_estimate_6k` | `kernel/detection/background/mesh/{typical,dense}/F`. One mesh, three duplicates removed. |
| `bit_buffer2::bench_dilate_mask_6k` | `kernel/detection/dilation/mask/typical/F`. |
| `calibration_masters::bench_calibrate_apply_bayer` | `kernel/calibration/apply/light/rggb/F`, on a sky with stars at the frame size. |
| `calibration_masters::bench_calibrate_30_lights_bayer` | `pipeline/calibrate_align_stack`. |
| `calibration_masters::bench_defect_map_build_bayer` | `kernel/calibration/defect_map/build/rggb/F`. |
| `calibration_masters::bench_cosmic_ray_reject_{mono,bayer}` | `kernel/calibration/cosmic_ray/reject/{mono,rggb}/F`. |
| `defect_map::bench_collect_color_sample_indices` | `kernel/calibration/defect_map/color_sample_indices/rggb/F`. |
| `calibration_masters::tests::real_data` (4 benches) | Removed (real data). |
| `combine::bench_stack_30`, `bench_stack_300` | `stage/combine/*/30xF` and `kernel/combine/rejection/*/{30,300}`. The `frame-clone` row is removed. |
| `combine::bench_spilled_mean_40` | `stage/combine/light_preset/spilled/30xF` and `kernel/frame_store/spill/read/{warm,cold}/F`. |
| `drizzle::bench_drizzle_kernels` | `kernel/drizzle/accumulate/<kernel>/<geometry>/F`. |
| `drizzle::bench_drizzle_quality_planes` | `stage/drizzle/default`. |
| `image_ops::bench_*` (9, real data) | `stage/image_ops/<op>/stacked_rgb/F` on synthetic data. |
| `stretching::bench_stretch_rgb` | `stage/image_ops/{stf,auto_asinh,ghs}/stacked_rgb/F`. |
| `stretching::bench_stretch_asinh_kernel_single_thread` | Removed (single-thread variant). |
| `fits::decode::bench_fits_decode_u16` | `kernel/io/fits/decode/u16/F`, on a sky with stars. |
| `io::raw::raw_load` | Removed (real data). |
| `io::raw::bench_demosaic_vs_libraw` | Removed (experiment). |
| `io::raw::bench_unpack_file_vs_buffer` | Removed (experiment; the buffer path is the production one). |
| `black_level::bench_raw_decode` | `kernel/io/raw/decode/rggb/F`. |
| `demosaic::bayer::bench_rcd_demosaic_core`, `bench_cfa_demosaic_balanced` | `kernel/io/demosaic/rcd/rggb/F` (the balanced path, which production runs). |
| `markesteijn::bench_markesteijn_demosaic` | `kernel/io/demosaic/markesteijn/xtrans_{1,3}pass/F`. |
| `pipeline::bench_detector_batch_reuse_1k` | Removed (experiment; production reuses). |
| `pipeline::bench_parked_warp_2k` | `stage/park/warp_from_map/{warm,cold}/F`; the `owned` variant is removed. |
| `pipeline::parked_lights_under_a_memory_cap` | Removed. |
| `registration::bench_register_{150,500}_stars` | `stage/register/pair/typical` and `kernel/registration/{triangle,ransac,sip}`. |
| `registration::real_data` (2) | Removed (real data). |
| `resample::bench_warp_*` (13) | `stage/warp/<method>/mono/F` (5 methods), `stage/warp/lanczos3/{rgb,rgb_sip,rgb_quality}/F`. The 1k, 2k and 4k copies, the single-thread copy and fresh against reused are removed. |
| `centroid::bench_measure_star_*`, `bench_compute_star_*`, `bench_windowed_*` (11) | `kernel/detection/centroid/{moments,annulus_background}/stamp_*` and `kernel/detection/fit/*`. The batches are removed: `stage/detect` covers a frame. |
| `gaussian_fit::bench_*` (3), `moffat_fit::bench_*` (3) | `kernel/detection/fit/{gaussian,moffat,moffat_free_beta}/stamp_{15,31}`. |
| `convolution::bench_*` (11) | `kernel/detection/convolution/{row/<tier>/6000, gaussian, matched_filter, elliptical}`. The 1k copies, `row_vs_col` and `elliptical_vs_circular` are removed. |
| `local_maxima::bench_*` (3), `multi_threshold::bench_*` (3) | `kernel/detection/deblend/{local_maxima,multi_threshold}/dense/F`. |
| `detector::bench_detect_*` (3) | `stage/detect/{sparse,typical,dense}/F`. |
| `filter::bench_remove_duplicate_stars_{5000,10000}` | `kernel/detection/filter/remove_duplicates/{3000,20000}`. |
| `labeling::bench_*` (4) | `kernel/detection/labeling/label/{typical,dense}/F`. |
| `median_filter::bench_median_filter_dispatch_vs_scalar` | `kernel/detection/median_filter/row/<tier>/6000`. |
| `threshold_mask::bench_threshold_mask_4k` | `kernel/detection/threshold_mask/row/<tier>/6000`. |
| `internals::real_data::pipeline_bench` | Removed (real data). |

Net result: about 134 quickbench benches become about 110 criterion benches outside `math`. That
removes about 40 duplicates and experiments and adds about 30 benches for the gaps.

## 6. Decisions

1. **The standard frame:** decided: 6000 × 4000 (24 MP).
2. **Memory of the `stage/combine` set:** decided: 30 × 24 MP, resident and spilled. The resident
   benches need a machine with 16 GB or more.
3. **`internals` for benches:** decided: an `internals` feature, `bench` enables it, and the crates
   `internals` needs become optional dependencies it enables (3.4).
4. **The cold-cache variants:** decided: kept, on Linux only, where the platform layer can evict;
   left out elsewhere, so no bench reports a warm read as cold.
5. **`parked_lights_under_a_memory_cap`:** decided: removed. The Linux cold variants measure the
   spill tier's disk reads, and the budget suite its memory.
