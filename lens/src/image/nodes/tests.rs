use std::fs;
use std::path::PathBuf;

use imaginarium::{ColorFormat, ContrastBrightness};
use scenarium::{
    AnyState, ContextManager, DynamicValue, Func, InvokeInput, OutputDemand, SharedAnyState,
    StaticValue,
};

use crate::image::format::{CONVERSION_FORMAT_DATATYPE, ConversionFormat, conversion_target};
use crate::image::nodes::image_library;
use crate::image::nodes::processing::adjust_image;
use crate::image::{IMAGE_DATA_TYPE, Image};

fn func<'a>(library: &'a scenarium::Library, name: &str) -> &'a Func {
    library
        .funcs()
        .find(|function| function.name == name)
        .unwrap_or_else(|| panic!("{name} registered"))
}

#[test]
fn adjust_image_runs_in_place_only_for_unique_cpu_inputs() {
    let desc = imaginarium::ImageDesc::new(9, 4, ColorFormat::RGBA_U8);
    let op = ContrastBrightness::new(1.5, 0.1);
    let mut context = imaginarium::ProcessingContext::cpu_only();
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
    let adjusted = adjust_image(op, &mut context, unique).unwrap();
    let adjusted_cpu = adjusted.buffer.make_cpu(&context).unwrap();
    assert_eq!(adjusted_cpu.bytes().as_ptr(), unique_ptr);
    assert_ne!(adjusted_cpu.bytes(), pattern.as_slice());

    let image = patterned_image();
    let shared_ptr = image.bytes().as_ptr();
    let shared = DynamicValue::from_custom(Image::from(image));
    let holder = shared.clone();
    let adjusted_shared = adjust_image(op, &mut context, shared).unwrap();
    let shared_cpu = adjusted_shared.buffer.make_cpu(&context).unwrap();
    assert_ne!(shared_cpu.bytes().as_ptr(), shared_ptr);
    let original = holder.as_custom::<Image>().unwrap();
    let original_cpu = original.buffer.make_cpu(&context).unwrap();
    assert_eq!(original_cpu.bytes().as_ptr(), shared_ptr);
    assert_eq!(original_cpu.bytes(), pattern.as_slice());
    assert_eq!(adjusted_cpu.bytes(), shared_cpu.bytes());
}

#[test]
fn conversion_target_collapses_as_is_and_matching_format() {
    assert_eq!(conversion_target("As Is", ColorFormat::RGB_U8), None);
    assert_eq!(conversion_target("RGB u8", ColorFormat::RGB_U8), None);
    assert_eq!(
        conversion_target("RGB u8", ColorFormat::RGBA_F32),
        Some(ColorFormat::RGB_U8)
    );
}

#[test]
fn format_defaults_are_exact() {
    let library = image_library();
    let convert = func(&library, "Convert");
    assert_eq!(convert.inputs[0].data_type, *IMAGE_DATA_TYPE);
    assert_eq!(convert.inputs[1].data_type, *CONVERSION_FORMAT_DATATYPE);
    assert_eq!(
        convert.inputs[1].default_value,
        Some(StaticValue::Enum(ConversionFormat::RgbU8.label())),
    );

    let save = func(&library, "Save Image");
    let names: Vec<&str> = save
        .inputs
        .iter()
        .map(|input| input.name.as_str())
        .collect();
    assert_eq!(names, ["Image", "Path", "Format"]);
    assert_eq!(
        save.inputs[2].default_value,
        Some(StaticValue::Enum(ConversionFormat::AsIs.label())),
    );
}

#[tokio::test]
async fn load_and_save_round_trip_exact_pixels() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("test_output/lens/image_io");
    if dir.exists() {
        fs::remove_dir_all(&dir).unwrap();
    }
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("roundtrip.png");
    let desc = imaginarium::ImageDesc::new(2, 1, ColorFormat::RGB_U8);
    let pixels = vec![10, 20, 30, 40, 50, 60];
    let image = imaginarium::Image::new_with_data(desc, pixels.clone()).unwrap();
    let library = image_library();

    let mut save_inputs = [
        InvokeInput {
            value: DynamicValue::from_custom(Image::from(image)),
        },
        InvokeInput {
            value: StaticValue::FsPath(path.display().to_string()).into(),
        },
        InvokeInput {
            value: StaticValue::Enum(ConversionFormat::AsIs.label()).into(),
        },
    ];
    func(&library, "Save Image")
        .lambda
        .invoke(
            &mut ContextManager::default(),
            &mut AnyState::default(),
            &SharedAnyState::default(),
            &mut save_inputs,
            &[],
            &mut [],
        )
        .await
        .unwrap();

    let mut load_inputs = [InvokeInput {
        value: StaticValue::FsPath(path.display().to_string()).into(),
    }];
    let mut outputs = [DynamicValue::Unbound];
    func(&library, "Load Image")
        .lambda
        .invoke(
            &mut ContextManager::default(),
            &mut AnyState::default(),
            &SharedAnyState::default(),
            &mut load_inputs,
            &[OutputDemand::Produce],
            &mut outputs,
        )
        .await
        .unwrap();
    let loaded = outputs[0].as_custom::<Image>().unwrap();
    let cpu = loaded
        .buffer
        .make_cpu(&imaginarium::ProcessingContext::cpu_only())
        .unwrap();
    assert_eq!(cpu.desc(), desc);
    assert_eq!(cpu.bytes(), pixels);
    fs::remove_dir_all(dir).unwrap();
}
