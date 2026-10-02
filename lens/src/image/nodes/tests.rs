use common::TempDir;
use scenarium::testing::func_invoker::FuncInvoker;

use imaginarium::ColorFormat;
use scenarium::{ConstValue, DynamicValue};

use crate::image::format::{CONVERSION_FORMAT_DATATYPE, ConversionFormat, conversion_target};
use crate::image::nodes::image_library;
use crate::image::{IMAGE_DATA_TYPE, Image};

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
    let convert = library.by_name("Convert").unwrap();
    assert_eq!(convert.inputs[0].data_type, IMAGE_DATA_TYPE);
    assert_eq!(convert.inputs[1].data_type, CONVERSION_FORMAT_DATATYPE);
    assert_eq!(
        convert.inputs[1].default_value,
        Some(ConstValue::Enum(ConversionFormat::RgbU8.label())),
    );

    let save = library.by_name("Save Image").unwrap();
    let names: Vec<&str> = save
        .inputs
        .iter()
        .map(|input| input.name.as_str())
        .collect();
    assert_eq!(names, ["Image", "Path", "Format"]);
    assert_eq!(
        save.inputs[2].default_value,
        Some(ConstValue::Enum(ConversionFormat::AsIs.label())),
    );
}

#[tokio::test]
async fn load_and_save_round_trip_exact_pixels() {
    let dir = TempDir::new("lens-image-io");
    let path = dir.join("roundtrip.png");
    let desc = imaginarium::ImageDesc::new(2, 1, ColorFormat::RGB_U8);
    let pixels = vec![10, 20, 30, 40, 50, 60];
    let image = imaginarium::Image::new_with_data(desc, pixels.clone()).unwrap();
    let library = image_library();

    let mut invoker = FuncInvoker::default();
    invoker
        .call(
            library.by_name("Save Image").unwrap(),
            [
                DynamicValue::from_custom(Image::from(image)),
                ConstValue::FsPath(path.display().to_string()).into(),
                ConstValue::Enum(ConversionFormat::AsIs.label()).into(),
            ],
        )
        .await
        .unwrap();
    let outputs = invoker
        .call(
            library.by_name("Load Image").unwrap(),
            [ConstValue::FsPath(path.display().to_string()).into()],
        )
        .await
        .unwrap();
    let loaded = outputs[0].as_custom::<Image>().unwrap();
    let cpu = loaded.interleaved();
    assert_eq!(cpu.desc(), desc);
    assert_eq!(cpu.bytes(), pixels);
}

/// Source and destination are two independent wires, so a size or format mismatch is the user's
/// graph: the node fails with an input error naming the destination, instead of the blend kernel's
/// assert taking the worker down.
#[tokio::test]
async fn blend_refuses_a_destination_of_another_size_or_format() {
    let library = image_library();
    let image = |width, format| {
        DynamicValue::from_custom(Image::from(
            imaginarium::Image::new_black(imaginarium::ImageDesc::new(width, 2, format)).unwrap(),
        ))
    };
    let blend = |source: DynamicValue, destination: DynamicValue| {
        let inputs = [
            source,
            destination,
            ConstValue::Enum("Normal".to_owned()).into(),
            ConstValue::Float(0.5).into(),
        ];
        let blend = library.by_name("Blend").unwrap();
        async move { FuncInvoker::default().call(blend, inputs).await }
    };

    for destination in [
        image(3, ColorFormat::RGB_U8),
        image(2, ColorFormat::RGBA_U8),
    ] {
        assert!(matches!(
            blend(image(2, ColorFormat::RGB_U8), destination).await,
            Err(scenarium::InvokeError::InvalidInput { index: 1, .. })
        ));
    }
    assert!(
        blend(image(2, ColorFormat::RGB_U8), image(2, ColorFormat::RGB_U8))
            .await
            .is_ok()
    );
}
