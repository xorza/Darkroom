//! In-memory image adjustment, conversion, blending, and transform nodes.

use imaginarium::{Blend, BlendMode, ContrastBrightness, Transform, Vec2};
use scenarium::{DataType, DynamicValue, StaticValue};
use scenarium::{Func, FuncInput, FuncLambda, FuncOutput, InvokeError, Library};

use crate::config_node::enum_input;
use crate::image::context::{VISION_CTX_TYPE, VisionCtx};
use crate::image::format::{CONVERSION_FORMAT_DATATYPE, ConversionFormat, conversion_target};
use crate::image::nodes::BLENDMODE_DATATYPE;
use crate::image::{IMAGE_DATA_TYPE, Image};

pub(crate) fn register(library: &mut Library) {
    register_brightness(library);
    register_convert(library);
    register_blend(library);
    register_transform(library);
}

fn register_brightness(library: &mut Library) {
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
            move |contexts, _, _, inputs, _, outputs| {
                Box::pin(async move {
                    debug_assert_eq!(inputs.len(), 3);
                    debug_assert_eq!(outputs.len(), 1);
                    let value = std::mem::take(&mut inputs[0].value);
                    let brightness = inputs[1]
                        .value
                        .as_f64()
                        .expect("brightness input type is validated at the compile boundary")
                        as f32;
                    let contrast = inputs[2]
                        .value
                        .as_f64()
                        .expect("contrast input type is validated at the compile boundary")
                        as f32;
                    let vision = contexts.get::<VisionCtx>(&VISION_CTX_TYPE);
                    let image = adjust_image(
                        ContrastBrightness::new(contrast, brightness),
                        &mut vision.processing_ctx,
                        value,
                    )
                    .map_err(InvokeError::external)?;
                    outputs[0] = DynamicValue::from_custom(image);
                    Ok(())
                })
            },
        )),
    );
}

fn register_convert(library: &mut Library) {
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
                move |contexts, _, _, inputs, _, outputs| {
                    Box::pin(async move {
                        debug_assert_eq!(inputs.len(), 2);
                        debug_assert_eq!(outputs.len(), 1);
                        let value = std::mem::take(&mut inputs[0].value);
                        let format = inputs[1]
                            .value
                            .as_enum()
                            .expect("format input type is validated at the compile boundary");
                        let converted = {
                            let image = value
                                .as_custom::<Image>()
                                .expect("image input type is validated at the compile boundary");
                            match conversion_target(format, image.buffer.desc.color_format) {
                                Some(target) => {
                                    let vision = contexts.get::<VisionCtx>(&VISION_CTX_TYPE);
                                    let cpu_image = image
                                        .buffer
                                        .make_cpu(&vision.processing_ctx)
                                        .map_err(InvokeError::external)?;
                                    Some(
                                        cpu_image
                                            .convert_to(target)
                                            .map_err(InvokeError::external)?,
                                    )
                                }
                                None => None,
                            }
                        };
                        outputs[0] = match converted {
                            Some(image) => DynamicValue::from_custom(Image::from(image)),
                            None => value,
                        };
                        Ok(())
                    })
                },
            )),
    );
}

fn register_blend(library: &mut Library) {
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
                move |contexts, _, _, inputs, _, outputs| {
                    Box::pin(async move {
                        debug_assert_eq!(inputs.len(), 4);
                        debug_assert_eq!(outputs.len(), 1);
                        let source = inputs[0]
                            .value
                            .as_custom::<Image>()
                            .expect("source input type is validated at the compile boundary");
                        let destination = inputs[1]
                            .value
                            .as_custom::<Image>()
                            .expect("destination input type is validated at the compile boundary");
                        let mode = inputs[2]
                            .value
                            .as_enum()
                            .expect("mode input type is validated at the compile boundary")
                            .parse::<BlendMode>()
                            .expect("enum input is validated at the compile boundary");
                        let alpha = inputs[3]
                            .value
                            .as_f64()
                            .expect("alpha input type is validated at the compile boundary")
                            as f32;
                        let vision = contexts.get::<VisionCtx>(&VISION_CTX_TYPE);
                        let mut output = imaginarium::ImageBuffer::new_empty(source.buffer.desc);
                        Blend::new(mode, alpha)
                            .execute(
                                &mut vision.processing_ctx,
                                &source.buffer,
                                &destination.buffer,
                                &mut output,
                            )
                            .map_err(InvokeError::external)?;
                        outputs[0] = DynamicValue::from_custom(Image::from(output));
                        Ok(())
                    })
                },
            )),
    );
}

fn register_transform(library: &mut Library) {
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
                move |contexts, _, _, inputs, _, outputs| {
                    Box::pin(async move {
                        debug_assert_eq!(inputs.len(), 6);
                        debug_assert_eq!(outputs.len(), 1);
                        let image = inputs[0]
                            .value
                            .as_custom::<Image>()
                            .expect("image input type is validated at the compile boundary");
                        let scalar =
                            |index: usize| {
                                inputs[index].value.as_f64().expect(
                                    "transform input type is validated at the compile boundary",
                                ) as f32
                            };
                        let vision = contexts.get::<VisionCtx>(&VISION_CTX_TYPE);
                        let mut output = imaginarium::ImageBuffer::new_empty(image.buffer.desc);
                        let center = Vec2::new(
                            image.buffer.desc.width as f32 / 2.0,
                            image.buffer.desc.height as f32 / 2.0,
                        );
                        Transform::new()
                            .scale(Vec2::new(scalar(1), scalar(2)))
                            .rotate_around(scalar(3), center)
                            .translate(Vec2::new(scalar(4), scalar(5)))
                            .execute(&mut vision.processing_ctx, &image.buffer, &mut output)
                            .map_err(InvokeError::external)?;
                        outputs[0] = DynamicValue::from_custom(Image::from(output));
                        Ok(())
                    })
                },
            )),
    );
}

pub(crate) fn adjust_image(
    op: ContrastBrightness,
    context: &mut imaginarium::ProcessingContext,
    value: DynamicValue,
) -> imaginarium::Result<Image> {
    match value.into_custom::<Image>() {
        Ok(mut image) if image.buffer.is_cpu() => {
            {
                let cpu = image.buffer.make_cpu_mut(context)?;
                op.apply_cpu(cpu);
            }
            Ok(image)
        }
        Ok(image) => adjust_into_fresh(op, context, &image),
        Err(value) => {
            let input = value
                .as_custom::<Image>()
                .expect("image input type is validated at the compile boundary");
            adjust_into_fresh(op, context, input)
        }
    }
}

fn adjust_into_fresh(
    op: ContrastBrightness,
    context: &mut imaginarium::ProcessingContext,
    input: &Image,
) -> imaginarium::Result<Image> {
    let mut output = imaginarium::ImageBuffer::new_empty(input.buffer.desc);
    op.execute(context, &input.buffer, &mut output)?;
    Ok(Image::from(output))
}
