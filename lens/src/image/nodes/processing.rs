//! In-memory image adjustment, conversion, blending, and transform nodes.

use imaginarium::{Blend, BlendMode, ColorFormat, ContrastBrightness, Transform, Vec2};
use scenarium::FuncId;
use scenarium::Invocation;
use scenarium::async_lambda;
use scenarium::{ConstValue, DataType, DynamicValue, InvokeError};
use scenarium::{Func, FuncInput, FuncOutput, Library};

use crate::config_node::enum_input;
use crate::image::format::{CONVERSION_FORMAT_DATATYPE, ConversionFormat};
use crate::image::nodes::BLENDMODE_DATATYPE;
use crate::image::{IMAGE_DATA_TYPE, Image};
use std::mem;

const BRIGHTNESS_CONTRAST_FUNC_ID: FuncId = FuncId::literal("b8c3d4e5-f6a7-4b8c-9d0e-1f2a3b4c5d6e");
const CONVERT_FUNC_ID: FuncId = FuncId::literal("80aa1ee7-3b75-4200-b480-b9db913bd6eb");
const BLEND_FUNC_ID: FuncId = FuncId::literal("975cc74b-8412-4293-b2cb-ef8d41fdd9b3");
const TRANSFORM_FUNC_ID: FuncId = FuncId::literal("d3e4f5a6-b7c8-4d9e-0f1a-2b3c4d5e6f7a");

pub(super) fn register(library: &mut Library) {
    register_brightness(library);
    register_convert(library);
    register_blend(library);
    register_transform(library);
}

fn register_brightness(library: &mut Library) {
    library.add(
        Func::new(
            BRIGHTNESS_CONTRAST_FUNC_ID,
            "Brightness / Contrast",
            async_lambda!(move |Invocation {
                                    inputs, outputs, ..
                                }| {
                debug_assert_eq!(inputs.len(), 3);
                debug_assert_eq!(outputs.len(), 1);
                let value = mem::take(&mut inputs[0]);
                let brightness = inputs[1].required_f64() as f32;
                let contrast = inputs[2].required_f64() as f32;
                let image = adjust_image(ContrastBrightness::new(contrast, brightness), value);
                outputs[0] = DynamicValue::from_custom(image);
                Ok(())
            }),
        )
        .description("Adjusts the brightness and contrast of an image.")
        .category("Image")
        .pure()
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
        .output(FuncOutput::new("Image", IMAGE_DATA_TYPE.clone()).description("Adjusted image.")),
    );
}

fn register_convert(library: &mut Library) {
    library.add(
        Func::new(
            CONVERT_FUNC_ID,
            "Convert",
            async_lambda!(move |Invocation {
                                    inputs, outputs, ..
                                }| {
                debug_assert_eq!(inputs.len(), 2);
                debug_assert_eq!(outputs.len(), 1);
                let value = mem::take(&mut inputs[0]);
                let format = inputs[1].required_enum_as::<ConversionFormat>();
                let converted = {
                    let image = value.required_custom::<Image>();
                    format
                        .target(image.desc().color_format)
                        .map(|target| image.interleaved().convert_to(target))
                };
                outputs[0] = match converted {
                    Some(image) => DynamicValue::from_custom(Image::from(image)),
                    None => value,
                };
                Ok(())
            }),
        )
        .description("Converts an image to a different color format.")
        .category("Image")
        .pure()
        .input(
            FuncInput::required("Image", IMAGE_DATA_TYPE.clone()).description("Image to convert."),
        )
        .input(
            enum_input::<ConversionFormat>("Format", &CONVERSION_FORMAT_DATATYPE)
                .default(ConstValue::Enum(ColorFormat::RGB_U8.name().to_string()))
                .description("Target color format."),
        )
        .output(FuncOutput::new("Image", IMAGE_DATA_TYPE.clone()).description("Converted image.")),
    );
}

fn register_blend(library: &mut Library) {
    library.add(
        Func::new(
            BLEND_FUNC_ID,
            "Blend",
            async_lambda!(move |Invocation {
                                    inputs, outputs, ..
                                }| {
                debug_assert_eq!(inputs.len(), 4);
                debug_assert_eq!(outputs.len(), 1);
                let source = inputs[0].required_custom::<Image>();
                let destination = inputs[1].required_custom::<Image>();
                let mode = inputs[2].required_enum_as::<BlendMode>();
                let alpha = inputs[3].required_f64() as f32;
                // Two independent wires: a different size or format is the user's
                // graph, not a broken invariant, and the blend kernel asserts on it.
                if destination.desc() != source.desc() {
                    return Err(InvokeError::invalid_input(
                        1,
                        "an image with the source's size and format",
                        destination.desc(),
                    ));
                }
                let mut output =
                    imaginarium::Image::new_black(source.desc()).map_err(InvokeError::external)?;
                Blend::new(mode, alpha).apply_cpu(
                    &source.interleaved(),
                    &destination.interleaved(),
                    &mut output,
                );
                outputs[0] = DynamicValue::from_custom(Image::from(output));
                Ok(())
            }),
        )
        .description("Blends two images using the selected blend mode.")
        .category("Image")
        .pure()
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
        .output(FuncOutput::new("Image", IMAGE_DATA_TYPE.clone()).description("Blended image.")),
    );
}

fn register_transform(library: &mut Library) {
    library.add(
        Func::new(
            TRANSFORM_FUNC_ID,
            "Transform",
            async_lambda!(move |Invocation {
                                    inputs, outputs, ..
                                }| {
                debug_assert_eq!(inputs.len(), 6);
                debug_assert_eq!(outputs.len(), 1);
                let image = inputs[0].required_custom::<Image>();
                let scalar = |index: usize| inputs[index].required_f64() as f32;
                let center = Vec2::new(
                    image.desc().width as f32 / 2.0,
                    image.desc().height as f32 / 2.0,
                );
                let scale = Vec2::new(scalar(1), scalar(2));
                let transform = Transform::new()
                    .scale(scale)
                    .rotate_around(scalar(3), center)
                    .translate(Vec2::new(scalar(4), scalar(5)));
                if !transform.is_invertible() {
                    return Err(InvokeError::invalid_input(
                        1,
                        "a scale, rotation and translation that make an invertible transform",
                        (1..6).map(scalar).collect::<Vec<f32>>(),
                    ));
                }
                let mut output =
                    imaginarium::Image::new_black(image.desc()).map_err(InvokeError::external)?;
                transform.apply_cpu(&image.interleaved(), &mut output);
                outputs[0] = DynamicValue::from_custom(Image::from(output));
                Ok(())
            }),
        )
        .description("Applies scale, rotation, and translation to an image.")
        .category("Image")
        .pure()
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
        ),
    );
}

fn adjust_image(op: ContrastBrightness, value: DynamicValue) -> Image {
    // Contrast/brightness works in place, so an owned input is adjusted where it stands — no output
    // image to allocate. Only a value still shared with other consumers has to be copied first.
    let mut image = Image::take_interleaved(value);
    op.apply_cpu(&mut image);
    Image::from(image)
}

#[cfg(test)]
mod tests {
    use imaginarium::{ColorFormat, ContrastBrightness};
    use scenarium::DynamicValue;

    use crate::image::Image;
    use crate::image::nodes::processing::adjust_image;

    #[test]
    fn adjust_image_runs_in_place_only_for_unique_cpu_inputs() {
        let desc = imaginarium::ImageDesc::new(9, 4, ColorFormat::RGBA_U8);
        let op = ContrastBrightness::new(1.5, 0.1);
        let pattern: Vec<u8> = (0..desc.size_in_bytes())
            .map(|index| (index % 251) as u8)
            .collect();
        let patterned_image = || {
            let mut image = imaginarium::Image::new_black(desc).unwrap();
            image.bytes_mut().copy_from_slice(&pattern);
            image
        };

        let image = patterned_image();
        let unique_ptr = image.bytes().as_ptr();
        let unique = DynamicValue::from_custom(Image::from(image));
        let adjusted = adjust_image(op, unique);
        let adjusted_cpu = adjusted.interleaved();
        assert_eq!(adjusted_cpu.bytes().as_ptr(), unique_ptr);
        assert_ne!(adjusted_cpu.bytes(), pattern.as_slice());

        let image = patterned_image();
        let shared_ptr = image.bytes().as_ptr();
        let shared = DynamicValue::from_custom(Image::from(image));
        let holder = shared.clone();
        let adjusted_shared = adjust_image(op, shared);
        let shared_cpu = adjusted_shared.interleaved();
        assert_ne!(shared_cpu.bytes().as_ptr(), shared_ptr);
        let original = holder.as_custom::<Image>().unwrap();
        let original_cpu = original.interleaved();
        assert_eq!(original_cpu.bytes().as_ptr(), shared_ptr);
        assert_eq!(original_cpu.bytes(), pattern.as_slice());
        assert_eq!(adjusted_cpu.bytes(), shared_cpu.bytes());
    }
}
