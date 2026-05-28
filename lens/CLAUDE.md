# lens

Image-processing function library: adapts `imaginarium` operations into the `scenarium` node-based workflow.

## Modules

| Module | Role |
|--------|------|
| `image_funclib.rs` | Image-processing node functions + type definitions. |
| `vision_ctx.rs` | `VisionCtx` wrapping `imaginarium::ProcessingContext` for GPU/CPU dispatch. |

## Key types

- `ImageFuncLib` — the function library exposing image nodes.
- `Image` — wrapper around `imaginarium::ImageBuffer` implementing `CustomValue`.
- `VisionCtx` — context holding a `ProcessingContext` for GPU/CPU dispatch.
- `ConversionFormat` — enum of the 12 color-format conversion targets.
- Lazy-initialized type handles: `IMAGE_DATA_TYPE`, `BLENDMODE_DATATYPE`, `CONVERSION_FORMAT_DATATYPE`, `VISION_CTX_TYPE`.

## Functions

| Function | Role |
|----------|------|
| `brightness_contrast` | Adjust brightness and contrast. |
| `transform` | Affine transform (translate, scale, rotate). |
| `save_image` | Write image to file. |
| `convert` | Convert image to a different color format (enum input). |
| `blend` | Blend two images with configurable mode and alpha. |

## Dependencies

common, scenarium, imaginarium, anyhow, strum, strum_macros.
