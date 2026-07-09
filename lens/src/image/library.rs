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
use imaginarium::{Blend, BlendMode, ContrastBrightness, SUPPORTED_EXTENSIONS, Transform, Vec2};
use scenarium::data::{DataType, DynamicValue, FsPathConfig, FsPathMode, StaticValue};
use scenarium::library::{Library, TypeEntry};
use scenarium::node::func_lambda::FuncLambda;
use scenarium::node::function::{Func, FuncInput, FuncOutput};

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

                    let input_image = inputs[0].value.as_custom::<Image>().unwrap();

                    let brightness = inputs[1].value.as_f64().unwrap() as f32;
                    let contrast = inputs[2].value.as_f64().unwrap() as f32;

                    let vision_ctx = ctx_manager.get::<VisionCtx>(&VISION_CTX_TYPE);

                    let mut output_buffer = imaginarium::ImageBuffer::new_empty(input_image.desc);

                    ContrastBrightness::new(contrast, brightness)
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

    // load_image
    library.add(
        Func::new("a4d9bf87-9d98-44f1-a162-7483c298be3d", "Load Image")
            .description("Loads an image from a file on disk.")
            .category("Image")
            .pure()
            .input(
                FuncInput::required(
                    "Path",
                    DataType::FsPath(Arc::new(FsPathConfig::with_extensions(
                        FsPathMode::ExistingFile,
                        SUPPORTED_EXTENSIONS.iter().map(|s| s.to_string()).collect(),
                    ))),
                )
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
            .terminal()
            .input(
                FuncInput::required("Image", IMAGE_DATA_TYPE.clone()).description("Image to save."),
            )
            .input(
                FuncInput::required(
                    "Path",
                    DataType::FsPath(Arc::new(FsPathConfig::with_extensions(
                        FsPathMode::NewFile,
                        SUPPORTED_EXTENSIONS.iter().map(|s| s.to_string()).collect(),
                    ))),
                )
                .description("Destination file; the extension picks the format."),
            )
            .context(VISION_CTX_TYPE.clone())
            .lambda(FuncLambda::new(move |ctx_manager, _, _, inputs, _, _| {
                Box::pin(async move {
                    assert_eq!(inputs.len(), 2);

                    let input_image = inputs[0].value.as_custom::<Image>().unwrap();
                    let path = inputs[1].value.as_fs_path().unwrap();

                    let vision_ctx = ctx_manager.get::<VisionCtx>(&VISION_CTX_TYPE);
                    let cpu_image = input_image
                        .make_cpu(&vision_ctx.processing_ctx)
                        .map_err(anyhow::Error::from)?;
                    cpu_image.save_file(path).map_err(anyhow::Error::from)?;

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
                    .default(StaticValue::Enum(
                        ConversionFormat::RgbU8.to_color_format().to_string(),
                    ))
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

                        let input_image = inputs[0].value.as_custom::<Image>().unwrap();
                        let format_str = inputs[1].value.as_enum().unwrap();
                        let vision_ctx = ctx_manager.get::<VisionCtx>(&VISION_CTX_TYPE);

                        let cpu_image = input_image
                            .make_cpu(&vision_ctx.processing_ctx)
                            .map_err(anyhow::Error::from)?;

                        let target_format = ConversionFormat::from_str(format_str)
                            .expect("Invalid conversion format")
                            .to_color_format();

                        let converted = cpu_image
                            .clone()
                            .convert(target_format)
                            .map_err(anyhow::Error::from)?;

                        outputs[0] = DynamicValue::from_custom(Image::from(converted));

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

#[cfg(test)]
mod tests {
    use super::*;

    fn func<'a>(lib: &'a Library, name: &str) -> &'a Func {
        lib.funcs
            .iter()
            .find(|f| f.name == name)
            .unwrap_or_else(|| panic!("{name} registered"))
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
            Some(StaticValue::Enum(
                ConversionFormat::RgbU8.to_color_format().to_string()
            )),
        );
        // It's a genuine override: `enum_input` would otherwise seed the first
        // variant (L_U8 → "L u8"), so a fresh node emits 8-bit RGB, not grayscale.
        assert_ne!(
            f.inputs[1].default_value,
            Some(StaticValue::Enum("L u8".to_string())),
        );
    }
}
