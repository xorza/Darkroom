use std::sync::LazyLock;

use graph::async_lambda;
use graph::data::DataType;
use graph::function::{Func, FuncBehavior, FuncInput, FuncLib, FuncOutput};
use imaginarium::ContrastBrightness;

use crate::vision_ctx::{VISION_CTX_TYPE, VisionCtx};

pub static IMAGE_BUFFER_DATA_TYPE: LazyLock<DataType> = LazyLock::new(|| DataType::Custom {
    type_id: "a69f9a9c-3be7-4d8b-abb1-dbd5c9ee4da2".into(),
    type_name: "ImageBuffer".to_string(),
});

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
                    data_type: IMAGE_BUFFER_DATA_TYPE.clone(),
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
                data_type: IMAGE_BUFFER_DATA_TYPE.clone(),
            }],
            events: vec![],
            required_contexts: vec![VISION_CTX_TYPE.clone()],
            lambda: async_lambda!(move |ctx_manager, _, _, inputs, _, outputs| {
                assert_eq!(inputs.len(), 3);
                assert_eq!(outputs.len(), 1);

                let input_buffer = inputs[0].value.as_custom::<imaginarium::ImageBuffer>();

                let brightness = inputs[1].value.as_f64() as f32;
                let contrast = inputs[2].value.as_f64() as f32;

                let vision_ctx = ctx_manager.get::<VisionCtx>(&VISION_CTX_TYPE);

                let mut output_buffer =
                    imaginarium::ImageBuffer::new_empty(*input_buffer.desc());

                ContrastBrightness::new(contrast, brightness)
                    .execute(&mut vision_ctx.processing_ctx, input_buffer, &mut output_buffer)
                    .expect("Failed to apply brightness/contrast");

                outputs[0] = graph::data::DynamicValue::Custom {
                    data_type: IMAGE_BUFFER_DATA_TYPE.clone(),
                    data: Box::new(output_buffer),
                };

                Ok(())
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
                data_type: IMAGE_BUFFER_DATA_TYPE.clone(),
            }],
            events: vec![],
            required_contexts: vec![],
            lambda: async_lambda!(move |_, _, _, _, _, outputs| {
                assert_eq!(outputs.len(), 1);

                // For now, always load lena.tiff from test_resources
                let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../test_resources/lena.tiff");
                let image = imaginarium::Image::read_file(path).expect("Failed to load image");
                let buffer = imaginarium::ImageBuffer::from(image);

                outputs[0] = graph::data::DynamicValue::Custom {
                    data_type: IMAGE_BUFFER_DATA_TYPE.clone(),
                    data: Box::new(buffer),
                };

                Ok(())
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
                data_type: IMAGE_BUFFER_DATA_TYPE.clone(),
                default_value: None,
                value_options: vec![],
            }],
            outputs: vec![],
            events: vec![],
            required_contexts: vec![VISION_CTX_TYPE.clone()],
            lambda: async_lambda!(move |ctx_manager, _, _, inputs, _, _| {
                assert_eq!(inputs.len(), 1);

                let input_buffer = inputs[0].value.as_custom::<imaginarium::ImageBuffer>();

                // For now, save to test_output directory
                let output_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../test_output");
                std::fs::create_dir_all(output_dir).expect("Failed to create output directory");

                let path = format!("{}/vision_output.tiff", output_dir);
                let vision_ctx = ctx_manager.get::<VisionCtx>(&VISION_CTX_TYPE);
                let image = input_buffer
                    .make_cpu(&vision_ctx.processing_ctx)
                    .expect("Failed to get CPU image");
                image.save_file(&path).expect("Failed to save image");

                Ok(())
            }),
        });

        Self { func_lib }
    }
}
