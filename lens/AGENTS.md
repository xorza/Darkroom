# lens

Node-function library: owns application-level node functions, including
filesystem watching and random generation, and adapts `imaginarium` (GPU image
ops) **and** `lumos` (astronomical processing) into the `scenarium` workflow.

## Layout

Image, astro, and utility domains are folders; the shared config bridge stays at
the root. `lib.rs` publishes the four library builders, `Image`, and the ML model
default configuration. Node execution never reads process-global configuration.

```
src/
├── lib.rs              published surface
├── config_node.rs      shared Introspect → config-builder bridge
├── utility/
│   ├── fs_watch/       filesystem watcher node + tests
│   └── random/         random-number node + tests
├── image/              imaginarium adapter and shared Image wire value
│   ├── mod.rs · context.rs · format.rs
│   ├── codec/          disk-cache codec + tests
│   └── nodes/          assembly, I/O nodes, processing nodes, tests
└── astro/              lumos adapter
    ├── masters.rs      Masters (CalibrationMasters CustomValue)
    ├── config/         builder registration, mirrors, and preset resolver
    └── nodes/          assembly + I/O/calibration/stacking/processing/ML/runtime
```

| Module | Role |
|--------|------|
| `image/mod.rs` | `Image` — `imaginarium::ImageBuffer` as a `CustomValue` (thumbnail via the fused `imaginarium::Preview` op: `make_cpu` + one-pass downscale→RGBA8). |
| `image/nodes/` | `image_library()` assembly; standard I/O is offloaded from the async worker and in-memory operations share `VisionCtx`. |
| `image/{context,format}.rs` | CPU `ProcessingContext` and conversion-format metadata. |
| `image/codec/` | Exact packed-pixel disk-cache codec. |
| `astro/mod.rs` | Astro-domain module declarations. Scientific results are converted into the shared `Image` wire type. |
| `astro/nodes/` | `astro_library(&MlModelPaths)` assembly plus cohesive node/runtime modules. |
| `astro/masters.rs` | `Masters` — `lumos::CalibrationMasters` as a `CustomValue`. |
| `astro/config/` | Stacking/processing mirrors, config-builder registration, and the common preset input/resolution contract. |
| `config_node.rs` | Scenarium bridge over `common`'s struct introspection: `config_builder_func::<T: NodeConfig>()` maps a `common::Introspect` type's `FieldDesc`s → a `Func` with one input per field (`FieldKind`→`DataType`, `FieldValue`↔`StaticValue`/`DynamicValue`) → a wireable `ConfigValue<T>`. `NodeConfig` = `Introspect` + a stable wire `TYPE_ID`/`NAME`; checked numeric reconstruction failures surface through the node invocation error path. Inputs required unless the field is `Option<_>`; enum type IDs come from each `IntrospectEnum`'s explicit UUID identity. Also home of the shared `enum_input` FuncInput helper. |

## Key types

- `image_library()` — builds a `Library` of the imaginarium image nodes.
- `astro_library(&MlModelPaths)` — builds the lumos nodes and seeds their explicit ONNX model inputs.
- `configure_ml_model_defaults()` — replaces only the two ML declarations so preference edits affect newly-authored nodes without introducing runtime-global inputs.
- `fs_watch_library()` — builds the directory watcher node library.
- `random_library()` — builds the random-number node library.
- `Image` — wrapper around `imaginarium::ImageBuffer` implementing `CustomValue`; `gen_preview` reads a CPU view (`make_cpu` — no-op on CPU, download on GPU) and builds the thumbnail with the fused `imaginarium::Preview` op (area-average downscale + RGBA8 convert in one pass).
- `Masters` — wrapper around `lumos::CalibrationMasters`.
- `VisionCtx` — context holding a `ProcessingContext` for GPU/CPU dispatch.
- `ConversionFormat` — enum of the 12 color-format conversion targets.
- Lazy-initialized type handles: `IMAGE_DATA_TYPE`, `MASTERS_DATA_TYPE`, `ASTRO_IMAGE_PATH_DATA_TYPE` (file picker filtered by Lumos's authoritative FITS/RAW/standard extension set), `ASTRO_DIR_DATA_TYPE` (camera-RAW frame-folder picker), `BLENDMODE_DATATYPE`, `CONVERSION_FORMAT_DATATYPE`, `VISION_CTX_TYPE`. (The astro presets have no standalone datatype handles — every preset node is a config-typed input with `value_variants`.)

## Functions

| Function | Role |
|----------|------|
| `brightness_contrast` | Adjust brightness and contrast. |
| `transform` | Affine transform (translate, scale, rotate). |
| `save_image` | Write image to file. |
| `convert` | Convert image to a different color format (enum input). |
| `blend` | Blend two images with configurable mode and alpha. |
| `load_astro_image` | Decode a FITS/RAW/standard file into the shared `Image` wire type (off-thread). |
| `build_masters` | Scan sorted camera-RAW paths from `darks`/`flats`/`bias`/`flat_darks` folders and stack them into `Masters` off-thread. Each cached `master_<role>.fits` has a `.source` marker keyed by the sorted RAW names, sizes, and mtimes; changed/removed/added frames or an unreadable FITS force a rebuild, while an empty set is cached explicitly as absent. |
| `stack_lights` | Scan sorted camera-RAW paths, then calibrate + align + stack a `lights` folder (+ optional `Masters`) into shared `Image` outputs (`calibrate_align_stack`, off-thread). Missing/unreadable folders are scan errors; a readable folder with no RAW frames is reported separately. Each of detection/registration/combine is **one required** config-typed input: a preset quick-pick (`value_variants` dropdown, seeded to the first) that a `build_*_config` node can wire into to override. Inputs with a default/variants are **required + seeded** (not optional with a hidden fallback) — clearing one is a missing input (errored run, highlighted port), never a silent default. `masters` is the only genuinely-optional input (absent = no calibration). Both `stack_lights` and `build_masters` clone `ctx.cancel_flag()` into their `spawn_blocking` and pass it to lumos via the shared `run_cancellable` helper, so a cancelled run bails the stack/master build cooperatively. Both are **`Pure`** and keyed by the contents of their input folders. |
| `auto_stretch` | Display-stretch an `Image`: a `method` value_variants quick-pick (`StretchPreset`), overridable by `build_stretch_config` → `lumos::Stretch::apply`, off-thread. |
| `background_extract` / `denoise` / `scnr` / `neutralize_background` / `hdr_compress` / `local_contrast` | Per-frame `Image → Image` processing through lumos in-place ops, run off-thread. `background_extract`/`scnr` take a unified value-variants preset input; `denoise`/`hdr_compress`/`local_contrast` keep an inline scalar plus an optional config override. |
| `ml_denoise` / `remove_stars` | ONNX nodes with a required `Model` `FsPath` input. The preferences seed that authored value, and Scenarium stamps the selected model file into the pure-node cache key. Inference runs off-thread via `run_ml`. |
| `build_background_config` / `build_detection_config` / `build_registration_config` / `build_combine_config` / `build_denoise_config` / `build_hdr_config` / `build_local_contrast_config` / `build_stretch_config` / `build_scnr_config` | Config-builder nodes generated by the shared bridge. Preset/config resolution is centralized in `astro/config/preset.rs`; mirror types live in the stacking and processing config modules. |

## Dependencies

common, scenarium, imaginarium, lumos, anyhow, strum, strum_macros, tracing,
tokio, async-trait, notify, rand, blake3.
