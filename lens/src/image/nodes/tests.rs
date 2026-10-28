use common::{Buffer2, Size2us, TempDir};
use scenarium::internals::func_invoker::FuncInvoker;

use imaginarium::{ColorFormat, PlanarImage};
use scenarium::{ConstValue, DynamicValue};

use crate::image::format::{AS_IS, CONVERSION_FORMAT_DATATYPE};
use crate::image::nodes::image_library;
use crate::image::{IMAGE_DATA_TYPE, Image};

#[test]
fn enum_defaults_are_exact() {
    let library = image_library();
    let convert = library.by_name("Convert").unwrap();
    assert_eq!(convert.inputs()[0].data_type, IMAGE_DATA_TYPE);
    assert_eq!(convert.inputs()[1].data_type, CONVERSION_FORMAT_DATATYPE);
    assert_eq!(
        convert.inputs()[1].default_value,
        Some(ConstValue::Enum("RGB u8".to_string())),
    );

    let save = library.by_name("Save Image").unwrap();
    let names: Vec<&str> = save
        .inputs()
        .iter()
        .map(|input| input.name.as_str())
        .collect();
    assert_eq!(names, ["Image", "Path", "Format"]);
    assert_eq!(
        save.inputs()[2].default_value,
        Some(ConstValue::Enum(AS_IS.to_string())),
    );

    let blend = library.by_name("Blend").unwrap();
    assert_eq!(blend.inputs()[2].name, "Mode");
    assert_eq!(
        blend.inputs()[2].default_value,
        Some(ConstValue::Enum("Normal".to_string())),
    );

    for (name, idx) in [("Brightness / Contrast", 3), ("Blend", 4)] {
        let func = library.by_name(name).unwrap();
        assert_eq!(func.inputs()[idx].name, "Clamp", "{name}");
        assert_eq!(
            func.inputs()[idx].default_value,
            Some(ConstValue::Enum("Black".to_string())),
            "{name}"
        );
    }
}

#[tokio::test]
async fn load_and_save_round_trip_exact_pixels() {
    let dir = TempDir::new("lens-image-io");
    let path = dir.join("roundtrip.png");
    let desc = imaginarium::InterleavedDesc::new(Size2us::new(2, 1), ColorFormat::RGB_U8);
    let pixels = vec![10, 20, 30, 40, 50, 60];
    let image = imaginarium::InterleavedImage::new_with_data(desc, pixels.clone()).unwrap();
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

    // A planar image saved as another format is interleaved and converted on the way out: RGB
    // f32 planes of 0, 1 and 0.2 store as the bytes 0, 255 and round(0.2 · 255) = 51.
    let planes =
        PlanarImage::new_rgb([0.0, 1.0, 0.2].map(|level| Buffer2::new_filled((2, 1), level)));
    let converted = dir.join("converted.png");
    invoker
        .call(
            library.by_name("Save Image").unwrap(),
            [
                DynamicValue::from_custom(Image::from(planes)),
                ConstValue::FsPath(converted.display().to_string()).into(),
                ConstValue::Enum(ColorFormat::RGB_U8.to_string()).into(),
            ],
        )
        .await
        .unwrap();
    let outputs = invoker
        .call(
            library.by_name("Load Image").unwrap(),
            [ConstValue::FsPath(converted.display().to_string()).into()],
        )
        .await
        .unwrap();
    let cpu = outputs[0]
        .as_custom::<Image>()
        .unwrap()
        .interleaved()
        .into_owned();
    assert_eq!(cpu.desc(), desc);
    assert_eq!(cpu.bytes(), [0, 255, 51, 0, 255, 51]);
}

/// Source and destination are two independent wires, so a size or format mismatch is the user's
/// graph: the node fails with an input error naming the destination, instead of the blend kernel's
/// assert taking the worker down. An alpha past [0, 1], NaN included, fails naming the alpha,
/// as the mix would extrapolate beyond both images; both ends run.
#[tokio::test]
async fn blend_refuses_a_mismatched_destination_and_an_alpha_out_of_range() {
    let library = image_library();
    let image = |width, format| {
        DynamicValue::from_custom(Image::from(imaginarium::InterleavedImage::new_zeroed(
            imaginarium::InterleavedDesc::new(Size2us::new(width, 2), format),
        )))
    };
    let blend = |source: DynamicValue, destination: DynamicValue, alpha: f64| {
        let inputs = [
            source,
            destination,
            ConstValue::Enum("Normal".to_owned()).into(),
            ConstValue::Float(alpha).into(),
            ConstValue::Enum("Black".to_owned()).into(),
        ];
        let blend = library.by_name("Blend").unwrap();
        async move { FuncInvoker::default().call(blend, inputs).await }
    };
    let rgb = || image(2, ColorFormat::RGB_U8);

    for destination in [
        image(3, ColorFormat::RGB_U8),
        image(2, ColorFormat::RGBA_U8),
    ] {
        assert!(matches!(
            blend(rgb(), destination, 0.5).await,
            Err(scenarium::InvokeError::InvalidInput { idx: 1, .. })
        ));
    }
    for alpha in [-0.1, 1.5, f64::NAN] {
        assert!(
            matches!(
                blend(rgb(), rgb(), alpha).await,
                Err(scenarium::InvokeError::InvalidInput { idx: 3, .. })
            ),
            "alpha {alpha}"
        );
    }
    for alpha in [0.0, 0.5, 1.0] {
        assert!(blend(rgb(), rgb(), alpha).await.is_ok(), "alpha {alpha}");
    }
}

/// A zero or non-finite scale, and a non-finite rotation or shift, fail the node with an input
/// error instead of crashing the worker; the identity runs.
#[tokio::test]
async fn transform_refuses_a_transform_with_no_inverse() {
    let library = image_library();
    let transform = library.by_name("Transform").unwrap();
    let run = |values: [f64; 5]| {
        let image =
            DynamicValue::from_custom(Image::from(imaginarium::InterleavedImage::new_zeroed(
                imaginarium::InterleavedDesc::new(Size2us::new(2, 2), ColorFormat::MONO_U8),
            )));
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
                Err(scenarium::InvokeError::InvalidInput { idx: 1, .. })
            ),
            "{values:?}"
        );
    }
    assert!(run([1.0, 1.0, 0.0, 0.0, 0.0]).await.is_ok());
}
