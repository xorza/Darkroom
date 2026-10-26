//! The astro library's node shapes, its path types, and the builder
//! nodes' invocation at the input boundary.

use std::borrow::Cow;

use common::TempDir;
use imaginarium::{ChannelLayout, PlanarDesc, RasterFormat};
use lumos::{
    Denoise, ExtractBg, Hdr, LinearImage, LocalContrast, PREVIEW_IMAGE_EXTENSIONS, RAW_EXTENSIONS,
    SigmaThreshold, SourceProvenance,
};
use scenarium::internals::func_invoker::FuncInvoker;
use scenarium::{ConstValue, DataType, DynamicValue, FsPathMode, FuncBehavior};

use crate::astro::masters::MASTERS_DATA_TYPE;
use crate::astro::nodes::io::{ASTRO_IMAGE_PATH_DATA_TYPE, ASTRO_RAW_PATHS_DATA_TYPE};
use crate::astro::nodes::{MlModelPaths, astro_library};
use crate::config_node::config_data_type;
use crate::image::{IMAGE_DATA_TYPE, Image};

#[test]
fn astro_image_path_filter_matches_preview_extensions() {
    let DataType::FsPath(cfg) = &*ASTRO_IMAGE_PATH_DATA_TYPE else {
        panic!("expected an FsPath data type");
    };
    assert_eq!(cfg.mode, FsPathMode::ExistingFile);
    assert_eq!(cfg.extensions, PREVIEW_IMAGE_EXTENSIONS);
}

#[test]
fn astro_raw_paths_are_a_filtered_multi_file_picker() {
    let DataType::FsPath(cfg) = &*ASTRO_RAW_PATHS_DATA_TYPE else {
        panic!("expected an FsPath data type");
    };
    assert_eq!(cfg.mode, FsPathMode::ExistingFiles);
    assert_eq!(cfg.extensions, RAW_EXTENSIONS);
}

/// The node keeps what the decoder stated, in planes for either file: a FITS's header and
/// provenance, and a TIFF's provenance, which states no header.
#[tokio::test]
async fn load_astro_image_keeps_the_decoded_metadata() {
    let dir = TempDir::new("lens-load-astro-image");
    let mut image = LinearImage::from_planar_channels(
        PlanarDesc::new((2, 1), ChannelLayout::Mono),
        [vec![0.25, 0.5]],
    );
    image.metadata.header.object = Some("M31".to_owned());
    let (fits, tiff) = (dir.join("light.fits"), dir.join("light.tiff"));
    image.write_fits(&fits).unwrap();
    image.write_raster(&tiff).unwrap();

    let library = astro_library(&MlModelPaths::default());
    let load = library.by_name("Load Astro Image").unwrap();
    for (path, object, from_fits) in [(&fits, Some("M31"), true), (&tiff, None, false)] {
        let outputs = FuncInvoker::default()
            .call(
                load,
                [ConstValue::FsPath(path.display().to_string()).into()],
            )
            .await
            .unwrap();
        let loaded = outputs[0].as_custom::<Image>().unwrap();
        assert!(
            matches!(loaded.planar(), Cow::Borrowed(_)),
            "{path:?} decodes to planes"
        );
        let planar = Image::take_planar(outputs[0].clone()).unwrap();
        let metadata = &planar.metadata;
        assert_eq!(metadata.header.object.as_deref(), object, "{path:?}");
        let source = metadata
            .derived
            .provenance()
            .map(|provenance| &provenance.source);
        if from_fits {
            assert!(
                matches!(source, Some(SourceProvenance::Fits(_))),
                "{source:?}"
            );
        } else {
            assert!(
                matches!(
                    source,
                    Some(SourceProvenance::Raster {
                        format: RasterFormat::Tiff,
                        ..
                    })
                ),
                "{source:?}"
            );
        }
    }
}

/// Every preset input is one required, wireable input of its detailed config's
/// wire type, the builder's output, holding a pick by default: on the three
/// preset processing nodes and on the stacking node's three stages.
#[test]
fn every_preset_input_takes_a_pick_or_its_builders_config() {
    let lib = astro_library(&MlModelPaths::default());
    // (node, input index, input name, builder)
    let cases = [
        ("Auto Stretch", 1, "Method", "Build Stretch Config"),
        ("Extract Background", 1, "Mode", "Build Background Config"),
        ("SCNR", 1, "Protection", "Build SCNR Config"),
        ("Stack Lights", 2, "Detection", "Build Detection Config"),
        (
            "Stack Lights",
            3,
            "Registration",
            "Build Registration Config",
        ),
        ("Stack Lights", 4, "Combine", "Build Combine Config"),
    ];
    for (node, idx, name, builder) in cases {
        let input = &lib.by_name(node).unwrap().inputs()[idx];
        assert_eq!(input.name, name, "{node}");
        assert!(input.required && !input.const_only, "{node} {name}");
        assert!(
            !input.value_variants.is_empty(),
            "{node} {name} offers picks"
        );
        assert_eq!(
            input.default_value.as_ref(),
            Some(&input.value_variants[0].value),
            "{node} {name} starts on its first pick"
        );
        let built = lib.by_name(builder).unwrap();
        assert_eq!(built.outputs()[0].data_type, input.data_type, "{builder}");
    }
}

/// An op of a few plain knobs takes them as its own inputs after the image,
/// one per field at the op's default, and has no config builder.
#[test]
fn a_few_knobs_are_the_nodes_own_inputs() {
    let lib = astro_library(&MlModelPaths::default());
    // The labels are the lumos field names title-cased.
    for (node, labels, builder) in [
        (
            "Denoise",
            &["Scales", "K", "Threshold", "Strength"][..],
            "Build Denoise Config",
        ),
        ("HDR Compression", &["Scales", "Amount"], "Build HDR Config"),
        (
            "Local Contrast",
            &["Tiles", "Clip Limit", "Strength"],
            "Build Local Contrast Config",
        ),
    ] {
        let inputs = &lib.by_name(node).unwrap().inputs()[1..];
        let names: Vec<&str> = inputs.iter().map(|input| input.name.as_str()).collect();
        assert_eq!(names, labels, "{node}");
        assert!(
            inputs.iter().all(|input| input.default_value.is_some()),
            "{node}"
        );
        assert!(lib.by_name(builder).is_none(), "{builder}");
    }

    // The strength each starts at is the op's own default.
    let default = |node: &str, name: &str| {
        lib.by_name(node)
            .unwrap()
            .inputs()
            .iter()
            .find(|input| input.name == name)
            .unwrap()
            .default_value
            .clone()
    };
    assert_eq!(
        default("Denoise", "Strength"),
        Some(ConstValue::Float(f64::from(Denoise::default().strength)))
    );
    assert_eq!(
        default("HDR Compression", "Amount"),
        Some(ConstValue::Float(f64::from(Hdr::default().amount)))
    );
    assert_eq!(
        default("Local Contrast", "Strength"),
        Some(ConstValue::Float(f64::from(
            LocalContrast::default().strength
        )))
    );
}

#[tokio::test]
async fn build_background_config_reflects_fields_and_rejects_invalid_values() {
    let lib = astro_library(&MlModelPaths::default());
    // The builder exposes one labeled input per BackgroundConfig field, in
    // struct order; all required (none are `Option`s).
    let builder = lib.by_name("Build Background Config").unwrap();
    assert_eq!(builder.category(), "Astro");
    let labels: Vec<&str> = builder.inputs().iter().map(|i| i.name.as_str()).collect();
    assert_eq!(
        labels,
        [
            "Tile Size",
            "Degree",
            "Mode",
            "Rejection Sigma",
            "Iterations",
            "Divide Floor"
        ]
    );
    assert!(builder.inputs().iter().all(|i| i.required));
    assert_eq!(builder.outputs()[0].name, "Config");
    assert_eq!(
        builder.outputs()[0].data_type,
        config_data_type::<ExtractBg>()
    );

    let mut inputs: Vec<DynamicValue> = builder
        .inputs()
        .iter()
        .map(|input| input.default_value.clone().unwrap().into())
        .collect();
    inputs[0] = ConstValue::Int(-1).into();
    let error = FuncInvoker::default()
        .call(builder, inputs)
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "field `tile_size` value -1 cannot be represented as usize"
    );
}

/// Each astro node is registered in the Astro category under its inputs and outputs, every output
/// an image but Build Masters', with what each states beside them: the picker types, the
/// optional sets, the defaults and the purity a digest relies on.
#[test]
fn astro_nodes_are_registered_with_their_ports() {
    let lib = astro_library(&MlModelPaths::default());
    let rows: [(&str, &[&str], &[&str]); 11] = [
        ("Load Astro Image", &["Path"], &["Image"]),
        (
            "Build Masters",
            &["Darks", "Flats", "Bias", "Flat Darks", "Sigma", "Cache"],
            &["Masters"],
        ),
        (
            "Stack Lights",
            &[
                "Lights",
                "Masters",
                "Detection",
                "Registration",
                "Combine",
                "Reference",
            ],
            &["Image", "Coverage", "Weight"],
        ),
        ("Extract Background", &["Image", "Mode"], &["Image"]),
        (
            "Denoise",
            &["Image", "Scales", "K", "Threshold", "Strength"],
            &["Image"],
        ),
        ("SCNR", &["Image", "Protection"], &["Image"]),
        ("Neutralize Background", &["Image"], &["Image"]),
        (
            "HDR Compression",
            &["Image", "Scales", "Amount"],
            &["Image"],
        ),
        (
            "Local Contrast",
            &["Image", "Tiles", "Clip Limit", "Strength"],
            &["Image"],
        ),
        ("ML Denoise", &["Image", "Model"], &["Image"]),
        (
            "ML Star Removal",
            &["Image", "Model"],
            &["Starless", "Stars"],
        ),
    ];
    for (name, inputs, outputs) in rows {
        let f = lib.by_name(name).unwrap();
        assert_eq!(f.category(), "Astro", "{name}");
        let input_names: Vec<&str> = f.inputs().iter().map(|i| i.name.as_str()).collect();
        assert_eq!(input_names, inputs, "{name} inputs");
        let output_names: Vec<&str> = f.outputs().iter().map(|o| o.name.as_str()).collect();
        assert_eq!(output_names, outputs, "{name} outputs");
        if inputs[0] == "Image" {
            assert_eq!(f.inputs()[0].data_type, IMAGE_DATA_TYPE, "{name}");
            assert!(f.inputs()[0].required, "{name} image required");
        }
        let output_type = if name == "Build Masters" {
            MASTERS_DATA_TYPE
        } else {
            IMAGE_DATA_TYPE
        };
        assert!(
            f.outputs().iter().all(|o| o.data_type == output_type),
            "{name} output types"
        );
    }

    let load = lib.by_name("Load Astro Image").unwrap();
    assert_eq!(load.inputs()[0].data_type, *ASTRO_IMAGE_PATH_DATA_TYPE);

    // Pure: the digest folds each selected calibration file's identity.
    let masters = lib.by_name("Build Masters").unwrap();
    assert_eq!(masters.behavior(), FuncBehavior::Pure);
    for input in &masters.inputs()[..4] {
        assert!(!input.required, "calibration frame sets are optional");
        assert_eq!(input.data_type, *ASTRO_RAW_PATHS_DATA_TYPE);
    }
    assert_eq!(masters.inputs()[4].data_type, DataType::Float);
    assert_eq!(
        masters.inputs()[4].default_value,
        Some(ConstValue::Float(f64::from(SigmaThreshold::DEFAULT.get()))),
    );
    assert_eq!(masters.inputs()[5].data_type, DataType::Bool);
    assert_eq!(
        masters.inputs()[5].default_value,
        Some(ConstValue::Bool(true))
    );

    // Pure: the digest folds exactly the selected light files. Masters are optional, and an unset
    // reference picks the sharpest frame with enough stars, with no sentinel standing in for it.
    let stack = lib.by_name("Stack Lights").unwrap();
    assert_eq!(stack.behavior(), FuncBehavior::Pure);
    assert_eq!(stack.inputs()[0].data_type, *ASTRO_RAW_PATHS_DATA_TYPE);
    assert!(stack.inputs()[0].required, "light frames are required");
    assert_eq!(stack.inputs()[1].data_type, MASTERS_DATA_TYPE);
    assert!(!stack.inputs()[1].required, "masters are optional");
    assert!(!stack.inputs()[5].required);
    assert_eq!(stack.inputs()[5].default_value, None);

    for (name, model) in [
        ("ML Denoise", "DeepSNR_weights_v2.onnx"),
        ("ML Star Removal", "StarNet2_weights.onnx"),
    ] {
        let f = lib.by_name(name).unwrap();
        let DataType::FsPath(config) = &f.inputs()[1].data_type else {
            panic!("{name}: the model is a file path");
        };
        assert_eq!(config.mode, FsPathMode::ExistingFile, "{name}");
        assert_eq!(config.extensions, ["onnx"], "{name}");
        assert_eq!(
            f.inputs()[1].default_value,
            Some(ConstValue::FsPath(model.to_owned())),
            "{name}"
        );
    }
}
