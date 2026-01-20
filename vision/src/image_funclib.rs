use std::ops::{Deref, DerefMut};
use std::sync::LazyLock;

use graph::data::{CustomValue, DataType, DynamicValue, EnumDef, StaticValue, TypeDef};
use graph::func_lambda::FuncLambda;
use graph::function::{Func, FuncBehavior, FuncInput, FuncLib, FuncOutput};
use imaginarium::{
    Blend, BlendMode, ChannelCount, ColorFormat, ContrastBrightness, Transform, Vec2,
};
use std::sync::Arc;

use crate::vision_ctx::{VISION_CTX_TYPE, VisionCtx};

pub static IMAGE_DATA_TYPE: LazyLock<DataType> = LazyLock::new(|| {
    DataType::Custom(Arc::new(TypeDef {
        type_id: "a69f9a9c-3be7-4d8b-abb1-dbd5c9ee4da2".into(),
        display_name: "Image".to_string(),
    }))
});

pub static BLENDMODE_ENUM: LazyLock<Arc<EnumDef>> = LazyLock::new(|| {
    Arc::new(EnumDef {
        type_id: "54d531cf-d353-4e30-8ea7-8823a9b5305f".into(),
        display_name: "Blendmode".to_string(),
        variants: vec![
            "Normal".to_string(),
            "Add".to_string(),
            "Subtract".to_string(),
            "Multiply".to_string(),
            "Screen".to_string(),
            "Overlay".to_string(),
        ],
    })
});

/// Wrapper around `imaginarium::ImageBuffer` that implements `CustomValue`.
#[derive(Debug)]
pub struct Image(pub imaginarium::ImageBuffer);

impl CustomValue for Image {
    fn data_type(&self) -> DataType {
        IMAGE_DATA_TYPE.clone()
    }
}

impl From<imaginarium::ImageBuffer> for Image {
    fn from(buffer: imaginarium::ImageBuffer) -> Self {
        Image(buffer)
    }
}

impl From<imaginarium::Image> for Image {
    fn from(image: imaginarium::Image) -> Self {
        Image(imaginarium::ImageBuffer::from(image))
    }
}

impl Deref for Image {
    type Target = imaginarium::ImageBuffer;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Image {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

#[derive(Debug)]
pub struct ImageFuncLib {
    func_lib: FuncLib,
}

impl ImageFuncLib {
    pub fn func_lib(&self) -> &FuncLib {
        &self.func_lib
    }

    pub fn into_func_lib(self) -> FuncLib {
        self.func_lib
    }
}

impl From<ImageFuncLib> for FuncLib {
    fn from(image: ImageFuncLib) -> Self {
        image.func_lib
    }
}

impl Default for ImageFuncLib {
    fn default() -> Self {
        let mut func_lib = FuncLib::default();

        // brightness_contrast
        func_lib.add(Func {
            id: "b8c3d4e5-f6a7-4b8c-9d0e-1f2a3b4c5d6e".into(),
            name: "brightness_contrast".to_string(),
            description: Some(
                "Adjusts brightness and contrast of an image. Contrast multiplier (1.0 = no change), brightness offset [-1.0, 1.0]".to_string(),
            ),
            behavior: FuncBehavior::Pure,
            terminal: false,
            category: "image".to_string(),
            inputs: vec![
                FuncInput {
                    name: "image".to_string(),
                    required: true,
                    data_type: IMAGE_DATA_TYPE.clone(),
                    default_value: None,
                    value_options: vec![],
                },
                FuncInput {
                    name: "brightness".to_string(),
                    required: true,
                    data_type: DataType::Float,
                    default_value: Some((0.0).into()),
                    value_options: vec![],
                },
                FuncInput {
                    name: "contrast".to_string(),
                    required: true,
                    data_type: DataType::Float,
                    default_value: Some((1.0).into()),
                    value_options: vec![],
                },
            ],
            outputs: vec![FuncOutput {
                name: "image".to_string(),
                data_type: IMAGE_DATA_TYPE.clone(),
            }],
            events: vec![],
            required_contexts: vec![VISION_CTX_TYPE.clone()],
            lambda: FuncLambda::new(move |ctx_manager, _, _, inputs, _, outputs| {
                Box::pin(async move {
                    assert_eq!(inputs.len(), 3);
                    assert_eq!(outputs.len(), 1);

                    let input_image = inputs[0].value.as_custom::<Image>().unwrap();

                    let brightness = inputs[1].value.as_f64().unwrap() as f32;
                    let contrast = inputs[2].value.as_f64().unwrap() as f32;

                    let vision_ctx = ctx_manager.get::<VisionCtx>(&VISION_CTX_TYPE);

                    let mut output_buffer =
                        imaginarium::ImageBuffer::new_empty(*input_image.desc());

                    ContrastBrightness::new(contrast, brightness)
                        .execute(&mut vision_ctx.processing_ctx, input_image, &mut output_buffer)
                        .expect("Failed to apply brightness/contrast");

                    outputs[0] = DynamicValue::from_custom(Image::from(output_buffer));

                    Ok(())
                })
            }),
        });

        // load_image
        func_lib.add(Func {
            id: "a4d9bf87-9d98-44f1-a162-7483c298be3d".into(),
            name: "load_image".to_string(),
            description: Some("Loads an image from file".to_string()),
            behavior: FuncBehavior::Pure,
            terminal: false,
            category: "image".to_string(),
            inputs: vec![],
            outputs: vec![FuncOutput {
                name: "image".to_string(),
                data_type: IMAGE_DATA_TYPE.clone(),
            }],
            events: vec![],
            required_contexts: vec![],
            lambda: FuncLambda::new(move |_, _, _, _, _, outputs| {
                Box::pin(async move {
                    assert_eq!(outputs.len(), 1);

                    // For now, always load lena.tiff from test_resources
                    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../test_resources/lena.tiff");
                    let image = imaginarium::Image::read_file(path).expect("Failed to load image");

                    outputs[0] = DynamicValue::from_custom(Image::from(image));

                    Ok(())
                })
            }),
        });

        // save_image
        func_lib.add(Func {
            id: "0c17bcbe-d757-43be-b184-27b429e8b434".into(),
            name: "save_image".to_string(),
            description: Some("Saves an image to file".to_string()),
            behavior: FuncBehavior::Impure,
            terminal: true,
            category: "image".to_string(),
            inputs: vec![FuncInput {
                name: "image".to_string(),
                required: true,
                data_type: IMAGE_DATA_TYPE.clone(),
                default_value: None,
                value_options: vec![],
            }],
            outputs: vec![],
            events: vec![],
            required_contexts: vec![VISION_CTX_TYPE.clone()],
            lambda: FuncLambda::new(move |ctx_manager, _, _, inputs, _, _| {
                Box::pin(async move {
                    assert_eq!(inputs.len(), 1);

                    let input_image = inputs[0].value.as_custom::<Image>().unwrap();

                    // For now, save to test_output directory

                    let path = "vision_output.tiff";
                    let vision_ctx = ctx_manager.get::<VisionCtx>(&VISION_CTX_TYPE);
                    let cpu_image = input_image
                        .make_cpu(&vision_ctx.processing_ctx)
                        .expect("Failed to get CPU image");
                    cpu_image.save_file(path).expect("Failed to save image");

                    Ok(())
                })
            }),
        });

        // convert_to_f32
        func_lib.add(Func {
            id: "80aa1ee7-3b75-4200-b480-b9db913bd6eb".into(),
            name: "convert_to_f32".to_string(),
            description: Some("Converts image color format to f32".to_string()),
            behavior: FuncBehavior::Pure,
            terminal: false,
            category: "image".to_string(),
            inputs: vec![FuncInput {
                name: "image".to_string(),
                required: true,
                data_type: IMAGE_DATA_TYPE.clone(),
                default_value: None,
                value_options: vec![],
            }],
            outputs: vec![FuncOutput {
                name: "image".to_string(),
                data_type: IMAGE_DATA_TYPE.clone(),
            }],
            events: vec![],
            required_contexts: vec![VISION_CTX_TYPE.clone()],
            lambda: FuncLambda::new(move |ctx_manager, _, _, inputs, _, outputs| {
                Box::pin(async move {
                    assert_eq!(inputs.len(), 1);
                    assert_eq!(outputs.len(), 1);

                    let input_image = inputs[0].value.as_custom::<Image>().unwrap();
                    let vision_ctx = ctx_manager.get::<VisionCtx>(&VISION_CTX_TYPE);

                    let cpu_image = input_image
                        .make_cpu(&vision_ctx.processing_ctx)
                        .expect("Failed to get CPU image");

                    let target_format = match cpu_image.desc().color_format.channel_count {
                        ChannelCount::Gray => ColorFormat::GRAY_F32,
                        ChannelCount::GrayAlpha => ColorFormat::GRAY_ALPHA_F32,
                        ChannelCount::Rgb => ColorFormat::RGB_F32,
                        ChannelCount::Rgba => ColorFormat::RGBA_F32,
                    };

                    let converted = cpu_image
                        .clone()
                        .convert(target_format)
                        .expect("Failed to convert image");

                    outputs[0] = DynamicValue::from_custom(Image::from(converted));

                    Ok(())
                })
            }),
        });

        // blend
        func_lib.add(Func {
            id: "975cc74b-8412-4293-b2cb-ef8d41fdd9b3".into(),
            name: "blend".to_string(),
            description: Some("Blends two images".to_string()),
            behavior: FuncBehavior::Pure,
            terminal: false,
            category: "image".to_string(),
            inputs: vec![
                FuncInput {
                    name: "source".to_string(),
                    required: true,
                    data_type: IMAGE_DATA_TYPE.clone(),
                    default_value: None,
                    value_options: vec![],
                },
                FuncInput {
                    name: "destination".to_string(),
                    required: true,
                    data_type: IMAGE_DATA_TYPE.clone(),
                    default_value: None,
                    value_options: vec![],
                },
                FuncInput {
                    name: "mode".to_string(),
                    required: true,
                    data_type: DataType::Enum(Arc::clone(&BLENDMODE_ENUM)),
                    default_value: Some(StaticValue::Enum {
                        type_id: BLENDMODE_ENUM.type_id,
                        variant_name: "Normal".to_string(),
                    }),
                    value_options: vec![],
                },
                FuncInput {
                    name: "alpha".to_string(),
                    required: true,
                    data_type: DataType::Float,
                    default_value: Some((1.0).into()),
                    value_options: vec![],
                },
            ],
            outputs: vec![FuncOutput {
                name: "image".to_string(),
                data_type: IMAGE_DATA_TYPE.clone(),
            }],
            events: vec![],
            required_contexts: vec![VISION_CTX_TYPE.clone()],
            lambda: FuncLambda::new(move |ctx_manager, _, _, inputs, _, outputs| {
                Box::pin(async move {
                    assert_eq!(inputs.len(), 4);
                    assert_eq!(outputs.len(), 1);

                    let src_image = inputs[0].value.as_custom::<Image>().unwrap();
                    let dst_image = inputs[1].value.as_custom::<Image>().unwrap();
                    let mode_name = inputs[2].value.as_enum().unwrap();
                    let alpha = inputs[3].value.as_f64().unwrap() as f32;

                    let blend_mode: BlendMode = mode_name.parse().expect("Invalid blend mode");

                    let vision_ctx = ctx_manager.get::<VisionCtx>(&VISION_CTX_TYPE);

                    let mut output_buffer = imaginarium::ImageBuffer::new_empty(*src_image.desc());

                    Blend::new(blend_mode, alpha)
                        .execute(
                            &mut vision_ctx.processing_ctx,
                            src_image,
                            dst_image,
                            &mut output_buffer,
                        )
                        .expect("Failed to blend images");

                    outputs[0] = DynamicValue::from_custom(Image::from(output_buffer));

                    Ok(())
                })
            }),
        });

        // transform
        func_lib.add(Func {
            id: "d3e4f5a6-b7c8-4d9e-0f1a-2b3c4d5e6f7a".into(),
            name: "transform".to_string(),
            description: Some("Applies scale, rotation, and translation to an image".to_string()),
            behavior: FuncBehavior::Pure,
            terminal: false,
            category: "image".to_string(),
            inputs: vec![
                FuncInput {
                    name: "image".to_string(),
                    required: true,
                    data_type: IMAGE_DATA_TYPE.clone(),
                    default_value: None,
                    value_options: vec![],
                },
                FuncInput {
                    name: "scale_x".to_string(),
                    required: true,
                    data_type: DataType::Float,
                    default_value: Some((1.0).into()),
                    value_options: vec![],
                },
                FuncInput {
                    name: "scale_y".to_string(),
                    required: true,
                    data_type: DataType::Float,
                    default_value: Some((1.0).into()),
                    value_options: vec![],
                },
                FuncInput {
                    name: "rotation".to_string(),
                    required: true,
                    data_type: DataType::Float,
                    default_value: Some((0.0).into()),
                    value_options: vec![],
                },
                FuncInput {
                    name: "translate_x".to_string(),
                    required: true,
                    data_type: DataType::Float,
                    default_value: Some((0.0).into()),
                    value_options: vec![],
                },
                FuncInput {
                    name: "translate_y".to_string(),
                    required: true,
                    data_type: DataType::Float,
                    default_value: Some((0.0).into()),
                    value_options: vec![],
                },
            ],
            outputs: vec![FuncOutput {
                name: "image".to_string(),
                data_type: IMAGE_DATA_TYPE.clone(),
            }],
            events: vec![],
            required_contexts: vec![VISION_CTX_TYPE.clone()],
            lambda: FuncLambda::new(move |ctx_manager, _, _, inputs, _, outputs| {
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
                        imaginarium::ImageBuffer::new_empty(*input_image.desc());

                    let center = Vec2::new(
                        input_image.desc().width as f32 / 2.0,
                        input_image.desc().height as f32 / 2.0,
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
                        .expect("Failed to transform image");

                    outputs[0] = DynamicValue::from_custom(Image::from(output_buffer));

                    Ok(())
                })
            }),
        });

        Self { func_lib }
    }
}
