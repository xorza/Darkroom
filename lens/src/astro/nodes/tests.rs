//! Registration tests for the astro library.

use scenarium::Invocation;
use std::fs;
use std::path::PathBuf;

use imaginarium::Image as RawImage;
use lumos::{
    DEFAULT_SIGMA_THRESHOLD, Denoise, ExtractBackground, Hdr, LocalContrast,
    PREVIEW_IMAGE_EXTENSIONS, RAW_EXTENSIONS,
};
use scenarium::{
    AnyState, ConstValue, ContextManager, DataType, DynamicValue, FsPathMode, Func, FuncBehavior,
    Library, OutputDemand, SharedAnyState,
};

use crate::astro::config::processing::{ScnrKnobs, StretchKnobs};
use crate::astro::config::stacking::{CombineKnobs, DetectionKnobs, RegistrationKnobs};
use crate::astro::masters::MASTERS_DATA_TYPE;
use crate::astro::nodes::calibration::internals::frame_set_key;
use crate::astro::nodes::io::{ASTRO_IMAGE_PATH_DATA_TYPE, ASTRO_RAW_PATHS_DATA_TYPE};
use crate::astro::nodes::runtime::internals::image_to_planar;
use crate::astro::nodes::{MlModelPaths, astro_library, configure_ml_model_defaults};
use crate::config_node::config_data_type;
use crate::image::{IMAGE_DATA_TYPE, Image};

fn func<'a>(lib: &'a Library, name: &str) -> &'a Func {
    lib.funcs()
        .find(|f| f.name == name)
        .unwrap_or_else(|| panic!("{name} registered"))
}

/// An input already produced by another astro node is planar, so `image_to_planar` hands its
/// planes straight on — no repack between astro nodes, which is the whole point of the graph
/// carrying planar frames. A shared one still has to be copied, and pointer identity of the plane
/// allocation tells the two apart.
#[test]
fn a_planar_input_is_taken_without_repacking_and_a_shared_one_is_cloned() {
    let dimensions = lumos::ImageDimensions::new((4, 3), 1);

    let planar = lumos::LinearImage::from_planar_channels(dimensions, [vec![0.25f32; 12]]);
    let planes = planar.channel(0).pixels().as_ptr();
    let unique = DynamicValue::from_custom(Image::from(planar));
    let out = image_to_planar(unique);
    assert_eq!(
        out.channel(0).pixels().as_ptr(),
        planes,
        "unique planar input: the planes are moved, not repacked"
    );

    let planar = lumos::LinearImage::from_planar_channels(dimensions, [vec![0.25f32; 12]]);
    let planes = planar.channel(0).pixels().as_ptr();
    let shared = DynamicValue::from_custom(Image::from(planar));
    let second_holder = shared.clone();
    let out = image_to_planar(shared);
    assert_ne!(
        out.channel(0).pixels().as_ptr(),
        planes,
        "shared planar input: the planes are deep-cloned"
    );
    assert_eq!(out.dimensions(), dimensions);
    let original = second_holder.as_custom::<Image>().unwrap();
    assert_eq!(
        original.desc(),
        imaginarium::ImageDesc::new(4, 3, imaginarium::ColorFormat::L_F32),
        "the shared original stays intact behind the other holder"
    );
}

/// The other side of the boundary: an input from the `imaginarium` domain is interleaved, so it
/// does convert — once, here, rather than inside every op.
#[test]
fn an_interleaved_input_deinterleaves_at_the_domain_boundary() {
    // 2x1 RGB: pixels (0.125, 0.25, 0.375) and (0.5, 0.625, 0.75).
    let samples = [0.125f32, 0.25, 0.375, 0.5, 0.625, 0.75];
    let raw = RawImage::new_with_data(
        imaginarium::ImageDesc::new(2, 1, imaginarium::ColorFormat::RGB_F32),
        samples.iter().flat_map(|v| v.to_le_bytes()).collect(),
    )
    .unwrap();
    let out = image_to_planar(DynamicValue::from_custom(Image::from(raw)));
    assert_eq!(out.channel(0).pixels(), &[0.125, 0.5]);
    assert_eq!(out.channel(1).pixels(), &[0.25, 0.625]);
    assert_eq!(out.channel(2).pixels(), &[0.375, 0.75]);
}

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
fn master_source_key_changes_with_the_frame_set() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("test_output/lens/master_source_key");
    if dir.exists() {
        fs::remove_dir_all(&dir).unwrap();
    }
    fs::create_dir_all(&dir).unwrap();
    let first = dir.join("a.raf");
    let second = dir.join("b.raf");
    fs::write(&first, b"a").unwrap();
    let one_frame = frame_set_key(std::slice::from_ref(&first)).unwrap();
    assert_eq!(
        frame_set_key(std::slice::from_ref(&first)).unwrap(),
        one_frame
    );

    fs::write(&second, b"bb").unwrap();
    let two_frames = frame_set_key(&[first.clone(), second.clone()]).unwrap();
    assert_ne!(two_frames, one_frame);
    fs::write(&first, b"aaa").unwrap();
    let edited = frame_set_key(&[first.clone(), second]).unwrap();
    assert_ne!(edited, two_frames);
    fs::remove_file(&first).unwrap();
    assert_ne!(frame_set_key(&[]).unwrap(), edited);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn load_astro_image_node_is_registered() {
    let lib = astro_library(&MlModelPaths::default());
    let f = func(&lib, "Load Astro Image");
    assert_eq!(f.category, "Astro");
    assert_eq!(f.inputs.len(), 1);
    assert_eq!(f.outputs.len(), 1);
    assert_eq!(f.inputs[0].data_type, *ASTRO_IMAGE_PATH_DATA_TYPE);
    assert_eq!(f.outputs[0].ty.declared(), *IMAGE_DATA_TYPE);
}

#[test]
fn build_masters_node_is_registered() {
    let lib = astro_library(&MlModelPaths::default());
    let f = func(&lib, "Build Masters");
    assert_eq!(f.category, "Astro");
    // Pure: the digest folds each selected calibration file's identity.
    assert_eq!(f.behavior, FuncBehavior::Pure);
    assert_eq!(f.outputs.len(), 1);
    assert_eq!(f.outputs[0].ty.declared(), *MASTERS_DATA_TYPE);

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
        Some(ConstValue::Float(DEFAULT_SIGMA_THRESHOLD as f64)),
    );
    assert_eq!(f.inputs[5].name, "Cache");
    assert_eq!(f.inputs[5].data_type, DataType::Bool);
    assert_eq!(f.inputs[5].default_value, Some(ConstValue::Bool(true)));
}

#[test]
fn stack_lights_node_is_registered() {
    let lib = astro_library(&MlModelPaths::default());
    let f = func(&lib, "Stack Lights");
    assert_eq!(f.category, "Astro");
    // Pure: the digest folds exactly the selected light files.
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
    assert_eq!(f.inputs[0].data_type, *ASTRO_RAW_PATHS_DATA_TYPE);
    assert!(f.inputs[0].required, "light frames are required");
    assert_eq!(f.inputs[1].data_type, *MASTERS_DATA_TYPE);
    assert!(!f.inputs[1].required, "masters are genuinely optional");
    // Each stage is one config-typed input (so a build_*_config wires in),
    // with the presets offered via value_variants + seeded to the first.
    // It's required: the seeded preset keeps a fresh node valid, but a
    // cleared input errors the run rather than silently defaulting.
    assert!(f.inputs[2].required, "detection is required");
    assert_eq!(f.inputs[2].data_type, config_data_type::<DetectionKnobs>());
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
        Some(ConstValue::Enum("wide_field".to_string())),
    );
    assert_eq!(
        f.inputs[3].data_type,
        config_data_type::<RegistrationKnobs>()
    );
    assert_eq!(f.inputs[4].data_type, config_data_type::<CombineKnobs>());
    assert_eq!(f.inputs[5].name, "Reference");
    assert_eq!(f.inputs[5].default_value, Some(ConstValue::Int(-1)));

    let out_names: Vec<&str> = f.outputs.iter().map(|o| o.name.as_str()).collect();
    assert_eq!(out_names, ["Image", "Coverage", "Weight"]);
    for out in &f.outputs {
        assert_eq!(out.ty.declared(), *IMAGE_DATA_TYPE);
    }
}

#[test]
fn auto_stretch_node_is_registered() {
    let lib = astro_library(&MlModelPaths::default());
    let f = func(&lib, "Auto Stretch");
    assert_eq!(f.category, "Astro");
    assert_eq!(f.inputs.len(), 2);
    assert_eq!(f.inputs[0].name, "Image");
    assert_eq!(f.inputs[0].data_type, *IMAGE_DATA_TYPE);
    assert!(f.inputs[0].required);
    // `method` is a config-typed input with the presets as value_variants
    // (seeded to the first), overridable by build_stretch_config.
    assert_eq!(f.inputs[1].name, "Method");
    assert_eq!(f.inputs[1].data_type, config_data_type::<StretchKnobs>());
    let methods: Vec<&str> = f.inputs[1]
        .value_variants
        .iter()
        .map(|o| o.name.as_str())
        .collect();
    assert_eq!(methods, ["auto_asinh", "auto_stf"]);
    assert_eq!(
        f.inputs[1].default_value,
        Some(ConstValue::Enum("auto_asinh".to_string())),
    );
    assert_eq!(f.outputs.len(), 1);
    assert_eq!(f.outputs[0].ty.declared(), *IMAGE_DATA_TYPE);
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
    let lib = astro_library(&MlModelPaths::default());
    // denoise / hdr_compress / local_contrast keep their inline scalar and
    // gain an optional `config` override fed by the matching build node.
    let cases: [(&str, &str, DataType); 3] = [
        (
            "Denoise",
            "Build Denoise Config",
            config_data_type::<Denoise>(),
        ),
        (
            "HDR Compression",
            "Build HDR Config",
            config_data_type::<Hdr>(),
        ),
        (
            "Local Contrast",
            "Build Local Contrast Config",
            config_data_type::<LocalContrast>(),
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
    let lib = astro_library(&MlModelPaths::default());
    // Every preset node is consistent: a config-typed input whose `value_variants`
    // are the preset names (seeded to the first), overridable by a build node.
    // (node, input name, input index, config type, build node, first preset)
    let cases: [(&str, &str, usize, DataType, &str, &str); 2] = [
        (
            "Auto Stretch",
            "Method",
            1,
            config_data_type::<StretchKnobs>(),
            "Build Stretch Config",
            "auto_asinh",
        ),
        (
            "SCNR",
            "Method",
            1,
            config_data_type::<ScnrKnobs>(),
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
            Some(ConstValue::Enum(first_preset.to_string())),
            "{node} seeded to first preset"
        );
        // The matching build node exists and emits the same config type.
        let b = func(&lib, builder);
        assert_eq!(b.outputs[0].ty.declared(), ty, "{builder} output type");
    }
}

#[tokio::test]
async fn build_background_config_reflects_fields_and_rejects_invalid_values() {
    let lib = astro_library(&MlModelPaths::default());
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
        config_data_type::<ExtractBackground>()
    );

    // background_extract is image + one `config` input of that type: a mode
    // preset quick-pick (value_variants) a builder can wire into to override.
    let bg = func(&lib, "Extract Background");
    let bg_names: Vec<&str> = bg.inputs.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(bg_names, ["Image", "Config"]);
    assert!(bg.inputs[1].required, "config is required (preset-seeded)");
    assert_eq!(
        bg.inputs[1].data_type,
        config_data_type::<ExtractBackground>()
    );
    let modes: Vec<&str> = bg.inputs[1]
        .value_variants
        .iter()
        .map(|o| o.name.as_str())
        .collect();
    assert_eq!(modes, ["subtract", "divide"]);

    let mut inputs: Vec<DynamicValue> = builder
        .inputs
        .iter()
        .map(|input| input.default_value.clone().unwrap().into())
        .collect();
    inputs[0] = ConstValue::Int(-1).into();
    let mut outputs = vec![DynamicValue::Unbound; builder.outputs.len()];
    let error = builder
        .lambda
        .invoke(Invocation {
            ctx: &mut ContextManager::default(),
            state: &mut AnyState::default(),
            event_state: &SharedAnyState::default(),
            inputs: &mut inputs,
            demand: &[OutputDemand::Produce],
            outputs: &mut outputs,
        })
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "field `tile_size` value -1 cannot be represented as usize"
    );
    assert!(matches!(outputs[0], DynamicValue::Unbound));
}

#[test]
fn ml_denoise_node_is_registered() {
    let lib = astro_library(&MlModelPaths::default());
    let f = func(&lib, "ML Denoise");
    assert_eq!(f.category, "Astro");
    let names: Vec<&str> = f.inputs.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(names, ["Image", "Model"]);
    assert_eq!(f.inputs[0].data_type, *IMAGE_DATA_TYPE);
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
    assert_eq!(f.outputs[0].ty.declared(), *IMAGE_DATA_TYPE);
}

#[test]
fn remove_stars_node_has_starless_and_stars_outputs() {
    let lib = astro_library(&MlModelPaths::default());
    let f = func(&lib, "ML Star Removal");
    assert_eq!(f.category, "Astro");
    let names: Vec<&str> = f.inputs.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(names, ["Image", "Model"]);
    assert_eq!(f.inputs[0].data_type, *IMAGE_DATA_TYPE);
    assert_eq!(
        f.inputs[1].default_value,
        Some(ConstValue::FsPath("StarNet2_weights.onnx".to_string()))
    );
    let out_names: Vec<&str> = f.outputs.iter().map(|o| o.name.as_str()).collect();
    assert_eq!(out_names, ["Starless", "Stars"]);
    for o in &f.outputs {
        assert_eq!(o.ty.declared(), *IMAGE_DATA_TYPE);
    }
}

#[test]
fn configured_model_defaults_replace_both_node_definitions() {
    let mut library = astro_library(&MlModelPaths::default());
    let paths = MlModelPaths {
        denoise: PathBuf::from("/models/denoise.onnx"),
        star_removal: PathBuf::from("/models/stars.onnx"),
    };
    let function_count = library.funcs().len();
    configure_ml_model_defaults(&mut library, &paths);
    assert_eq!(library.funcs().len(), function_count);
    assert_eq!(
        func(&library, "ML Denoise").inputs[1].default_value,
        Some(ConstValue::FsPath(paths.denoise.display().to_string()))
    );
    assert_eq!(
        func(&library, "ML Star Removal").inputs[1].default_value,
        Some(ConstValue::FsPath(paths.star_removal.display().to_string()))
    );
}
