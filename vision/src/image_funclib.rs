use std::ops::{Deref, DerefMut};
use std::sync::LazyLock;

use crate::vision_ctx::{VISION_CTX_TYPE, VisionCtx};
use graph::data::{CustomValue, DataType, DynamicValue, FsPathConfig, FsPathMode};
use graph::func_lambda::FuncLambda;
use graph::function::{Func, FuncBehavior, FuncInput, FuncLib, FuncOutput};
use imaginarium::{
    Blend, BlendMode, ColorFormat, ContrastBrightness, SUPPORTED_EXTENSIONS, Transform, Vec2,
};
use std::str::FromStr;

pub static IMAGE_DATA_TYPE: LazyLock<DataType> =
    LazyLock::new(|| DataType::from_custom("a69f9a9c-3be7-4d8b-abb1-dbd5c9ee4da2", "Image"));

pub static BLENDMODE_DATATYPE: LazyLock<DataType> = LazyLock::new(|| {
    DataType::from_enum::<BlendMode>("54d531cf-d353-4e30-8ea7-8823a9b5305f", "BlendMode")
});

use graph::data::EnumVariants;
use strum::IntoEnumIterator;
use strum_macros::EnumIter;

#[derive(Debug, Clone, Copy, PartialEq, Eq, EnumIter)]
pub enum ConversionFormat {
    GrayU8,
    GrayU16,
    GrayF32,
    GrayAlphaU8,
    GrayAlphaU16,
    GrayAlphaF32,
    RgbU8,
    RgbU16,
    RgbF32,
    RgbaU8,
    RgbaU16,
    RgbaF32,
}

impl ConversionFormat {
    pub fn to_color_format(self) -> ColorFormat {
        match self {
            ConversionFormat::GrayU8 => ColorFormat::GRAY_U8,
            ConversionFormat::GrayU16 => ColorFormat::GRAY_U16,
            ConversionFormat::GrayF32 => ColorFormat::GRAY_F32,
            ConversionFormat::GrayAlphaU8 => ColorFormat::GRAY_ALPHA_U8,
            ConversionFormat::GrayAlphaU16 => ColorFormat::GRAY_ALPHA_U16,
            ConversionFormat::GrayAlphaF32 => ColorFormat::GRAY_ALPHA_F32,
            ConversionFormat::RgbU8 => ColorFormat::RGB_U8,
            ConversionFormat::RgbU16 => ColorFormat::RGB_U16,
            ConversionFormat::RgbF32 => ColorFormat::RGB_F32,
            ConversionFormat::RgbaU8 => ColorFormat::RGBA_U8,
            ConversionFormat::RgbaU16 => ColorFormat::RGBA_U16,
            ConversionFormat::RgbaF32 => ColorFormat::RGBA_F32,
        }
    }
}

static CONVERSION_FORMAT_VARIANTS: LazyLock<Vec<String>> = LazyLock::new(|| {
    ConversionFormat::iter()
        .map(|f| f.to_color_format().to_string())
        .collect()
});

impl EnumVariants for ConversionFormat {
    fn variant_names() -> Vec<String> {
        CONVERSION_FORMAT_VARIANTS.clone()
    }
}

impl FromStr for ConversionFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        ConversionFormat::iter()
            .find(|f| f.to_color_format().to_string() == s)
            .ok_or_else(|| format!("Unknown conversion format: {}", s))
    }
}

pub static CONVERSION_FORMAT_DATATYPE: LazyLock<DataType> = LazyLock::new(|| {
    DataType::from_enum::<ConversionFormat>(
        "6d9db73e-5c92-4332-af0d-b2eb7c95acd0",
        "ConversionFormat",
    )
});

/// Wrapper around `imaginarium::ImageBuffer` that implements `CustomValue`.
#[derive(Debug)]
pub struct Image(pub imaginarium::ImageBuffer);

impl CustomValue for Image {
    fn data_type(&self) -> DataType {
        IMAGE_DATA_TYPE.clone()
    }
}

impl std::fmt::Display for Image {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0.desc())
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
                        .map_err(anyhow::Error::from)?;

                    outputs[0] = DynamicValue::from_custom(Image::from(output_buffer));

                    Ok(())
                })
            }),
            ..Default::default()
        });

        // load_image
        func_lib.add(Func {
            id: "a4d9bf87-9d98-44f1-a162-7483c298be3d".into(),
            name: "load_image".to_string(),
            description: Some("Loads an image from file".to_string()),
            behavior: FuncBehavior::Impure,
            node_default_behavior: graph::graph::NodeBehavior::Once,
            terminal: false,
            category: "image".to_string(),
            inputs: vec![FuncInput {
                name: "path".to_string(),
                required: true,
                data_type: DataType::FsPath(FsPathConfig::with_extensions(
                    FsPathMode::ExistingFile,
                    SUPPORTED_EXTENSIONS.iter().map(|s| s.to_string()).collect(),
                )),
                default_value: None,
                value_options: vec![],
            }],
            outputs: vec![FuncOutput {
                name: "image".to_string(),
                data_type: IMAGE_DATA_TYPE.clone(),
            }],
            events: vec![],
            required_contexts: vec![],
            lambda: FuncLambda::new(move |_, _, _, inputs, _, outputs| {
                Box::pin(async move {
                    assert_eq!(inputs.len(), 1);
                    assert_eq!(outputs.len(), 1);

                    let path = inputs[0].value.as_fs_path().unwrap();
                    let image = imaginarium::Image::read_file(path).map_err(anyhow::Error::from)?;

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
            inputs: vec![
                FuncInput {
                    name: "image".to_string(),
                    required: true,
                    data_type: IMAGE_DATA_TYPE.clone(),
                    default_value: None,
                    value_options: vec![],
                },
                FuncInput {
                    name: "path".to_string(),
                    required: true,
                    data_type: DataType::FsPath(FsPathConfig::with_extensions(
                        FsPathMode::NewFile,
                        SUPPORTED_EXTENSIONS.iter().map(|s| s.to_string()).collect(),
                    )),
                    default_value: None,
                    value_options: vec![],
                },
            ],
            outputs: vec![],
            events: vec![],
            required_contexts: vec![VISION_CTX_TYPE.clone()],
            lambda: FuncLambda::new(move |ctx_manager, _, _, inputs, _, _| {
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
            }),
            ..Default::default()
        });

        // convert
        func_lib.add(Func {
            id: "80aa1ee7-3b75-4200-b480-b9db913bd6eb".into(),
            name: "convert".to_string(),
            description: Some("Converts image to a different color format".to_string()),
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
                    name: "format".to_string(),
                    required: true,
                    data_type: CONVERSION_FORMAT_DATATYPE.clone(),
                    default_value: Some(CONVERSION_FORMAT_DATATYPE.default_value()),
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
            }),
            ..Default::default()
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
                    data_type: BLENDMODE_DATATYPE.clone(),
                    default_value: Some(BLENDMODE_DATATYPE.default_value()),
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
                        .map_err(anyhow::Error::from)?;

                    outputs[0] = DynamicValue::from_custom(Image::from(output_buffer));

                    Ok(())
                })
            }),
            ..Default::default()
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
                        .map_err(anyhow::Error::from)?;

                    outputs[0] = DynamicValue::from_custom(Image::from(output_buffer));

                    Ok(())
                })
            }),
            ..Default::default()
        });

        Self { func_lib }
    }
}
