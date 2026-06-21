# lens

Node-function library: adapts `imaginarium` (GPU image ops) **and** `lumos`
(astronomical processing) into the `scenarium` node-based workflow.

## Modules

| Module | Role |
|--------|------|
| `image_funclib.rs` | Image-processing node functions (imaginarium) + type definitions. |
| `vision_ctx.rs` | `VisionCtx` wrapping `imaginarium::ProcessingContext` for GPU/CPU dispatch. |
| `astro_funclib.rs` | Astro node functions (lumos), category `astro`. |
| `astro_frame.rs` | `AstroFrame` — `lumos::AstroImage` as a `CustomValue` (CPU thumbnail preview). |
| `masters.rs` | `Masters` — `lumos::CalibrationMasters` as a `CustomValue`. |
| `astro_presets.rs` | `preset_enum!` macro + `DetectionPreset`/`RegistrationPreset`/`CombinePreset` (dropdown enums → lumos stage configs). |

## Key types

- `ImageFuncLib` — function library exposing the imaginarium image nodes.
- `AstroFuncLib` — function library exposing the lumos astro nodes.
- `Image` — wrapper around `imaginarium::ImageBuffer` implementing `CustomValue` (GPU thumbnail preview).
- `AstroFrame` — wrapper around `lumos::AstroImage` implementing `CustomValue`; `gen_preview` builds a downscaled RGBA_U8 thumbnail on the CPU (planar channels sampled straight to RGBA, no display stretch — linear frames preview dark) and parks it in a `Slot`.
- `Masters` — wrapper around `lumos::CalibrationMasters`.
- `VisionCtx` — context holding a `ProcessingContext` for GPU/CPU dispatch.
- `ConversionFormat` — enum of the 12 color-format conversion targets.
- Lazy-initialized type handles: `IMAGE_DATA_TYPE`, `ASTRO_FRAME_DATA_TYPE`, `MASTERS_DATA_TYPE`, `ASTRO_IMAGE_PATH_DATA_TYPE` (file picker filtered to the FITS/RAW/standard extensions `from_file` loads), `ASTRO_DIR_DATA_TYPE` (frame-folder picker), `DETECTION_PRESET_DATATYPE` / `REGISTRATION_PRESET_DATATYPE` / `COMBINE_PRESET_DATATYPE`, `BLENDMODE_DATATYPE`, `CONVERSION_FORMAT_DATATYPE`, `VISION_CTX_TYPE`.

## Functions

| Function | Role |
|----------|------|
| `brightness_contrast` | Adjust brightness and contrast. |
| `transform` | Affine transform (translate, scale, rotate). |
| `save_image` | Write image to file. |
| `convert` | Convert image to a different color format (enum input). |
| `blend` | Blend two images with configurable mode and alpha. |
| `load_astro_image` | Decode a FITS/RAW/standard file into an `AstroFrame` (off-thread). |
| `build_masters` | Stack `darks`/`flats`/`bias`/`flat_darks` folders into `Masters` (`CalibrationMasters::from_files`, off-thread). |
| `stack_lights` | Calibrate + align + stack a `lights` folder (+ optional `Masters`, preset dropdowns) into `image`/`coverage`/`weight` `AstroFrame`s (`calibrate_align_stack`, off-thread). |
| `auto_stretch` | Display-stretch an `AstroFrame` (`StretchPreset` dropdown → `lumos::stretch`, off-thread). |
| `background_extract` / `denoise` / `scnr` / `neutralize_background` / `hdr_compress` / `local_contrast` | Per-frame `AstroFrame → AstroFrame` processing (lumos in-place ops via the `processing_func` + `run_frame_op` helpers, off-thread). |
| `star_detect` | Detect stars in an `AstroFrame` → star `count` (Int) (`StarDetector`, `DetectionPreset` dropdown). |
| `astro_to_image` | Bridge an `AstroFrame` → `Image` so the imaginarium image nodes can consume astro output. |

## Dependencies

common, scenarium, imaginarium, lumos, anyhow, strum, strum_macros, tracing, tokio, async-trait.
