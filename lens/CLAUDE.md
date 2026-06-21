# lens

Node-function library: adapts `imaginarium` (GPU image ops) **and** `lumos`
(astronomical processing) into the `scenarium` node-based workflow.

## Layout

Two domains as folders + a shared bridge at the root. Modules are private;
`lib.rs` publishes only `AstroFrame`, `Image`, `astro_funclib`, `image_funclib`
(everything else — config mirrors, presets, datatypes, the bridge — is
crate-internal).

```
src/
├── lib.rs              published surface (4 items)
├── config_node.rs      shared Introspect → config-builder bridge
├── image/              imaginarium adapter
│   ├── mod.rs          Image (CustomValue, async GPU thumbnail) + submodules
│   ├── funclib.rs      image_funclib() — category `image`
│   ├── blend_mode.rs · conversion_format.rs · vision_ctx.rs
└── astro/              lumos adapter
    ├── mod.rs          AstroFrame (CustomValue, CPU thumbnail) + submodules
    ├── funclib/        astro_funclib() — category `astro` (mod.rs + tests.rs)
    ├── configs.rs      mirror config structs (Introspect)
    ├── presets.rs      preset_enum! dropdown enums → lumos stage configs
    └── masters.rs      Masters (CalibrationMasters CustomValue)
```

| Module | Role |
|--------|------|
| `image/mod.rs` | `Image` — `imaginarium::ImageBuffer` as a `CustomValue` (async GPU thumbnail). |
| `image/funclib.rs` | `image_funclib()` — imaginarium nodes (category `image`). |
| `image/{blend_mode,conversion_format,vision_ctx}.rs` | `BlendMode`/`ColorFormat` datatypes; `VisionCtx` (GPU/CPU `ProcessingContext`). |
| `astro/mod.rs` | `AstroFrame` — `lumos::AstroImage` as a `CustomValue` (CPU thumbnail preview). |
| `astro/funclib/` | `astro_funclib()` — lumos nodes (category `astro`); tests in `tests.rs`. |
| `astro/masters.rs` | `Masters` — `lumos::CalibrationMasters` as a `CustomValue`. |
| `astro/presets.rs` | `preset_enum!` macro + dropdown enums → lumos stage configs. The macro's `datatype:` handle is optional — presets consumed only as `value_variants` (registration/combine/background) skip it; those rendered via `enum_input` (detection/stretch/scnr) keep it. |
| `config_node.rs` | Scenarium bridge over `common`'s struct introspection: `config_builder_func::<T: NodeConfig>()` maps a `common::Introspect` type's `FieldDesc`s → a `Func` with one input per field (`FieldKind`→`DataType`, `FieldValue`↔`StaticValue`/`DynamicValue`) → a wireable `ConfigValue<T>`. `NodeConfig` = `Introspect` + a stable wire `TYPE_ID`/`NAME`. Inputs required unless the field is `Option<_>`; enum `type_id`s via `common::FnvHasher`. Also home of the shared `enum_input` FuncInput helper. |
| `astro/configs.rs` | Lens-side editable **mirror** structs of lumos configs (e.g. `BackgroundConfigDef`) deriving `common::Introspect` (so lumos needn't) + `impl NodeConfig` + `From`/`Into` the lumos type. Mirror enums (`BackgroundModeDef`) impl `common::IntrospectEnum` via `strum`. `From<lumos::X>` gives the mirror's `Default`; `From<Mirror> for lumos::X` is compile-checked against the lumos struct. |

## Key types

- `image_funclib()` — builds a `FuncLib` of the imaginarium image nodes.
- `astro_funclib()` — builds a `FuncLib` of the lumos astro nodes.
- `Image` — wrapper around `imaginarium::ImageBuffer` implementing `CustomValue` (GPU thumbnail preview).
- `AstroFrame` — wrapper around `lumos::AstroImage` implementing `CustomValue`; `gen_preview` builds a downscaled RGBA_U8 thumbnail on the CPU (planar channels sampled straight to RGBA, no display stretch — linear frames preview dark) and parks it in a `Slot`.
- `Masters` — wrapper around `lumos::CalibrationMasters`.
- `VisionCtx` — context holding a `ProcessingContext` for GPU/CPU dispatch.
- `ConversionFormat` — enum of the 12 color-format conversion targets.
- Lazy-initialized type handles: `IMAGE_DATA_TYPE`, `ASTRO_FRAME_DATA_TYPE`, `MASTERS_DATA_TYPE`, `ASTRO_IMAGE_PATH_DATA_TYPE` (file picker filtered to the FITS/RAW/standard extensions `from_file` loads), `ASTRO_DIR_DATA_TYPE` (frame-folder picker), the dropdown preset datatypes `DETECTION_PRESET_DATATYPE` / `STRETCH_PRESET_DATATYPE` / `SCNR_METHOD_DATATYPE` (registration/combine/background are consumed via `value_variants`, so they have no datatype handle), `BLENDMODE_DATATYPE`, `CONVERSION_FORMAT_DATATYPE`, `VISION_CTX_TYPE`.

## Functions

| Function | Role |
|----------|------|
| `brightness_contrast` | Adjust brightness and contrast. |
| `transform` | Affine transform (translate, scale, rotate). |
| `save_image` | Write image to file. |
| `convert` | Convert image to a different color format (enum input). |
| `blend` | Blend two images with configurable mode and alpha. |
| `load_astro_image` | Decode a FITS/RAW/standard file into an `AstroFrame` (off-thread). |
| `build_masters` | Stack `darks`/`flats`/`bias`/`flat_darks` folders into `Masters` (off-thread, per-role via `stack_cfa_master` + `CalibrationMasters::from_images`). `cache` toggle (default on) writes each master as `master_<role>.lcm` next to its frames and reloads it next run instead of re-stacking (`CfaImage::save`/`load`). |
| `stack_lights` | Calibrate + align + stack a `lights` folder (+ optional `Masters`) into `image`/`coverage`/`weight` `AstroFrame`s (`calibrate_align_stack`, off-thread). Each of detection/registration/combine is **one required** config-typed input: a preset quick-pick (`value_variants` dropdown, seeded to the first) that a `build_*_config` node can wire into to override. Inputs with a default/variants are **required + seeded** (not optional with a hidden fallback) — clearing one is a missing input (errored run, highlighted port), never a silent default. `masters` is the only genuinely-optional input (absent = no calibration). |
| `auto_stretch` | Display-stretch an `AstroFrame` (`StretchPreset` dropdown → `lumos::stretch`, off-thread). |
| `background_extract` / `denoise` / `scnr` / `neutralize_background` / `hdr_compress` / `local_contrast` | Per-frame `AstroFrame → AstroFrame` processing (lumos in-place ops via the `processing_func` + `run_frame_op` helpers, off-thread). `denoise`/`hdr_compress`/`local_contrast` keep an inline scalar (`strength`/`amount`) **plus** an optional `config` override fed by their `build_*_config` node; `background_extract` takes the unified preset/config input. |
| `star_detect` | Detect stars in an `AstroFrame` → star `count` (Int) (`StarDetector`, `DetectionPreset` dropdown). |
| `build_background_config` / `build_detection_config` / `build_registration_config` / `build_combine_config` / `build_denoise_config` / `build_hdr_config` / `build_local_contrast_config` | Config-builder nodes (`config_builder_func::<…ConfigDef>`): a field-per-input node → a config value, wired into the matching node's config input. For preset nodes (`background_extract`; `stack_lights`' detection/registration/combine) it overrides the `value_variants` preset quick-pick. For scalar nodes (`denoise`/`hdr_compress`/`local_contrast`) it overrides the inline scalar via an optional `config` input (`config_override_input`). Mirrors live in `astro_configs.rs`. |
| `astro_to_image` | Bridge an `AstroFrame` → `Image` so the imaginarium image nodes can consume astro output. |

## Dependencies

common, scenarium, imaginarium, lumos, anyhow, strum, strum_macros, tracing, tokio, async-trait.
