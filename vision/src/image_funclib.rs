use std::sync::LazyLock;

use graph::async_lambda;
use graph::data::{DataType, TypeId};
use graph::function::{Func, FuncBehavior, FuncInput, FuncLib, FuncOutput};
use imaginarium::ContrastBrightness;

use crate::vision_ctx::{VISION_CTX_TYPE, VisionCtx};

pub static IMAGE_BUFFER_TYPE_ID: LazyLock<TypeId> =
    LazyLock::new(|| "a1b2c3d4-image-type-0001-000000000001".into());

fn image_buffer_data_type() -> DataType {
    DataType::Custom {
        type_id: *IMAGE_BUFFER_TYPE_ID,
        type_name: "ImageBuffer".to_string(),
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
                    data_type: image_buffer_data_type(),
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
                data_type: image_buffer_data_type(),
            }],
            events: vec![],
            required_contexts: vec![VISION_CTX_TYPE.clone()],
            lambda: async_lambda!(move |ctx_manager, _, _, inputs, _, outputs| {
                assert_eq!(inputs.len(), 3);
                assert_eq!(outputs.len(), 1);

                let input_buffer = inputs[0]
                    .value
                    .as_custom()
                    .downcast_ref::<imaginarium::ImageBuffer>()
                    .expect("Input should be an ImageBuffer");

                let brightness = inputs[1].value.as_f64() as f32;
                let contrast = inputs[2].value.as_f64() as f32;

                let vision_ctx = ctx_manager.get::<VisionCtx>(&VISION_CTX_TYPE);

                let mut output_buffer =
                    imaginarium::ImageBuffer::new_empty(*input_buffer.desc());

                ContrastBrightness::new(contrast, brightness)
                    .execute(&mut vision_ctx.processing_ctx, input_buffer, &mut output_buffer)
                    .expect("Failed to apply brightness/contrast");

                outputs[0] = graph::data::DynamicValue::Custom {
                    data_type: image_buffer_data_type(),
                    data: Box::new(output_buffer),
                };

                Ok(())
            }),
        });

        Self { func_lib }
    }
}
