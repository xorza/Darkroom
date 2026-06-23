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
| `astro/presets.rs` | `preset_enum!` macro + preset enums → lumos stage configs. Each enum gives a `variant_names()` list (the node's `value_variants` quick-pick) + `FromStr`/`config()`. No `DataType` handles — every preset node is a config-typed input with a `build_*_config` override. |
| `config_node.rs` | Scenarium bridge over `common`'s struct introspection: `config_builder_func::<T: NodeConfig>()` maps a `common::Introspect` type's `FieldDesc`s → a `Func` with one input per field (`FieldKind`→`DataType`, `FieldValue`↔`StaticValue`/`DynamicValue`) → a wireable `ConfigValue<T>`. `NodeConfig` = `Introspect` + a stable wire `TYPE_ID`/`NAME`. Inputs required unless the field is `Option<_>`; enum `type_id`s via `common::FnvHasher`. Also home of the shared `enum_input` FuncInput helper. |
| `astro/configs.rs` | Lens-side editable **mirror** structs of lumos configs (e.g. `BackgroundConfigDef`) deriving `common::Introspect` (so lumos needn't) + `impl NodeConfig` + `From`/`Into` the lumos type. Mirror enums (`BackgroundModeDef`) get `common::IntrospectEnum` from `#[derive(IntrospectEnum)]` (which delegates to their strum `Display`/`EnumString`). `From<lumos::X>` gives the mirror's `Default`; `From<Mirror> for lumos::X` is compile-checked against the lumos struct. |

## Key types

- `image_funclib()` — builds a `FuncLib` of the imaginarium image nodes.
- `astro_funclib()` — builds a `FuncLib` of the lumos astro nodes.
- `Image` — wrapper around `imaginarium::ImageBuffer` implementing `CustomValue` (GPU thumbnail preview).
- `AstroFrame` — wrapper around `lumos::AstroImage` implementing `CustomValue`; `gen_preview` builds a downscaled RGBA_U8 thumbnail on the CPU (planar channels sampled straight to RGBA, no display stretch — linear frames preview dark) and parks it in a `Slot`.
- `Masters` — wrapper around `lumos::CalibrationMasters`.
- `VisionCtx` — context holding a `ProcessingContext` for GPU/CPU dispatch.
- `ConversionFormat` — enum of the 12 color-format conversion targets.
- Lazy-initialized type handles: `IMAGE_DATA_TYPE`, `ASTRO_FRAME_DATA_TYPE`, `MASTERS_DATA_TYPE`, `ASTRO_IMAGE_PATH_DATA_TYPE` (file picker filtered to the FITS/RAW/standard extensions `from_file` loads), `ASTRO_DIR_DATA_TYPE` (frame-folder picker), `BLENDMODE_DATATYPE`, `CONVERSION_FORMAT_DATATYPE`, `VISION_CTX_TYPE`. (The astro presets have no standalone datatype handles — every preset node is a config-typed input with `value_variants`.)

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
| `stack_lights` | Calibrate + align + stack a `lights` folder (+ optional `Masters`) into `image`/`coverage`/`weight` `AstroFrame`s (`calibrate_align_stack`, off-thread). Each of detection/registration/combine is **one required** config-typed input: a preset quick-pick (`value_variants` dropdown, seeded to the first) that a `build_*_config` node can wire into to override. Inputs with a default/variants are **required + seeded** (not optional with a hidden fallback) — clearing one is a missing input (errored run, highlighted port), never a silent default. `masters` is the only genuinely-optional input (absent = no calibration). Both `stack_lights` and `build_masters` clone `ctx.cancel_flag()` into their `spawn_blocking` and pass it to lumos via the shared `run_cancellable` helper, so a cancelled run bails the stack/master build cooperatively: a lumos failure *while the flag is set* surfaces as `InvokeError::Cancelled` (propagated with `?`), which the executor turns into `Error::Cancelled` — the node's output is dropped (so it re-runs next time) and it's reported as cancelled, not a failure. A failure with the flag *unset* is a real error. |
| `auto_stretch` | Display-stretch an `AstroFrame`: a `method` value_variants quick-pick (`StretchPreset`), overridable by `build_stretch_config` → `lumos::Stretch::apply`, off-thread. |
| `background_extract` / `denoise` / `scnr` / `neutralize_background` / `hdr_compress` / `local_contrast` | Per-frame `AstroFrame → AstroFrame` processing (lumos in-place ops — op-named config structs `lumos::{ExtractBackground, Denoise, Scnr, NeutralizeBackground, Hdr, LocalContrast}` whose `.apply(&mut Image) -> Result<_, OpError>` runs via the `processing_func` + `run_frame_op` helpers, which surface an `OpError` as the node's error, off-thread). `background_extract`/`scnr` take a unified value_variants preset input (overridable by their build node); `denoise`/`hdr_compress`/`local_contrast` keep an inline scalar (`strength`/`amount`) **plus** an optional `config` override fed by their `build_*_config` node. |
| `star_detect` | Detect stars in an `AstroFrame` → star `count` (Int): a `detection` value_variants quick-pick (`DetectionPreset`) overridable by `build_detection_config` (`StarDetector`). |
| `build_background_config` / `build_detection_config` / `build_registration_config` / `build_combine_config` / `build_denoise_config` / `build_hdr_config` / `build_local_contrast_config` / `build_stretch_config` / `build_scnr_config` | Config-builder nodes (`config_builder_func::<…ConfigDef>`): a field-per-input node → a config value, wired into the matching node's config input. **Every** preset node (stack_lights' detection/registration/combine, background_extract, auto_stretch, scnr, star_detect) is the same shape — a `value_variants` quick-pick overridable by its build node. The scalar nodes (`denoise`/`hdr_compress`/`local_contrast`) instead keep an inline scalar + an optional `config` override (`config_override_input`). Mirrors live in `astro/configs.rs`. |
| `astro_to_image` | Bridge an `AstroFrame` → `Image` so the imaginarium image nodes can consume astro output. |

## Dependencies

common, scenarium, imaginarium, lumos, anyhow, strum, strum_macros, tracing, tokio, async-trait.
