use std::str::FromStr;
use std::sync::Arc;

use crate::blend_mode::BLENDMODE_DATATYPE;
use crate::conversion_format::{CONVERSION_FORMAT_DATATYPE, ConversionFormat};
use crate::image::{IMAGE_DATA_TYPE, Image};
use crate::vision_ctx::{VISION_CTX_TYPE, VisionCtx};
use imaginarium::{Blend, BlendMode, ContrastBrightness, SUPPORTED_EXTENSIONS, Transform, Vec2};
use scenarium::data::{DataType, DynamicValue, FsPathConfig, FsPathMode};
use scenarium::func_lambda::FuncLambda;
use scenarium::function::{Func, FuncInput, FuncLib};

/// The imaginarium image-processing nodes (category `image`).
pub fn image_funclib() -> FuncLib {
    let mut func_lib = FuncLib::default();

    // brightness_contrast
    func_lib.add(
        Func::new(
            "b8c3d4e5-f6a7-4b8c-9d0e-1f2a3b4c5d6e",
            "brightness_contrast",
        )
        .description(
            "Adjusts brightness and contrast of an image. Contrast multiplier (1.0 = no change), \
             brightness offset [-1.0, 1.0]",
        )
        .category("image")
        .pure()
        .context(VISION_CTX_TYPE.clone())
        .input(FuncInput::required("image", IMAGE_DATA_TYPE.clone()))
        .input(FuncInput::required("brightness", DataType::Float).default(0.0))
        .input(FuncInput::required("contrast", DataType::Float).default(1.0))
        .output("image", IMAGE_DATA_TYPE.clone())
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
    func_lib.add(
        Func::new("a4d9bf87-9d98-44f1-a162-7483c298be3d", "load_image")
            .description("Loads an image from file")
            .category("image")
            .run_once()
            .input(FuncInput::required(
                "path",
                DataType::FsPath(Arc::new(FsPathConfig::with_extensions(
                    FsPathMode::ExistingFile,
                    SUPPORTED_EXTENSIONS.iter().map(|s| s.to_string()).collect(),
                ))),
            ))
            .output("image", IMAGE_DATA_TYPE.clone())
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
    func_lib.add(
        Func::new("0c17bcbe-d757-43be-b184-27b429e8b434", "save_image")
            .description("Saves an image to file")
            .category("image")
            .terminal()
            .input(FuncInput::required("image", IMAGE_DATA_TYPE.clone()))
            .input(FuncInput::required(
                "path",
                DataType::FsPath(Arc::new(FsPathConfig::with_extensions(
                    FsPathMode::NewFile,
                    SUPPORTED_EXTENSIONS.iter().map(|s| s.to_string()).collect(),
                ))),
            ))
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
    func_lib.add(
        Func::new("80aa1ee7-3b75-4200-b480-b9db913bd6eb", "convert")
            .description("Converts image to a different color format")
            .category("image")
            .pure()
            .context(VISION_CTX_TYPE.clone())
            .input(FuncInput::required("image", IMAGE_DATA_TYPE.clone()))
            .input(enum_input("format", &CONVERSION_FORMAT_DATATYPE))
            .output("image", IMAGE_DATA_TYPE.clone())
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
    func_lib.add(
        Func::new("975cc74b-8412-4293-b2cb-ef8d41fdd9b3", "blend")
            .description("Blends two images")
            .category("image")
            .pure()
            .context(VISION_CTX_TYPE.clone())
            .input(FuncInput::required("source", IMAGE_DATA_TYPE.clone()))
            .input(FuncInput::required("destination", IMAGE_DATA_TYPE.clone()))
            .input(enum_input("mode", &BLENDMODE_DATATYPE))
            .input(FuncInput::required("alpha", DataType::Float).default(1.0))
            .output("image", IMAGE_DATA_TYPE.clone())
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
    func_lib.add(
        Func::new("d3e4f5a6-b7c8-4d9e-0f1a-2b3c4d5e6f7a", "transform")
            .description("Applies scale, rotation, and translation to an image")
            .category("image")
            .pure()
            .context(VISION_CTX_TYPE.clone())
            .input(FuncInput::required("image", IMAGE_DATA_TYPE.clone()))
            .input(FuncInput::required("scale_x", DataType::Float).default(1.0))
            .input(FuncInput::required("scale_y", DataType::Float).default(1.0))
            .input(FuncInput::required("rotation", DataType::Float).default(0.0))
            .input(FuncInput::required("translate_x", DataType::Float).default(0.0))
            .input(FuncInput::required("translate_y", DataType::Float).default(0.0))
            .output("image", IMAGE_DATA_TYPE.clone())
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

    func_lib
}

/// A required enum-dropdown input seeded to the datatype's first variant.
fn enum_input(name: &str, datatype: &DataType) -> FuncInput {
    let mut input = FuncInput::required(name, datatype.clone());
    input.default_value = datatype.default_value();
    input
}
