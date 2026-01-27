# palantir - Implementation Notes (AI)

Image processing function library adapting imaginarium operations to the node-based workflow.

## Key Modules

| Module | Description |
|--------|-------------|
| `image_funclib.rs` | Image processing functions and type definitions |
| `vision_ctx.rs` | `VisionCtx` with `ProcessingContext` for GPU/CPU image operations |

## Key Types

```rust
ImageFuncLib       // Function library with image processing nodes
Image              // Wrapper around imaginarium::ImageBuffer implementing CustomValue
VisionCtx          // Context holding imaginarium::ProcessingContext for GPU/CPU dispatch
ConversionFormat   // Enum for all 12 color format conversion targets
IMAGE_DATA_TYPE    // Lazy-initialized custom DataType for images
BLENDMODE_DATATYPE // Lazy-initialized enum DataType for blend modes
CONVERSION_FORMAT_DATATYPE // Lazy-initialized enum DataType for conversion formats
VISION_CTX_TYPE    // Lazy-initialized ContextType for context manager
```

## Current Functions

| Function | Description |
|----------|-------------|
| `brightness_contrast` | Adjusts image brightness and contrast |
| `transform` | Applies affine transformations (translate, scale, rotate) |
| `save_image` | Saves image to file |
| `convert` | Converts image to different color format (enum input) |
| `blend` | Blends two images with configurable mode and alpha |

## Dependencies

common, scenarium, imaginarium, anyhow, strum, strum_macros
