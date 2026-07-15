use std::str::FromStr;
use std::sync::Arc;

use crate::config_node::enum_input;
use crate::image::blend_mode::{BLENDMODE_DATATYPE, BLENDMODE_TYPE_ID};
use crate::image::codec::image_type_entry;
use crate::image::conversion_format::{
    CONVERSION_FORMAT_DATATYPE, CONVERSION_FORMAT_TYPE_ID, ConversionFormat,
};
use crate::image::vision_ctx::{VISION_CTX_TYPE, VisionCtx};
use crate::image::{IMAGE_DATA_TYPE, IMAGE_TYPE_ID, Image};
use imaginarium::{
    Blend, BlendMode, ColorFormat, ContrastBrightness, SUPPORTED_EXTENSIONS, Transform, Vec2,
};
use scenarium::FuncLambda;
use scenarium::{DataType, DynamicValue, FsPathConfig, FsPathMode, StaticValue};
use scenarium::{Func, FuncInput, FuncOutput};
use scenarium::{Library, TypeEntry};

/// The imaginarium image-processing nodes (category `image`).
pub fn image_library() -> Library {
    let mut library = Library::default();

    // brightness_contrast
    library.add(
        Func::new(
            "b8c3d4e5-f6a7-4b8c-9d0e-1f2a3b4c5d6e",
            "Brightness / Contrast",
        )
        .description("Adjusts the brightness and contrast of an image.")
        .category("Image")
        .pure()
        .context(VISION_CTX_TYPE.clone())
        .input(
            FuncInput::required("Image", IMAGE_DATA_TYPE.clone()).description("Image to adjust."),
        )
        .input(
            FuncInput::required("Brightness", DataType::Float)
                .description("Brightness offset in [−1, 1]. 0 leaves it unchanged.")
                .default(0.0),
        )
        .input(
            FuncInput::required("Contrast", DataType::Float)
                .description("Contrast multiplier. 1 leaves it unchanged.")
                .default(1.0),
        )
        .output(FuncOutput::new("Image", IMAGE_DATA_TYPE.clone()).description("Adjusted image."))
        .lambda(FuncLambda::new(
            move |ctx_manager, _, _, inputs, _, outputs| {
                Box::pin(async move {
                    assert_eq!(inputs.len(), 3);
                    assert_eq!(outputs.len(), 1);

                    let value = std::mem::take(&mut inputs[0].value);
                    let brightness = inputs[1].value.as_f64().unwrap() as f32;
                    let contrast = inputs[2].value.as_f64().unwrap() as f32;

                    let vision_ctx = ctx_manager.get::<VisionCtx>(&VISION_CTX_TYPE);

                    let image = adjust_image(
                        ContrastBrightness::new(contrast, brightness),
                        &mut vision_ctx.processing_ctx,
                        value,
                    )?;
                    outputs[0] = DynamicValue::from_custom(image);

                    Ok(())
                })
            },
        )),
    );

    // load_image
    library.add(
        Func::new("a4d9bf87-9d98-44f1-a162-7483c298be3d", "Load Image")
            .description("Loads an image from a file on disk.")
            .category("Image")
            .pure()
            .input(
                FuncInput::required("Path", image_fs_path(FsPathMode::ExistingFile))
                    .description("Image file to load."),
            )
            .output(FuncOutput::new("Image", IMAGE_DATA_TYPE.clone()).description("Loaded image."))
            .lambda(FuncLambda::new(move |_, _, _, inputs, _, outputs| {
                Box::pin(async move {
                    assert_eq!(inputs.len(), 1);
                    assert_eq!(outputs.len(), 1);

                    let path = inputs[0].value.as_fs_path().unwrap();
                    let image = imaginarium::Image::read_file(path).map_err(anyhow::Error::from)?;

                    outputs[0] = DynamicValue::from_custom(Image::from(image));

                    Ok(())
                })
            })),
    );

    // save_image
    library.add(
        Func::new("0c17bcbe-d757-43be-b184-27b429e8b434", "Save Image")
            .description("Writes an image to a file on disk.")
            .category("Image")
            .sink()
            .input(
                FuncInput::required("Image", IMAGE_DATA_TYPE.clone()).description("Image to save."),
            )
            .input(
                FuncInput::required("Path", image_fs_path(FsPathMode::NewFile))
                    .description("Destination file; the extension picks the container."),
            )
            .input(
                enum_input::<ConversionFormat>("Format", &CONVERSION_FORMAT_DATATYPE)
                    .default(StaticValue::Enum(ConversionFormat::AsIs.label()))
                    .description(
                        "Convert to this color format before saving; \"As Is\" keeps the source format.",
                    ),
            )
            .context(VISION_CTX_TYPE.clone())
            .lambda(FuncLambda::new(move |ctx_manager, _, _, inputs, _, _| {
                Box::pin(async move {
                    assert_eq!(inputs.len(), 3);

                    let input_image = inputs[0].value.as_custom::<Image>().unwrap();
                    let path = inputs[1].value.as_fs_path().unwrap();
                    let format_str = inputs[2].value.as_enum().unwrap();

                    let vision_ctx = ctx_manager.get::<VisionCtx>(&VISION_CTX_TYPE);
                    let cpu_image = input_image
                        .make_cpu(&vision_ctx.processing_ctx)
                        .map_err(anyhow::Error::from)?;

                    match conversion_target(format_str, cpu_image.desc.color_format) {
                        Some(format) => cpu_image
                            .convert_to(format)
                            .map_err(anyhow::Error::from)?
                            .save_file(path),
                        None => cpu_image.save_file(path),
                    }
                    .map_err(anyhow::Error::from)?;

                    Ok(())
                })
            })),
    );

    // convert
    library.add(
        Func::new("80aa1ee7-3b75-4200-b480-b9db913bd6eb", "Convert")
            .description("Converts an image to a different color format.")
            .category("Image")
            .pure()
            .context(VISION_CTX_TYPE.clone())
            .input(
                FuncInput::required("Image", IMAGE_DATA_TYPE.clone())
                    .description("Image to convert."),
            )
            .input(
                enum_input::<ConversionFormat>("Format", &CONVERSION_FORMAT_DATATYPE)
                    .default(StaticValue::Enum(ConversionFormat::RgbU8.label()))
                    .description("Target color format."),
            )
            .output(
                FuncOutput::new("Image", IMAGE_DATA_TYPE.clone()).description("Converted image."),
            )
            .lambda(FuncLambda::new(
                move |ctx_manager, _, _, inputs, _, outputs| {
                    Box::pin(async move {
                        assert_eq!(inputs.len(), 2);
                        assert_eq!(outputs.len(), 1);

                        let value = std::mem::take(&mut inputs[0].value);
                        let format_str = inputs[1].value.as_enum().unwrap();

                        let converted = {
                            let image = value
                                .as_custom::<Image>()
                                .expect("image input type is validated at the compile boundary");
                            match conversion_target(format_str, image.desc.color_format) {
                                Some(format) => {
                                    let vision_ctx = ctx_manager.get::<VisionCtx>(&VISION_CTX_TYPE);
                                    let cpu_image = image
                                        .make_cpu(&vision_ctx.processing_ctx)
                                        .map_err(anyhow::Error::from)?;
                                    Some(
                                        cpu_image
                                            .convert_to(format)
                                            .map_err(anyhow::Error::from)?,
                                    )
                                }
                                None => None,
                            }
                        };

                        outputs[0] = match converted {
                            Some(image) => DynamicValue::from_custom(Image::from(image)),
                            // "As Is", or already in the requested format: pass the input
                            // through untouched — the same value, zero copies, no forced
                            // CPU download. Sharing the Arc across slots is safe: any
                            // downstream in-place op sees a shared value and clones.
                            None => value,
                        };

                        Ok(())
                    })
                },
            )),
    );

    // blend
    library.add(
        Func::new("975cc74b-8412-4293-b2cb-ef8d41fdd9b3", "Blend")
            .description("Blends two images using the selected blend mode.")
            .category("Image")
            .pure()
            .context(VISION_CTX_TYPE.clone())
            .input(
                FuncInput::required("Source", IMAGE_DATA_TYPE.clone())
                    .description("Top image (the blend source)."),
            )
            .input(
                FuncInput::required("Destination", IMAGE_DATA_TYPE.clone())
                    .description("Bottom image (the blend backdrop)."),
            )
            .input(enum_input::<BlendMode>("Mode", &BLENDMODE_DATATYPE).description("Blend mode."))
            .input(
                FuncInput::required("Alpha", DataType::Float)
                    .description("Blend strength in [0, 1]. 1 is full source.")
                    .default(1.0),
            )
            .output(FuncOutput::new("Image", IMAGE_DATA_TYPE.clone()).description("Blended image."))
            .lambda(FuncLambda::new(
                move |ctx_manager, _, _, inputs, _, outputs| {
                    Box::pin(async move {
                        assert_eq!(inputs.len(), 4);
                        assert_eq!(outputs.len(), 1);

                        let src_image = inputs[0].value.as_custom::<Image>().unwrap();
                        let dst_image = inputs[1].value.as_custom::<Image>().unwrap();
                        let mode_name = inputs[2].value.as_enum().unwrap();
                        let alpha = inputs[3].value.as_f64().unwrap() as f32;

                        let blend_mode: BlendMode = mode_name.parse().expect("Invalid blend mode");

                        let vision_ctx = ctx_manager.get::<VisionCtx>(&VISION_CTX_TYPE);

                        let mut output_buffer = imaginarium::ImageBuffer::new_empty(src_image.desc);

                        Blend::new(blend_mode, alpha)
                            .execute(
                                &mut vision_ctx.processing_ctx,
                                src_image,
                                dst_image,
                                &mut output_buffer,
                            )
                            .map_err(anyhow::Error::from)?;

                        outputs[0] = DynamicValue::from_custom(Image::from(output_buffer));

                        Ok(())
                    })
                },
            )),
    );

    // transform
    library.add(
        Func::new("d3e4f5a6-b7c8-4d9e-0f1a-2b3c4d5e6f7a", "Transform")
            .description("Applies scale, rotation, and translation to an image.")
            .category("Image")
            .pure()
            .context(VISION_CTX_TYPE.clone())
            .input(
                FuncInput::required("Image", IMAGE_DATA_TYPE.clone())
                    .description("Image to transform."),
            )
            .input(
                FuncInput::required("Scale X", DataType::Float)
                    .description("Horizontal scale factor. 1 leaves width unchanged.")
                    .default(1.0),
            )
            .input(
                FuncInput::required("Scale Y", DataType::Float)
                    .description("Vertical scale factor. 1 leaves height unchanged.")
                    .default(1.0),
            )
            .input(
                FuncInput::required("Rotation", DataType::Float)
                    .description("Rotation in radians, about the image center.")
                    .default(0.0),
            )
            .input(
                FuncInput::required("Translate X", DataType::Float)
                    .description("Horizontal shift in pixels.")
                    .default(0.0),
            )
            .input(
                FuncInput::required("Translate Y", DataType::Float)
                    .description("Vertical shift in pixels.")
                    .default(0.0),
            )
            .output(
                FuncOutput::new("Image", IMAGE_DATA_TYPE.clone()).description("Transformed image."),
            )
            .lambda(FuncLambda::new(
                move |ctx_manager, _, _, inputs, _, outputs| {
                    Box::pin(async move {
                        assert_eq!(inputs.len(), 6);
                        assert_eq!(outputs.len(), 1);

                        let input_image = inputs[0].value.as_custom::<Image>().unwrap();
                        let scale_x = inputs[1].value.as_f64().unwrap() as f32;
                        let scale_y = inputs[2].value.as_f64().unwrap() as f32;
                        let rotation = inputs[3].value.as_f64().unwrap() as f32;
                        let translate_x = inputs[4].value.as_f64().unwrap() as f32;
                        let translate_y = inputs[5].value.as_f64().unwrap() as f32;

                        let vision_ctx = ctx_manager.get::<VisionCtx>(&VISION_CTX_TYPE);

                        let mut output_buffer =
                            imaginarium::ImageBuffer::new_empty(input_image.desc);

                        let center = Vec2::new(
                            input_image.desc.width as f32 / 2.0,
                            input_image.desc.height as f32 / 2.0,
                        );

                        Transform::new()
                            .scale(Vec2::new(scale_x, scale_y))
                            .rotate_around(rotation, center)
                            .translate(Vec2::new(translate_x, translate_y))
                            .execute(
                                &mut vision_ctx.processing_ctx,
                                input_image,
                                &mut output_buffer,
                            )
                            .map_err(anyhow::Error::from)?;

                        outputs[0] = DynamicValue::from_custom(Image::from(output_buffer));

                        Ok(())
                    })
                },
            )),
    );

    library.register_type(*IMAGE_TYPE_ID, image_type_entry());
    library.register_type(
        *BLENDMODE_TYPE_ID,
        TypeEntry::enum_of::<BlendMode>("BlendMode"),
    );
    library.register_type(
        *CONVERSION_FORMAT_TYPE_ID,
        TypeEntry::enum_of::<ConversionFormat>("ConversionFormat"),
    );

    library
}

/// The FsPath input type for image files — the extensions `read_file`/`save_file`
/// accept.
fn image_fs_path(mode: FsPathMode) -> DataType {
    DataType::FsPath(Arc::new(FsPathConfig::with_extensions(
        mode,
        SUPPORTED_EXTENSIONS.iter().map(|s| s.to_string()).collect(),
    )))
}

/// Resolve a `Format` enum input against the image's current format: `None` when the
/// pick is "As Is" or the image is already in that format — no conversion needed.
fn conversion_target(format_str: &str, current: ColorFormat) -> Option<ColorFormat> {
    let format = ConversionFormat::from_str(format_str)
        .expect("enum input is validated at the compile boundary")
        .to_color_format()?;
    (format != current).then_some(format)
}

/// Apply `op` to an image value: a uniquely-owned, CPU-resident input is adjusted
/// **in place** (the executor's move-on-last-use hands the node the sole holder, so
/// the allocation is reused and no output image is allocated); a shared or
/// GPU-resident one goes through the allocate-fresh-output path as before.
fn adjust_image(
    op: ContrastBrightness,
    ctx: &mut imaginarium::ProcessingContext,
    value: DynamicValue,
) -> anyhow::Result<Image> {
    match value.into_custom::<Image>() {
        Ok(mut image) if image.is_cpu() => {
            {
                // `make_cpu_mut` through `DerefMut` also invalidates the preview.
                let mut cpu = image.make_cpu_mut(ctx).map_err(anyhow::Error::from)?;
                op.apply_cpu(&mut cpu);
            }
            Ok(image)
        }
        // A GPU-resident unique input keeps the GPU pipeline: fresh output, GPU op.
        Ok(image) => adjust_into_fresh(op, ctx, &image),
        Err(value) => {
            let input = value
                .as_custom::<Image>()
                .expect("image input type is validated at the compile boundary");
            adjust_into_fresh(op, ctx, input)
        }
    }
}

fn adjust_into_fresh(
    op: ContrastBrightness,
    ctx: &mut imaginarium::ProcessingContext,
    input: &Image,
) -> anyhow::Result<Image> {
    let mut output = imaginarium::ImageBuffer::new_empty(input.desc);
    op.execute(ctx, input, &mut output)
        .map_err(anyhow::Error::from)?;
    Ok(Image::from(output))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn func<'a>(lib: &'a Library, name: &str) -> &'a Func {
        lib.funcs
            .iter()
            .find(|f| f.name == name)
            .unwrap_or_else(|| panic!("{name} registered"))
    }

    /// `adjust_image` reuses a uniquely-held CPU input's allocation and leaves a
    /// shared input untouched behind a fresh output — with identical pixels either
    /// way. Pointer identity of the pixel bytes tells the two paths apart.
    #[test]
    fn adjust_image_runs_in_place_only_for_unique_cpu_inputs() {
        let desc = imaginarium::ImageDesc::new(9, 4, imaginarium::ColorFormat::RGBA_U8);
        let op = ContrastBrightness::new(1.5, 0.1);
        let mut ctx = imaginarium::ProcessingContext::cpu_only();

        // A non-uniform pattern so the adjustment visibly changes bytes.
        let pattern: Vec<u8> = (0..desc.size_in_bytes()).map(|i| (i % 251) as u8).collect();
        let patterned_image = || {
            let mut img = imaginarium::Image::new_black(desc).unwrap();
            img.bytes_mut().copy_from_slice(&pattern);
            img
        };

        // Unique CPU input: adjusted in place — the pixel allocation is reused.
        let img = patterned_image();
        let unique_ptr = img.bytes().as_ptr();
        let unique = DynamicValue::from_custom(Image::from(img));
        let adjusted = adjust_image(op, &mut ctx, unique).unwrap();
        let adjusted_cpu = adjusted.buffer.make_cpu(&ctx).unwrap();
        assert_eq!(
            adjusted_cpu.bytes().as_ptr(),
            unique_ptr,
            "unique CPU input is adjusted in place"
        );
        assert_ne!(
            adjusted_cpu.bytes(),
            pattern.as_slice(),
            "adjustment changed the pixels"
        );

        // Shared input: a second holder blocks the move → fresh output, original intact.
        let img = patterned_image();
        let shared_ptr = img.bytes().as_ptr();
        let shared = DynamicValue::from_custom(Image::from(img));
        let holder = shared.clone();
        let adjusted_shared = adjust_image(op, &mut ctx, shared).unwrap();
        let shared_cpu = adjusted_shared.buffer.make_cpu(&ctx).unwrap();
        assert_ne!(
            shared_cpu.bytes().as_ptr(),
            shared_ptr,
            "shared input gets a fresh output allocation"
        );
        let original = holder.as_custom::<Image>().unwrap();
        let original_cpu = original.buffer.make_cpu(&ctx).unwrap();
        assert_eq!(original_cpu.bytes().as_ptr(), shared_ptr);
        assert_eq!(
            original_cpu.bytes(),
            pattern.as_slice(),
            "the shared original is untouched"
        );

        // Both paths compute identical pixels.
        assert_eq!(adjusted_cpu.bytes(), shared_cpu.bytes());
    }

    #[test]
    fn conversion_target_collapses_as_is_and_matching_format() {
        // "As Is" never converts, whatever the source format.
        assert_eq!(conversion_target("As Is", ColorFormat::RGB_U8), None);
        // A pick matching the source format is a no-op too — the node passes the
        // input value through instead of copying it.
        assert_eq!(conversion_target("RGB u8", ColorFormat::RGB_U8), None);
        // A genuinely different format converts.
        assert_eq!(
            conversion_target("RGB u8", ColorFormat::RGBA_F32),
            Some(ColorFormat::RGB_U8)
        );
    }

    #[test]
    fn convert_node_defaults_to_rgb_u8() {
        let lib = image_library();
        let f = func(&lib, "Convert");
        assert_eq!(f.category, "Image");

        // Image in (required), Format enum (required, seeded), Image out.
        let names: Vec<&str> = f.inputs.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["Image", "Format"]);
        assert_eq!(f.inputs[0].data_type, *IMAGE_DATA_TYPE);
        assert!(f.inputs[0].required);
        assert_eq!(f.inputs[1].data_type, *CONVERSION_FORMAT_DATATYPE);
        assert!(f.inputs[1].required);

        // The Format input is seeded to RGB_U8 — `ColorFormat`'s Display is
        // "{count} {type}{size}", so RGB_U8 → "RGB u8".
        assert_eq!(
            f.inputs[1].default_value,
            Some(StaticValue::Enum("RGB u8".to_string())),
        );
        // Cross-check the literal against the enum's own formatting so a Display
        // change can't let the two silently drift.
        assert_eq!(
            f.inputs[1].default_value,
            Some(StaticValue::Enum(ConversionFormat::RgbU8.label())),
        );
        // It's a genuine override: `enum_input` would otherwise seed the first
        // variant (now "As Is"), so a fresh node emits 8-bit RGB, not a pass-through.
        assert_ne!(
            f.inputs[1].default_value,
            Some(StaticValue::Enum("As Is".to_string())),
        );
    }

    #[test]
    fn save_image_format_defaults_to_as_is() {
        let lib = image_library();
        let f = func(&lib, "Save Image");

        // Image in, Path in, and the new Format enum — all required.
        let names: Vec<&str> = f.inputs.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["Image", "Path", "Format"]);
        assert_eq!(f.inputs[2].data_type, *CONVERSION_FORMAT_DATATYPE);
        assert!(f.inputs[2].required);

        // Save Image explicitly defaults to "As Is", so a fresh node saves the
        // source format untouched.
        assert_eq!(
            f.inputs[2].default_value,
            Some(StaticValue::Enum("As Is".to_string())),
        );
        assert_eq!(
            f.inputs[2].default_value,
            Some(StaticValue::Enum(ConversionFormat::AsIs.label())),
        );
        assert_eq!(
            ConversionFormat::from_str("As Is")
                .unwrap()
                .to_color_format(),
            None,
        );
    }
}
