//! Registration tests for the astro library.

use scenarium::data::StaticValue;
use scenarium::node::function::FuncBehavior;

use super::*;

fn func<'a>(lib: &'a Library, name: &str) -> &'a Func {
    lib.funcs
        .iter()
        .find(|f| f.name == name)
        .unwrap_or_else(|| panic!("{name} registered"))
}

#[test]
fn astro_image_path_filter_matches_from_file_extensions() {
    let DataType::FsPath(cfg) = &*ASTRO_IMAGE_PATH_DATA_TYPE else {
        panic!("expected an FsPath data type");
    };
    assert_eq!(cfg.mode, FsPathMode::ExistingFile);
    assert_eq!(cfg.extensions, ASTRO_IMAGE_EXTENSIONS);
}

#[test]
fn astro_dir_is_an_existing_directory_picker() {
    let DataType::FsPath(cfg) = &*ASTRO_DIR_DATA_TYPE else {
        panic!("expected an FsPath data type");
    };
    assert_eq!(cfg.mode, FsPathMode::Directory);
    assert!(cfg.extensions.is_empty());
}

#[test]
fn load_astro_image_node_is_registered() {
    let lib = astro_library();
    let f = func(&lib, "Load Astro Image");
    assert_eq!(f.category, "Astro");
    assert_eq!(f.inputs.len(), 1);
    assert_eq!(f.outputs.len(), 1);
    assert_eq!(f.inputs[0].data_type, *ASTRO_IMAGE_PATH_DATA_TYPE);
    assert_eq!(f.outputs[0].ty.declared(), *IMAGE_DATA_TYPE);
}

#[test]
fn build_masters_node_is_registered() {
    let lib = astro_library();
    let f = func(&lib, "Build Masters");
    assert_eq!(f.category, "Astro");
    // Pure: the digest folds each calibration folder's contents (directory-aware
    // FsPath resolver), so a changed folder re-keys — no purity override needed.
    assert_eq!(f.behavior, FuncBehavior::Pure);
    assert_eq!(f.outputs.len(), 1);
    assert_eq!(f.outputs[0].ty.declared(), *MASTERS_DATA_TYPE);

    // Four optional calibration-frame folders, then sigma, then cache.
    assert_eq!(f.inputs.len(), 6);
    let dir_names: Vec<&str> = f.inputs[..4].iter().map(|i| i.name.as_str()).collect();
    assert_eq!(dir_names, ["Darks", "Flats", "Bias", "Flat Darks"]);
    for input in &f.inputs[..4] {
        assert!(!input.required, "calibration folders are optional");
        assert_eq!(input.data_type, *ASTRO_DIR_DATA_TYPE);
    }
    assert_eq!(f.inputs[4].name, "Sigma");
    assert_eq!(f.inputs[4].data_type, DataType::Float);
    assert_eq!(
        f.inputs[4].default_value,
        Some(StaticValue::Float(DEFAULT_SIGMA_THRESHOLD as f64)),
    );
    // Cache toggle defaults on, so masters persist + reload by default.
    assert_eq!(f.inputs[5].name, "Cache");
    assert_eq!(f.inputs[5].data_type, DataType::Bool);
    assert_eq!(f.inputs[5].default_value, Some(StaticValue::Bool(true)));
}

#[test]
fn stack_lights_node_is_registered() {
    let lib = astro_library();
    let f = func(&lib, "Stack Lights");
    assert_eq!(f.category, "Astro");
    // Pure: the digest folds the `lights` folder's contents (directory-aware
    // FsPath resolver), so any add/remove/edit re-keys it — no override needed.
    assert_eq!(f.behavior, FuncBehavior::Pure);

    // One input per stage: lights, masters, detection, registration,
    // combine, reference.
    assert_eq!(f.inputs.len(), 6);
    let names: Vec<&str> = f.inputs.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "Lights",
            "Masters",
            "Detection",
            "Registration",
            "Combine",
            "Reference"
        ]
    );
    assert_eq!(f.inputs[0].data_type, *ASTRO_DIR_DATA_TYPE);
    assert!(f.inputs[0].required, "lights folder is required");
    assert_eq!(f.inputs[1].data_type, *MASTERS_DATA_TYPE);
    assert!(!f.inputs[1].required, "masters are genuinely optional");
    // Each stage is one config-typed input (so a build_*_config wires in),
    // with the presets offered via value_variants + seeded to the first.
    // It's required: the seeded preset keeps a fresh node valid, but a
    // cleared input errors the run rather than silently defaulting.
    assert!(f.inputs[2].required, "detection is required");
    assert_eq!(
        f.inputs[2].data_type,
        config_data_type::<DetectionConfigDef>()
    );
    let detection_presets: Vec<&str> = f.inputs[2]
        .value_variants
        .iter()
        .map(|o| o.name.as_str())
        .collect();
    assert_eq!(
        detection_presets,
        [
            "wide_field",
            "high_resolution",
            "crowded_field",
            "precise_ground"
        ]
    );
    // The dropdown *displays* friendly labels while the stored value stays the
    // raw preset name (so saved graphs keep resolving) — display is decoupled.
    let detection_displays: Vec<&str> = f.inputs[2]
        .value_variants
        .iter()
        .map(|o| o.display_name.as_str())
        .collect();
    assert_eq!(
        detection_displays,
        [
            "Wide Field",
            "High Resolution",
            "Crowded Field",
            "Precise Ground"
        ]
    );
    assert_eq!(
        f.inputs[2].default_value,
        Some(StaticValue::Enum("wide_field".to_string())),
    );
    assert_eq!(
        f.inputs[3].data_type,
        config_data_type::<RegistrationConfigDef>()
    );
    assert_eq!(
        f.inputs[4].data_type,
        config_data_type::<CombineConfigDef>()
    );
    assert_eq!(f.inputs[5].name, "Reference");
    assert_eq!(f.inputs[5].default_value, Some(StaticValue::Int(-1)));

    let out_names: Vec<&str> = f.outputs.iter().map(|o| o.name.as_str()).collect();
    assert_eq!(out_names, ["Image", "Coverage", "Weight"]);
    for out in &f.outputs {
        assert_eq!(out.ty.declared(), *IMAGE_DATA_TYPE);
    }
}

#[test]
fn auto_stretch_node_is_registered() {
    let lib = astro_library();
    let f = func(&lib, "Auto Stretch");
    assert_eq!(f.category, "Astro");
    assert_eq!(f.inputs.len(), 2);
    assert_eq!(f.inputs[0].name, "Image");
    assert_eq!(f.inputs[0].data_type, *IMAGE_DATA_TYPE);
    assert!(f.inputs[0].required);
    // `method` is a config-typed input with the presets as value_variants
    // (seeded to the first), overridable by build_stretch_config.
    assert_eq!(f.inputs[1].name, "Method");
    assert_eq!(
        f.inputs[1].data_type,
        config_data_type::<StretchConfigDef>()
    );
    let methods: Vec<&str> = f.inputs[1]
        .value_variants
        .iter()
        .map(|o| o.name.as_str())
        .collect();
    assert_eq!(methods, ["auto_asinh", "auto_stf"]);
    assert_eq!(
        f.inputs[1].default_value,
        Some(StaticValue::Enum("auto_asinh".to_string())),
    );
    assert_eq!(f.outputs.len(), 1);
    assert_eq!(f.outputs[0].ty.declared(), *IMAGE_DATA_TYPE);
}

#[test]
fn processing_nodes_are_registered() {
    let lib = astro_library();
    // Each in-place op: a required `image` Image in, an Image out.
    for name in [
        "Extract Background",
        "Denoise",
        "SCNR",
        "Neutralize Background",
        "HDR Compress",
        "Local Contrast",
    ] {
        let f = func(&lib, name);
        assert_eq!(f.category, "Astro", "{name} category");
        assert_eq!(f.inputs[0].name, "Image", "{name} first input");
        assert_eq!(f.inputs[0].data_type, *IMAGE_DATA_TYPE, "{name} in type");
        assert!(f.inputs[0].required, "{name} image required");
        assert_eq!(f.outputs.len(), 1, "{name} one output");
        assert_eq!(
            f.outputs[0].ty.declared(),
            *IMAGE_DATA_TYPE,
            "{name} out type"
        );
    }
}

#[test]
fn scalar_per_frame_nodes_take_optional_config_overrides() {
    let lib = astro_library();
    // denoise / hdr_compress / local_contrast keep their inline scalar and
    // gain an optional `config` override fed by the matching build node.
    let cases: [(&str, &str, DataType); 3] = [
        (
            "Denoise",
            "Build Denoise Config",
            config_data_type::<DenoiseConfigDef>(),
        ),
        (
            "HDR Compress",
            "Build HDR Config",
            config_data_type::<HdrConfigDef>(),
        ),
        (
            "Local Contrast",
            "Build Local Contrast Config",
            config_data_type::<LocalContrastConfigDef>(),
        ),
    ];
    for (node, builder, ty) in cases {
        let f = func(&lib, node);
        let config = f.inputs.last().unwrap();
        assert_eq!(config.name, "Config", "{node} override input");
        assert_eq!(config.data_type, ty, "{node} override type");
        assert!(!config.required, "{node} config is an optional override");

        // The builder node emits that same config type.
        let b = func(&lib, builder);
        assert_eq!(b.category, "Astro");
        assert_eq!(b.outputs[0].ty.declared(), ty, "{builder} output type");
        assert!(
            b.inputs.iter().all(|i| i.required),
            "{builder} fields required"
        );
    }
}

#[test]
fn preset_nodes_use_value_variant_picks_with_build_overrides() {
    let lib = astro_library();
    // Every preset node is consistent: a config-typed input whose `value_variants`
    // are the preset names (seeded to the first), overridable by a build node.
    // (node, input name, input index, config type, build node, first preset)
    let cases: [(&str, &str, usize, DataType, &str, &str); 2] = [
        (
            "Auto Stretch",
            "Method",
            1,
            config_data_type::<StretchConfigDef>(),
            "Build Stretch Config",
            "auto_asinh",
        ),
        (
            "SCNR",
            "Method",
            1,
            config_data_type::<ScnrConfigDef>(),
            "Build SCNR Config",
            "average_neutral",
        ),
    ];
    for (node, input_name, idx, ty, builder, first_preset) in cases {
        let f = func(&lib, node);
        let input = &f.inputs[idx];
        assert_eq!(input.name, input_name, "{node} preset input name");
        assert_eq!(input.data_type, ty, "{node} preset input is config-typed");
        assert!(
            !input.value_variants.is_empty(),
            "{node} offers preset value_variants"
        );
        assert_eq!(
            input.value_variants[0].name, first_preset,
            "{node} first preset"
        );
        assert_eq!(
            input.default_value,
            Some(StaticValue::Enum(first_preset.to_string())),
            "{node} seeded to first preset"
        );
        // The matching build node exists and emits the same config type.
        let b = func(&lib, builder);
        assert_eq!(b.outputs[0].ty.declared(), ty, "{builder} output type");
    }
}

#[test]
fn build_background_config_reflects_fields_and_feeds_background_extract() {
    let lib = astro_library();
    // The builder exposes one labeled input per BackgroundConfig field, in
    // struct order; all required (none are `Option`s).
    let builder = func(&lib, "Build Background Config");
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
        config_data_type::<BackgroundConfigDef>()
    );

    // background_extract is image + one `config` input of that type: a mode
    // preset quick-pick (value_variants) a builder can wire into to override.
    let bg = func(&lib, "Extract Background");
    let bg_names: Vec<&str> = bg.inputs.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(bg_names, ["Image", "Config"]);
    assert!(bg.inputs[1].required, "config is required (preset-seeded)");
    assert_eq!(
        bg.inputs[1].data_type,
        config_data_type::<BackgroundConfigDef>()
    );
    let modes: Vec<&str> = bg.inputs[1]
        .value_variants
        .iter()
        .map(|o| o.name.as_str())
        .collect();
    assert_eq!(modes, ["subtract", "divide"]);
}

#[test]
fn ml_denoise_node_is_registered() {
    let lib = astro_library();
    let f = func(&lib, "ML Denoise");
    assert_eq!(f.category, "Astro");
    let names: Vec<&str> = f.inputs.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(names, ["Image"]);
    assert_eq!(f.inputs[0].data_type, *IMAGE_DATA_TYPE);
    assert_eq!(f.outputs.len(), 1);
    assert_eq!(f.outputs[0].name, "Image");
    assert_eq!(f.outputs[0].ty.declared(), *IMAGE_DATA_TYPE);
}

#[test]
fn remove_stars_node_has_starless_and_stars_outputs() {
    let lib = astro_library();
    let f = func(&lib, "Remove Stars");
    assert_eq!(f.category, "Astro");
    let names: Vec<&str> = f.inputs.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(names, ["Image"]);
    assert_eq!(f.inputs[0].data_type, *IMAGE_DATA_TYPE);
    let out_names: Vec<&str> = f.outputs.iter().map(|o| o.name.as_str()).collect();
    assert_eq!(out_names, ["Starless", "Stars"]);
    for o in &f.outputs {
        assert_eq!(o.ty.declared(), *IMAGE_DATA_TYPE);
    }
}

#[test]
fn ml_model_paths_default_and_round_trip() {
    use std::path::Path;
    assert_eq!(
        MlModelPaths::default().denoise,
        Path::new("DeepSNR_weights_v2.onnx")
    );
    assert_eq!(
        MlModelPaths::default().star_removal,
        Path::new("StarNet2_weights.onnx")
    );

    let original = ml_model_paths();
    set_ml_model_paths(MlModelPaths {
        denoise: "/models/d.onnx".into(),
        star_removal: "/models/s.onnx".into(),
    });
    let got = ml_model_paths();
    assert_eq!(got.denoise, Path::new("/models/d.onnx"));
    assert_eq!(got.star_removal, Path::new("/models/s.onnx"));
    set_ml_model_paths(original); // restore for any other test reading the global
}
