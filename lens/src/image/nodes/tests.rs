use common::TempDir;
use scenarium::internals::func_invoker::FuncInvoker;

use imaginarium::ColorFormat;
use scenarium::{ConstValue, DynamicValue};

use crate::image::format::{AS_IS, CONVERSION_FORMAT_DATATYPE};
use crate::image::nodes::image_library;
use crate::image::{IMAGE_DATA_TYPE, Image};

#[test]
fn enum_defaults_are_exact() {
    let library = image_library();
    let convert = library.by_name("Convert").unwrap();
    assert_eq!(convert.inputs[0].data_type, IMAGE_DATA_TYPE);
    assert_eq!(convert.inputs[1].data_type, CONVERSION_FORMAT_DATATYPE);
    assert_eq!(
        convert.inputs[1].default_value,
        Some(ConstValue::Enum("RGB u8".to_string())),
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
        Some(ConstValue::Enum(AS_IS.to_string())),
    );

    let blend = library.by_name("Blend").unwrap();
    assert_eq!(blend.inputs[2].name, "Mode");
    assert_eq!(
        blend.inputs[2].default_value,
        Some(ConstValue::Enum("Normal".to_string())),
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
                ConstValue::Enum(AS_IS.to_string()).into(),
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

/// A zero or non-finite scale, and a non-finite rotation or shift, fail the node with an input
/// error instead of crashing the worker; the identity runs.
#[tokio::test]
async fn transform_refuses_a_transform_with_no_inverse() {
    let library = image_library();
    let transform = library.by_name("Transform").unwrap();
    let run = |values: [f64; 5]| {
        let image = DynamicValue::from_custom(Image::from(
            imaginarium::Image::new_black(imaginarium::ImageDesc::new(2, 2, ColorFormat::L_U8))
                .unwrap(),
        ));
        let inputs = [image]
            .into_iter()
            .chain(values.map(|value| ConstValue::Float(value).into()))
            .collect::<Vec<DynamicValue>>();
        async move { FuncInvoker::default().call(transform, inputs).await }
    };

    for values in [
        [0.0, 1.0, 0.0, 0.0, 0.0],
        [1.0, f64::NAN, 0.0, 0.0, 0.0],
        [1.0, 1.0, f64::INFINITY, 0.0, 0.0],
        [1.0, 1.0, 0.0, 0.0, f64::NAN],
    ] {
        assert!(
            matches!(
                run(values).await,
                Err(scenarium::InvokeError::InvalidInput { index: 1, .. })
            ),
            "{values:?}"
        );
    }
    assert!(run([1.0, 1.0, 0.0, 0.0, 0.0]).await.is_ok());
}
