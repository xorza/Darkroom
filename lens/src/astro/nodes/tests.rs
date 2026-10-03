//! Registration tests for the astro library.

use scenarium::testing::func_invoker::FuncInvoker;

use lumos::{
    DEFAULT_SIGMA_THRESHOLD, Denoise, ExtractBackground, Hdr, LocalContrast,
    PREVIEW_IMAGE_EXTENSIONS, RAW_EXTENSIONS,
};
use scenarium::{ConstValue, DataType, DynamicValue, FsPathMode, FuncBehavior};

use crate::astro::masters::MASTERS_DATA_TYPE;
use crate::astro::nodes::io::{ASTRO_IMAGE_PATH_DATA_TYPE, ASTRO_RAW_PATHS_DATA_TYPE};
use crate::astro::nodes::{MlModelPaths, astro_library};
use crate::config_node::config_data_type;
use crate::image::IMAGE_DATA_TYPE;

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

#[test]
fn load_astro_image_node_is_registered() {
    let lib = astro_library(&MlModelPaths::default());
    let f = lib.by_name("Load Astro Image").unwrap();
    assert_eq!(f.category, "Astro");
    assert_eq!(f.inputs.len(), 1);
    assert_eq!(f.outputs.len(), 1);
    assert_eq!(f.inputs[0].data_type, *ASTRO_IMAGE_PATH_DATA_TYPE);
    assert_eq!(f.outputs[0].ty.declared(), IMAGE_DATA_TYPE);
}

#[test]
fn build_masters_node_is_registered() {
    let lib = astro_library(&MlModelPaths::default());
    let f = lib.by_name("Build Masters").unwrap();
    assert_eq!(f.category, "Astro");
    // Pure: the digest folds each selected calibration file's identity.
    assert_eq!(f.behavior, FuncBehavior::Pure);
    assert_eq!(f.outputs.len(), 1);
    assert_eq!(f.outputs[0].ty.declared(), MASTERS_DATA_TYPE);

    // Four optional calibration-frame sets, then sigma and cache.
    assert_eq!(f.inputs.len(), 6);
    let frame_names: Vec<&str> = f.inputs[..4].iter().map(|i| i.name.as_str()).collect();
    assert_eq!(frame_names, ["Darks", "Flats", "Bias", "Flat Darks"]);
    for input in &f.inputs[..4] {
        assert!(!input.required, "calibration frame sets are optional");
        assert_eq!(input.data_type, *ASTRO_RAW_PATHS_DATA_TYPE);
    }
    assert_eq!(f.inputs[4].name, "Sigma");
    assert_eq!(f.inputs[4].data_type, DataType::Float);
    assert_eq!(
        f.inputs[4].default_value,
        Some(ConstValue::Float(f64::from(DEFAULT_SIGMA_THRESHOLD))),
    );
    assert_eq!(f.inputs[5].name, "Cache");
    assert_eq!(f.inputs[5].data_type, DataType::Bool);
    assert_eq!(f.inputs[5].default_value, Some(ConstValue::Bool(true)));
}

/// Every quick knob is one const-only input beside a detailed config declared
/// to override it, and the matching builder emits that config: the D12 shape,
/// on the six processing nodes and on the stacking node's three stages.
#[test]
fn every_quick_knob_has_a_config_that_overrides_it() {
    let lib = astro_library(&MlModelPaths::default());
    // (node, knob index, knob name, config index, config name, builder)
    let cases = [
        (
            "Auto Stretch",
            1,
            "Method",
            2,
            "Config",
            "Build Stretch Config",
        ),
        (
            "Extract Background",
            1,
            "Mode",
            2,
            "Config",
            "Build Background Config",
        ),
        ("SCNR", 1, "Method", 2, "Config", "Build SCNR Config"),
        (
            "Denoise",
            1,
            "Strength",
            2,
            "Config",
            "Build Denoise Config",
        ),
        (
            "HDR Compression",
            1,
            "Amount",
            2,
            "Config",
            "Build HDR Config",
        ),
        (
            "Local Contrast",
            1,
            "Strength",
            2,
            "Config",
            "Build Local Contrast Config",
        ),
        (
            "Stack Lights",
            2,
            "Detection",
            3,
            "Detection Config",
            "Build Detection Config",
        ),
        (
            "Stack Lights",
            4,
            "Registration",
            5,
            "Registration Config",
            "Build Registration Config",
        ),
        (
            "Stack Lights",
            6,
            "Combine",
            7,
            "Combine Config",
            "Build Combine Config",
        ),
    ];
    for (node, knob_idx, knob_name, config_idx, config_name, builder) in cases {
        let f = lib.by_name(node).unwrap();
        let knob = &f.inputs[knob_idx];
        assert_eq!(knob.name, knob_name, "{node}");
        assert!(knob.const_only && knob.required, "{node} {knob_name}");
        assert!(
            knob.default_value.is_some(),
            "{node} {knob_name} starts set"
        );
        let config = &f.inputs[config_idx];
        assert_eq!(config.name, config_name, "{node}");
        assert!(!config.required, "{node} {config_name} is optional");
        assert_eq!(config.overrides, Some(knob_idx), "{node} {config_name}");
        let built = lib.by_name(builder).unwrap();
        assert_eq!(
            built.outputs[0].ty.declared(),
            config.data_type,
            "{builder}"
        );
    }

    // A strength knob starts where the op's own default does.
    let strength = |node: &str| lib.by_name(node).unwrap().inputs[1].default_value.clone();
    assert_eq!(
        strength("Denoise"),
        Some(ConstValue::Float(f64::from(Denoise::default().strength)))
    );
    assert_eq!(
        strength("HDR Compression"),
        Some(ConstValue::Float(f64::from(Hdr::default().amount)))
    );
    assert_eq!(
        strength("Local Contrast"),
        Some(ConstValue::Float(f64::from(
            LocalContrast::default().strength
        )))
    );
}

#[test]
fn stack_lights_node_is_registered() {
    let lib = astro_library(&MlModelPaths::default());
    let f = lib.by_name("Stack Lights").unwrap();
    assert_eq!(f.category, "Astro");
    // Pure: the digest folds exactly the selected light files.
    assert_eq!(f.behavior, FuncBehavior::Pure);
    let names: Vec<&str> = f.inputs.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "Lights",
            "Masters",
            "Detection",
            "Detection Config",
            "Registration",
            "Registration Config",
            "Combine",
            "Combine Config",
            "Reference"
        ]
    );
    assert_eq!(f.inputs[0].data_type, *ASTRO_RAW_PATHS_DATA_TYPE);
    assert!(f.inputs[0].required, "light frames are required");
    assert_eq!(f.inputs[1].data_type, MASTERS_DATA_TYPE);
    assert!(!f.inputs[1].required, "masters are genuinely optional");
    // Unset picks the richest frame; no sentinel stands in for it.
    assert!(!f.inputs[8].required);
    assert_eq!(f.inputs[8].default_value, None);

    let out_names: Vec<&str> = f.outputs.iter().map(|o| o.name.as_str()).collect();
    assert_eq!(out_names, ["Image", "Coverage", "Weight"]);
    for out in &f.outputs {
        assert_eq!(out.ty.declared(), IMAGE_DATA_TYPE);
    }
}

#[test]
fn processing_nodes_are_registered() {
    let lib = astro_library(&MlModelPaths::default());
    // Each in-place op: a required `image` Image in, an Image out.
    for name in [
        "Extract Background",
        "Denoise",
        "SCNR",
        "Neutralize Background",
        "HDR Compression",
        "Local Contrast",
    ] {
        let f = lib.by_name(name).unwrap();
        assert_eq!(f.category, "Astro", "{name} category");
        assert_eq!(f.inputs[0].name, "Image", "{name} first input");
        assert_eq!(f.inputs[0].data_type, IMAGE_DATA_TYPE, "{name} in type");
        assert!(f.inputs[0].required, "{name} image required");
        assert_eq!(f.outputs.len(), 1, "{name} one output");
        assert_eq!(
            f.outputs[0].ty.declared(),
            IMAGE_DATA_TYPE,
            "{name} out type"
        );
    }
}

#[tokio::test]
async fn build_background_config_reflects_fields_and_rejects_invalid_values() {
    let lib = astro_library(&MlModelPaths::default());
    // The builder exposes one labeled input per BackgroundConfig field, in
    // struct order; all required (none are `Option`s).
    let builder = lib.by_name("Build Background Config").unwrap();
    assert_eq!(builder.category, "Astro");
    let labels: Vec<&str> = builder.inputs.iter().map(|i| i.name.as_str()).collect();
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
    assert!(builder.inputs.iter().all(|i| i.required));
    assert_eq!(builder.outputs[0].name, "Config");
    assert_eq!(
        builder.outputs[0].ty.declared(),
        config_data_type::<ExtractBackground>()
    );

    let mut inputs: Vec<DynamicValue> = builder
        .inputs
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

#[test]
fn ml_denoise_node_is_registered() {
    let lib = astro_library(&MlModelPaths::default());
    let f = lib.by_name("ML Denoise").unwrap();
    assert_eq!(f.category, "Astro");
    let names: Vec<&str> = f.inputs.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(names, ["Image", "Model"]);
    assert_eq!(f.inputs[0].data_type, IMAGE_DATA_TYPE);
    let DataType::FsPath(model) = &f.inputs[1].data_type else {
        panic!("model is a file path");
    };
    assert_eq!(model.mode, FsPathMode::ExistingFile);
    assert_eq!(model.extensions, ["onnx"]);
    assert_eq!(
        f.inputs[1].default_value,
        Some(ConstValue::FsPath("DeepSNR_weights_v2.onnx".to_string()))
    );
    assert_eq!(f.outputs.len(), 1);
    assert_eq!(f.outputs[0].name, "Image");
    assert_eq!(f.outputs[0].ty.declared(), IMAGE_DATA_TYPE);
}

#[test]
fn remove_stars_node_has_starless_and_stars_outputs() {
    let lib = astro_library(&MlModelPaths::default());
    let f = lib.by_name("ML Star Removal").unwrap();
    assert_eq!(f.category, "Astro");
    let names: Vec<&str> = f.inputs.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(names, ["Image", "Model"]);
    assert_eq!(f.inputs[0].data_type, IMAGE_DATA_TYPE);
    assert_eq!(
        f.inputs[1].default_value,
        Some(ConstValue::FsPath("StarNet2_weights.onnx".to_string()))
    );
    let out_names: Vec<&str> = f.outputs.iter().map(|o| o.name.as_str()).collect();
    assert_eq!(out_names, ["Starless", "Stars"]);
    for o in &f.outputs {
        assert_eq!(o.ty.declared(), IMAGE_DATA_TYPE);
    }
}
