//! `astro_funclib()` — the `lumos`-backed node library (category `astro`):
//! `load_astro_image` (decode), `build_masters` (calibration masters),
//! `stack_lights` (calibrate + align + stack), and per-frame processing
//! nodes like `auto_stretch`. Heavy work runs off the worker via
//! `spawn_blocking`; preset dropdowns live in [`crate::astro_presets`].

use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{Arc, LazyLock};

use common::Buffer2;
use common::file_utils::astro_image_files;
use imaginarium::Image as RawImage;
use lumos::{
    AlignStackConfig, AstroImage, BackgroundConfig, CalibrationImages, CalibrationMasters,
    CfaImage, DEFAULT_SIGMA_THRESHOLD, DenoiseConfig, HdrConfig, ImageDimensions,
    LocalContrastConfig, Reference, StackConfig, StarDetector, calibrate_align_stack,
    compress_dynamic_range, denoise, enhance_local_contrast, extract_background,
    neutralize_background, scnr, stack_cfa_master, stretch,
};
use scenarium::data::{
    DataType, DynamicValue, EnumVariants, FsPathConfig, FsPathMode, StaticValue,
};
use scenarium::func_lambda::FuncLambda;
use scenarium::function::{Func, FuncInput, FuncLib, ValueOption};

use crate::astro_configs::{
    BackgroundConfigDef, CombineConfigDef, DenoiseConfigDef, DetectionConfigDef, HdrConfigDef,
    LocalContrastConfigDef, RegistrationConfigDef,
};
use crate::astro_frame::{ASTRO_FRAME_DATA_TYPE, AstroFrame};
use crate::astro_presets::{
    BackgroundModeKind, CombinePreset, DETECTION_PRESET_DATATYPE, DetectionPreset,
    RegistrationPreset, SCNR_METHOD_DATATYPE, STRETCH_PRESET_DATATYPE, ScnrKind, StretchPreset,
};
use crate::config_node::{ConfigValue, NodeConfig, config_builder_func, config_data_type};
use crate::image::{IMAGE_DATA_TYPE, Image};
use crate::masters::{MASTERS_DATA_TYPE, Masters};

/// Every file extension `AstroImage::from_file` recognizes: FITS, camera
/// RAW, and standard images. Kept in lockstep with that dispatch (note
/// `fts` is *not* accepted, so it's absent here).
const ASTRO_IMAGE_EXTENSIONS: [&str; 13] = [
    "fits", "fit", // FITS
    "raf", "cr2", "cr3", "nef", "arw", "dng", // camera RAW
    "tiff", "tif", "png", "jpg", "jpeg", // standard
];

/// Reusable data type for an astro image file-path input: an existing-file
/// picker filtered to [`ASTRO_IMAGE_EXTENSIONS`]. Shared so every astro
/// loader presents the same filter.
pub static ASTRO_IMAGE_PATH_DATA_TYPE: LazyLock<DataType> = LazyLock::new(|| {
    DataType::FsPath(Arc::new(FsPathConfig::with_extensions(
        FsPathMode::ExistingFile,
        ASTRO_IMAGE_EXTENSIONS
            .iter()
            .map(|s| s.to_string())
            .collect(),
    )))
});

/// Reusable data type for a frame-folder input: an existing-directory
/// picker (no extension filter). Shared by the masters / stacking nodes,
/// which glob the directory for astro frames via
/// [`common::file_utils::astro_image_files`].
pub static ASTRO_DIR_DATA_TYPE: LazyLock<DataType> =
    LazyLock::new(|| DataType::FsPath(Arc::new(FsPathConfig::new(FsPathMode::Directory))));

/// The lumos-backed astro nodes (category `astro`).
pub fn astro_funclib() -> FuncLib {
    let mut func_lib = FuncLib::default();

    // load_astro_image
    func_lib.add(
        Func::new("fbcc8899-efc3-40e0-a6fd-8743f86edbd3", "load_astro_image")
            .description("Loads a FITS/RAW/standard astronomical image")
            .category("astro")
            .run_once()
            .input(FuncInput::required(
                "path",
                ASTRO_IMAGE_PATH_DATA_TYPE.clone(),
            ))
            .output("image", ASTRO_FRAME_DATA_TYPE.clone())
            .lambda(FuncLambda::new(move |_, _, _, inputs, _, outputs| {
                Box::pin(async move {
                    assert_eq!(inputs.len(), 1);
                    assert_eq!(outputs.len(), 1);

                    let path = inputs[0].value.as_fs_path().unwrap().to_owned();
                    // Decoding (FITS parse / libraw / demosaic) is heavy
                    // synchronous CPU work — keep it off the worker thread.
                    let image = tokio::task::spawn_blocking(move || AstroImage::from_file(&path))
                        .await
                        .map_err(anyhow::Error::from)?
                        .map_err(anyhow::Error::from)?;

                    outputs[0] = DynamicValue::from_custom(AstroFrame::from(image));

                    Ok(())
                })
            })),
    );

    // build_masters
    func_lib.add(
        Func::new("f2f6f1ff-5b10-409c-900f-d6b48750a529", "build_masters")
            .description(
                "Stacks raw calibration frames (darks/flats/bias/flat-darks) into calibration \
                 masters. With `cache` on, each master is written next to its frames and reused \
                 next run instead of re-stacking.",
            )
            .category("astro")
            .run_once()
            .inputs([
                dir_input("darks"),
                dir_input("flats"),
                dir_input("bias"),
                dir_input("flat_darks"),
            ])
            .input(
                FuncInput::required("sigma", DataType::Float)
                    .default(DEFAULT_SIGMA_THRESHOLD as f64),
            )
            .input(FuncInput::required("cache", DataType::Bool).default(true))
            .output("masters", MASTERS_DATA_TYPE.clone())
            .lambda(FuncLambda::new(move |_, _, _, inputs, _, outputs| {
                Box::pin(async move {
                    assert_eq!(inputs.len(), 6);
                    assert_eq!(outputs.len(), 1);

                    // Each optional folder maps to its directory (unbound → no
                    // master for that role); frames are globbed only on a miss.
                    let dir = |idx: usize| inputs[idx].value.as_fs_path().map(PathBuf::from);
                    let dirs = [dir(0), dir(1), dir(2), dir(3)];
                    let sigma = inputs[4]
                        .value
                        .as_f64()
                        .map(|v| v as f32)
                        .expect("sigma is required");
                    let cache = inputs[5].value.as_bool().expect("cache is required");

                    // Stacking many full-resolution CFA frames is heavy CPU work;
                    // a cached master is loaded instead when present.
                    let masters = tokio::task::spawn_blocking(move || {
                        build_masters_cached(dirs, sigma, cache)
                    })
                    .await
                    .map_err(anyhow::Error::from)??;

                    outputs[0] = DynamicValue::from_custom(Masters::from(masters));

                    Ok(())
                })
            })),
    );

    // stack_lights
    func_lib.add(
        Func::new("b02f5c42-7bda-48f6-81dd-81338efbb126", "stack_lights")
            .description("Calibrates, aligns and stacks a folder of light frames into one image")
            .category("astro")
            .run_once()
            .input(FuncInput::required("lights", ASTRO_DIR_DATA_TYPE.clone()))
            .input(FuncInput::optional("masters", MASTERS_DATA_TYPE.clone()))
            // Each stage is one input: a preset quick-pick (the `value_options`
            // dropdown) that a build_*_config node can wire into to override.
            .input(preset_config_input::<DetectionConfigDef>(
                "detection",
                DetectionPreset::variant_names(),
            ))
            .input(preset_config_input::<RegistrationConfigDef>(
                "registration",
                RegistrationPreset::variant_names(),
            ))
            .input(preset_config_input::<CombineConfigDef>(
                "combine",
                CombinePreset::variant_names(),
            ))
            // reference: < 0 picks the frame with the most stars (auto); >= 0 is
            // a 0-based index into the (directory-sorted) light frames.
            .input(FuncInput::required("reference", DataType::Int).default(-1_i64))
            .output("image", ASTRO_FRAME_DATA_TYPE.clone())
            .output("coverage", ASTRO_FRAME_DATA_TYPE.clone())
            .output("weight", ASTRO_FRAME_DATA_TYPE.clone())
            .lambda(FuncLambda::new(move |_, _, _, inputs, _, outputs| {
                Box::pin(async move {
                    assert_eq!(inputs.len(), 6);
                    assert_eq!(outputs.len(), 3);

                    let lights = inputs[0]
                        .value
                        .as_fs_path()
                        .map(|dir| astro_image_files(Path::new(dir)))
                        .unwrap_or_default();
                    // Arc-clone the masters value so it can move into the
                    // blocking task; `Unbound` means "no calibration".
                    let masters_val = inputs[1].value.clone();
                    // Each stage input: a wired config overrides the picked preset.
                    let detection = inputs[2]
                        .value
                        .as_custom::<ConfigValue<DetectionConfigDef>>()
                        .map(|c| c.0.clone().into())
                        .or_else(|| {
                            inputs[2]
                                .value
                                .as_enum()
                                .and_then(|s| DetectionPreset::from_str(s).ok())
                                .map(|preset| preset.config())
                        })
                        .expect("detection is required");
                    let registration = inputs[3]
                        .value
                        .as_custom::<ConfigValue<RegistrationConfigDef>>()
                        .map(|c| c.0.clone().into())
                        .or_else(|| {
                            inputs[3]
                                .value
                                .as_enum()
                                .and_then(|s| RegistrationPreset::from_str(s).ok())
                                .map(|preset| preset.config())
                        })
                        .expect("registration is required");
                    let stack = inputs[4]
                        .value
                        .as_custom::<ConfigValue<CombineConfigDef>>()
                        .map(|c| c.0.clone().into())
                        .or_else(|| {
                            inputs[4]
                                .value
                                .as_enum()
                                .and_then(|s| CombinePreset::from_str(s).ok())
                                .map(|preset| preset.config())
                        })
                        .expect("combine is required");
                    // `reference` is required + seeded to -1 (auto); >= 0 selects a frame.
                    let reference = match inputs[5].value.as_i64().expect("reference is required") {
                        index if index >= 0 => Reference::Index(index as usize),
                        _ => Reference::Auto,
                    };
                    let config = AlignStackConfig {
                        detection,
                        registration,
                        stack,
                        reference,
                        cosmic_ray: None,
                    };

                    // Load → calibrate → demosaic → detect → register → combine:
                    // the whole pipeline is heavy synchronous CPU work.
                    let result = tokio::task::spawn_blocking(move || {
                        let empty = CalibrationMasters {
                            master_dark: None,
                            master_flat: None,
                            master_bias: None,
                            master_flat_dark: None,
                            defect_map: None,
                        };
                        let masters = masters_val
                            .as_custom::<Masters>()
                            .map(|m| &m.masters)
                            .unwrap_or(&empty);
                        calibrate_align_stack(&lights, masters, &config)
                    })
                    .await
                    .map_err(anyhow::Error::from)?
                    .map_err(anyhow::Error::from)?;

                    outputs[0] = DynamicValue::from_custom(AstroFrame::from(result.image));
                    outputs[1] = DynamicValue::from_custom(AstroFrame::from(plane_to_frame(
                        result.coverage,
                    )));
                    outputs[2] =
                        DynamicValue::from_custom(AstroFrame::from(plane_to_frame(result.weight)));

                    Ok(())
                })
            })),
    );

    // auto_stretch
    func_lib.add(
        Func::new("c15248e0-006a-4a4a-9aae-b1fc7886dea1", "auto_stretch")
            .description("Auto-stretches a linear frame to a viewable image (display tone curve)")
            .category("astro")
            .pure()
            .input(frame_input("image"))
            .input(preset_input("method", &STRETCH_PRESET_DATATYPE))
            .output("image", ASTRO_FRAME_DATA_TYPE.clone())
            .lambda(FuncLambda::new(move |_, _, _, inputs, _, outputs| {
                Box::pin(async move {
                    assert_eq!(inputs.len(), 2);
                    assert_eq!(outputs.len(), 1);

                    let config = inputs[1]
                        .value
                        .as_enum()
                        .and_then(|s| StretchPreset::from_str(s).ok())
                        .expect("method is required")
                        .config();
                    // Arc-clone the frame so the deep copy + stretch run
                    // off the worker thread.
                    let value = inputs[0].value.clone();
                    let stretched = tokio::task::spawn_blocking(move || {
                        let frame = value
                            .as_custom::<AstroFrame>()
                            .expect("image input is an AstroFrame");
                        let mut image = frame.image.clone();
                        stretch(&mut image, config);
                        image
                    })
                    .await
                    .map_err(anyhow::Error::from)?;

                    outputs[0] = DynamicValue::from_custom(AstroFrame::from(stretched));

                    Ok(())
                })
            })),
    );

    // astro_to_image: bridge an `AstroFrame` into a `lens::Image` so the
    // imaginarium image nodes (brightness/contrast, blend, convert, save…)
    // can consume astro output.
    func_lib.add(
        Func::new("7a0265e1-9631-45bd-8ecd-1e923b67a58c", "astro_to_image")
            .description("Converts an astro frame to an image (for the imaginarium image nodes)")
            .category("astro")
            .pure()
            .input(FuncInput::required("frame", ASTRO_FRAME_DATA_TYPE.clone()))
            .output("image", IMAGE_DATA_TYPE.clone())
            .lambda(FuncLambda::new(move |_, _, _, inputs, _, outputs| {
                Box::pin(async move {
                    assert_eq!(inputs.len(), 1);
                    assert_eq!(outputs.len(), 1);

                    // The planar→interleaved conversion is a full-frame copy;
                    // run it off the worker thread.
                    let value = inputs[0].value.clone();
                    let raw = tokio::task::spawn_blocking(move || {
                        let frame = value
                            .as_custom::<AstroFrame>()
                            .expect("frame input is an AstroFrame");
                        RawImage::from(&frame.image)
                    })
                    .await
                    .map_err(anyhow::Error::from)?;

                    outputs[0] = DynamicValue::from_custom(Image::from(raw));

                    Ok(())
                })
            })),
    );

    // --- config-builder nodes ---

    // build_background_config: expose every BackgroundConfig field as an input,
    // output a detailed config to wire into background_extract.
    func_lib.add(config_builder_func::<BackgroundConfigDef>(
        "9cda0462-1b8e-4c50-83d6-4db470df22d9",
        "build_background_config",
        "Builds a detailed background-extraction config",
    ));

    // build_detection_config / build_registration_config / build_combine_config:
    // detailed overrides for stack_lights' detection / registration / combine
    // preset dropdowns.
    func_lib.add(config_builder_func::<DetectionConfigDef>(
        "6c6f92e7-0f74-454c-acc4-68691cb8462f",
        "build_detection_config",
        "Builds a detailed star-detection config",
    ));
    func_lib.add(config_builder_func::<RegistrationConfigDef>(
        "adf216fe-baa9-4abd-8c4a-bfb98bb60fbc",
        "build_registration_config",
        "Builds a detailed registration config",
    ));
    func_lib.add(config_builder_func::<CombineConfigDef>(
        "05313ceb-a3b2-4488-92af-c9e228bb1789",
        "build_combine_config",
        "Builds a detailed frame-combination config",
    ));

    // build_denoise_config / build_hdr_config / build_local_contrast_config:
    // full configs for the per-frame nodes whose inline param is one scalar.
    func_lib.add(config_builder_func::<DenoiseConfigDef>(
        "77693298-3531-4858-89ce-03cb347dc3f2",
        "build_denoise_config",
        "Builds a detailed wavelet-denoise config",
    ));
    func_lib.add(config_builder_func::<HdrConfigDef>(
        "dc82d7a9-b7a7-460b-a86d-5dc9055e0d18",
        "build_hdr_config",
        "Builds a detailed HDR dynamic-range-compression config",
    ));
    func_lib.add(config_builder_func::<LocalContrastConfigDef>(
        "f9ebdedf-38e3-4a74-8c74-eb207903d327",
        "build_local_contrast_config",
        "Builds a detailed local-contrast config",
    ));

    // --- per-frame processing nodes (AstroFrame → AstroFrame) ---

    // background_extract: a quick `mode` preset, or a `config` wired from
    // build_background_config (which overrides the preset when present).
    func_lib.add(processing_func(
        "e27c2a02-ec2a-4c6d-afea-60d1276ff8e1",
        "background_extract",
        "Fits and removes a smooth sky-background gradient",
        vec![
            frame_input("image"),
            // One `config` input: pick a `mode` preset (value_options dropdown)
            // or wire a build_background_config node to override it.
            preset_config_input::<BackgroundConfigDef>(
                "config",
                BackgroundModeKind::variant_names(),
            ),
        ],
        FuncLambda::new(move |_, _, _, inputs, _, outputs| {
            Box::pin(async move {
                let config = inputs[1]
                    .value
                    .as_custom::<ConfigValue<BackgroundConfigDef>>()
                    .map(|c| c.0.clone().into())
                    .or_else(|| {
                        inputs[1]
                            .value
                            .as_enum()
                            .and_then(|s| BackgroundModeKind::from_str(s).ok())
                            .map(|mode| BackgroundConfig {
                                mode: mode.config(),
                                ..Default::default()
                            })
                    })
                    .expect("config is required");
                let value = inputs[0].value.clone();
                outputs[0] = run_frame_op(value, move |img| {
                    extract_background(img, &config);
                })
                .await?;
                Ok(())
            })
        }),
    ));

    // denoise
    func_lib.add(processing_func(
        "61c17dfa-8369-446b-b6e7-d91d62d344ee",
        "denoise",
        "Wavelet denoise (starlet coefficient thresholding)",
        vec![
            frame_input("image"),
            float_input("strength", 0.85),
            config_override_input::<DenoiseConfigDef>(),
        ],
        FuncLambda::new(move |_, _, _, inputs, _, outputs| {
            Box::pin(async move {
                let config = inputs[2]
                    .value
                    .as_custom::<ConfigValue<DenoiseConfigDef>>()
                    .map(|c| c.0.clone().into())
                    .unwrap_or_else(|| {
                        let strength = inputs[1]
                            .value
                            .as_f64()
                            .map(|v| v as f32)
                            .expect("strength is required");
                        DenoiseConfig {
                            strength,
                            ..Default::default()
                        }
                    });
                let value = inputs[0].value.clone();
                outputs[0] = run_frame_op(value, move |img| denoise(img, config)).await?;
                Ok(())
            })
        }),
    ));

    // scnr
    func_lib.add(processing_func(
        "ef0c2661-8553-4302-9251-95b2d383af19",
        "scnr",
        "Removes the residual green cast (SCNR)",
        vec![
            frame_input("image"),
            preset_input("method", &SCNR_METHOD_DATATYPE),
        ],
        FuncLambda::new(move |_, _, _, inputs, _, outputs| {
            Box::pin(async move {
                let method = inputs[1]
                    .value
                    .as_enum()
                    .and_then(|s| ScnrKind::from_str(s).ok())
                    .expect("method is required")
                    .config();
                let value = inputs[0].value.clone();
                outputs[0] = run_frame_op(value, move |img| scnr(img, method)).await?;
                Ok(())
            })
        }),
    ));

    // neutralize_background
    func_lib.add(processing_func(
        "5a8c9043-61ca-4a5a-8e55-ce27c804e84b",
        "neutralize_background",
        "Shifts each channel so the background reads neutral gray",
        vec![frame_input("image")],
        FuncLambda::new(move |_, _, _, inputs, _, outputs| {
            Box::pin(async move {
                let value = inputs[0].value.clone();
                outputs[0] = run_frame_op(value, neutralize_background).await?;
                Ok(())
            })
        }),
    ));

    // hdr_compress
    func_lib.add(processing_func(
        "300a2ec5-0ccd-47ec-b282-030eea41441c",
        "hdr_compress",
        "Compresses large-scale dynamic range (multiscale HDR)",
        vec![
            frame_input("image"),
            float_input("amount", 0.5),
            config_override_input::<HdrConfigDef>(),
        ],
        FuncLambda::new(move |_, _, _, inputs, _, outputs| {
            Box::pin(async move {
                let config = inputs[2]
                    .value
                    .as_custom::<ConfigValue<HdrConfigDef>>()
                    .map(|c| c.0.clone().into())
                    .unwrap_or_else(|| {
                        let amount = inputs[1]
                            .value
                            .as_f64()
                            .map(|v| v as f32)
                            .expect("amount is required");
                        HdrConfig {
                            amount,
                            ..Default::default()
                        }
                    });
                let value = inputs[0].value.clone();
                outputs[0] =
                    run_frame_op(value, move |img| compress_dynamic_range(img, config)).await?;
                Ok(())
            })
        }),
    ));

    // local_contrast
    func_lib.add(processing_func(
        "6a28b732-2704-454b-8afd-0a91d385458a",
        "local_contrast",
        "Local contrast enhancement (CLAHE)",
        vec![
            frame_input("image"),
            float_input("strength", 0.8),
            config_override_input::<LocalContrastConfigDef>(),
        ],
        FuncLambda::new(move |_, _, _, inputs, _, outputs| {
            Box::pin(async move {
                let config = inputs[2]
                    .value
                    .as_custom::<ConfigValue<LocalContrastConfigDef>>()
                    .map(|c| c.0.clone().into())
                    .unwrap_or_else(|| {
                        let strength = inputs[1]
                            .value
                            .as_f64()
                            .map(|v| v as f32)
                            .expect("strength is required");
                        LocalContrastConfig {
                            strength,
                            ..Default::default()
                        }
                    });
                let value = inputs[0].value.clone();
                outputs[0] =
                    run_frame_op(value, move |img| enhance_local_contrast(img, config)).await?;
                Ok(())
            })
        }),
    ));

    // star_detect → star count
    func_lib.add(
        Func::new("eb93559d-370c-4bea-aef0-c43897f3416a", "star_detect")
            .description("Detects stars and outputs the count")
            .category("astro")
            .pure()
            .input(frame_input("image"))
            .input(preset_input("detection", &DETECTION_PRESET_DATATYPE))
            .output("count", DataType::Int)
            .lambda(FuncLambda::new(move |_, _, _, inputs, _, outputs| {
                Box::pin(async move {
                    assert_eq!(inputs.len(), 2);
                    assert_eq!(outputs.len(), 1);

                    let config = inputs[1]
                        .value
                        .as_enum()
                        .and_then(|s| DetectionPreset::from_str(s).ok())
                        .expect("detection is required")
                        .config();
                    let value = inputs[0].value.clone();
                    let count = tokio::task::spawn_blocking(move || {
                        let frame = value
                            .as_custom::<AstroFrame>()
                            .expect("image input is an AstroFrame");
                        StarDetector::from_config(config)
                            .detect(&frame.image)
                            .stars
                            .len()
                    })
                    .await
                    .map_err(anyhow::Error::from)?;

                    outputs[0] = (count as i64).into();

                    Ok(())
                })
            })),
    );

    func_lib
}

/// A required preset dropdown input seeded to the enum's first variant. The
/// default keeps a fresh node valid; clearing it surfaces as a missing input.
fn preset_input(name: &str, datatype: &DataType) -> FuncInput {
    let mut input = FuncInput::required(name, datatype.clone());
    input.default_value = datatype.default_value();
    input
}

/// A single config input that's a config `T`'s wire (so a `build_*_config` node
/// can drive it) *and* offers `presets` as a quick-pick dropdown via
/// `value_options` (seeded to the first). The node resolves a wired
/// `ConfigValue<T>` if present, else the picked preset name.
fn preset_config_input<T: NodeConfig>(name: &str, presets: Vec<String>) -> FuncInput {
    let options = presets
        .iter()
        .map(|preset| ValueOption {
            name: preset.clone(),
            value: StaticValue::Enum(preset.clone()),
        })
        .collect();
    let mut input = FuncInput::required(name, config_data_type::<T>()).options(options);
    input.default_value = presets.first().map(|p| StaticValue::Enum(p.clone()));
    input
}

/// An optional `config` override input of config `T`'s custom type, for nodes
/// whose quick knob is an inline scalar (no presets to enumerate). Unbound → the
/// node uses its scalar param; wired from a `build_*_config` node → the full
/// config overrides it.
fn config_override_input<T: NodeConfig>() -> FuncInput {
    FuncInput::optional("config", config_data_type::<T>())
}

/// Wrap a single-channel result plane (coverage / weight) as a grayscale
/// `AstroImage` so it can ride an `AstroFrame` wire and preview.
fn plane_to_frame(plane: Buffer2<f32>) -> AstroImage {
    let dims = ImageDimensions::new((plane.width(), plane.height()), 1);
    AstroImage::from_planar_channels(dims, [Vec::from(plane)])
}

/// An optional calibration-frame folder input (`darks`/`flats`/…): an
/// [`ASTRO_DIR_DATA_TYPE`] directory picker, not required (an unwired role
/// simply yields no master for it).
fn dir_input(name: &str) -> FuncInput {
    FuncInput::optional(name, ASTRO_DIR_DATA_TYPE.clone())
}

/// A required `AstroFrame` input port.
fn frame_input(name: &str) -> FuncInput {
    FuncInput::required(name, ASTRO_FRAME_DATA_TYPE.clone())
}

/// A required float parameter input seeded with `default`.
fn float_input(name: &str, default: f32) -> FuncInput {
    FuncInput::required(name, DataType::Float).default(default as f64)
}

/// Assemble a `Func` for an `AstroFrame → AstroFrame` processing node:
/// `Pure`, category `astro`, a single `image` output. The caller supplies
/// the inputs (the frame first) and the lambda.
fn processing_func(
    id: &str,
    name: &str,
    description: &str,
    inputs: Vec<FuncInput>,
    lambda: FuncLambda,
) -> Func {
    Func::new(id, name)
        .category("astro")
        .description(description)
        .pure()
        .inputs(inputs)
        .output("image", ASTRO_FRAME_DATA_TYPE.clone())
        .lambda(lambda)
}

/// Clone the input `AstroFrame`'s image, apply `op` to it off the worker
/// thread, and wrap the result back into a fresh `AstroFrame` value.
async fn run_frame_op<F>(value: DynamicValue, op: F) -> Result<DynamicValue, anyhow::Error>
where
    F: FnOnce(&mut AstroImage) + Send + 'static,
{
    let image = tokio::task::spawn_blocking(move || {
        let frame = value
            .as_custom::<AstroFrame>()
            .expect("frame input is an AstroFrame");
        let mut image = frame.image.clone();
        op(&mut image);
        image
    })
    .await
    .map_err(anyhow::Error::from)?;
    Ok(DynamicValue::from_custom(AstroFrame::from(image)))
}

/// Build (or load from cache) the four calibration masters for `dirs`
/// (darks / flats / bias / flat_darks, in order). With `cache` on, a master is
/// loaded from `master_<role>.lcm` next to its frames when present, and a
/// freshly-stacked one is written there for next time. Delete the `.lcm` to
/// force a rebuild (a changed frame set is not auto-detected).
fn build_masters_cached(
    dirs: [Option<PathBuf>; 4],
    sigma: f32,
    cache: bool,
) -> anyhow::Result<CalibrationMasters> {
    let [darks, flats, bias, flat_darks] = dirs;
    let role = |dir: Option<PathBuf>,
                config: StackConfig,
                file: &str|
     -> anyhow::Result<Option<CfaImage>> {
        let Some(dir) = dir else {
            return Ok(None);
        };
        let cache_path = dir.join(file);
        if cache && cache_path.exists() {
            return Ok(Some(CfaImage::load(&cache_path)?));
        }
        let frames = astro_image_files(&dir);
        let master = stack_cfa_master(&frames, config).map_err(anyhow::Error::from)?;
        if cache && let Some(master) = &master {
            master.save(&cache_path)?;
        }
        Ok(master)
    };

    Ok(CalibrationMasters::from_images(
        CalibrationImages {
            dark: role(darks, StackConfig::dark(), "master_dark.lcm")?,
            flat: role(flats, StackConfig::flat(), "master_flat.lcm")?,
            bias: role(bias, StackConfig::bias(), "master_bias.lcm")?,
            flat_dark: role(flat_darks, StackConfig::dark(), "master_flat_dark.lcm")?,
        },
        sigma,
    ))
}

#[cfg(test)]
mod tests {
    use scenarium::data::StaticValue;

    use super::*;

    fn func<'a>(lib: &'a FuncLib, name: &str) -> &'a Func {
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
        let lib = astro_funclib();
        let f = func(&lib, "load_astro_image");
        assert_eq!(f.category, "astro");
        assert_eq!(f.inputs.len(), 1);
        assert_eq!(f.outputs.len(), 1);
        assert_eq!(f.inputs[0].data_type, *ASTRO_IMAGE_PATH_DATA_TYPE);
        assert_eq!(f.outputs[0].data_type, *ASTRO_FRAME_DATA_TYPE);
    }

    #[test]
    fn build_masters_node_is_registered() {
        let lib = astro_funclib();
        let f = func(&lib, "build_masters");
        assert_eq!(f.category, "astro");
        assert_eq!(f.outputs.len(), 1);
        assert_eq!(f.outputs[0].data_type, *MASTERS_DATA_TYPE);

        // Four optional calibration-frame folders, then sigma, then cache.
        assert_eq!(f.inputs.len(), 6);
        let dir_names: Vec<&str> = f.inputs[..4].iter().map(|i| i.name.as_str()).collect();
        assert_eq!(dir_names, ["darks", "flats", "bias", "flat_darks"]);
        for input in &f.inputs[..4] {
            assert!(!input.required, "calibration folders are optional");
            assert_eq!(input.data_type, *ASTRO_DIR_DATA_TYPE);
        }
        assert_eq!(f.inputs[4].name, "sigma");
        assert_eq!(f.inputs[4].data_type, DataType::Float);
        assert_eq!(
            f.inputs[4].default_value,
            Some(StaticValue::Float(DEFAULT_SIGMA_THRESHOLD as f64)),
        );
        // Cache toggle defaults on, so masters persist + reload by default.
        assert_eq!(f.inputs[5].name, "cache");
        assert_eq!(f.inputs[5].data_type, DataType::Bool);
        assert_eq!(f.inputs[5].default_value, Some(StaticValue::Bool(true)));
    }

    #[test]
    fn stack_lights_node_is_registered() {
        let lib = astro_funclib();
        let f = func(&lib, "stack_lights");
        assert_eq!(f.category, "astro");

        // One input per stage: lights, masters, detection, registration,
        // combine, reference.
        assert_eq!(f.inputs.len(), 6);
        let names: Vec<&str> = f.inputs.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "lights",
                "masters",
                "detection",
                "registration",
                "combine",
                "reference"
            ]
        );
        assert_eq!(f.inputs[0].data_type, *ASTRO_DIR_DATA_TYPE);
        assert!(f.inputs[0].required, "lights folder is required");
        assert_eq!(f.inputs[1].data_type, *MASTERS_DATA_TYPE);
        assert!(!f.inputs[1].required, "masters are genuinely optional");
        // Each stage is one config-typed input (so a build_*_config wires in),
        // with the presets offered via value_options + seeded to the first.
        // It's required: the seeded preset keeps a fresh node valid, but a
        // cleared input errors the run rather than silently defaulting.
        assert!(f.inputs[2].required, "detection is required");
        assert_eq!(
            f.inputs[2].data_type,
            config_data_type::<DetectionConfigDef>()
        );
        let detection_presets: Vec<&str> = f.inputs[2]
            .value_options
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
        assert_eq!(f.inputs[5].name, "reference");
        assert_eq!(f.inputs[5].default_value, Some(StaticValue::Int(-1)));

        let out_names: Vec<&str> = f.outputs.iter().map(|o| o.name.as_str()).collect();
        assert_eq!(out_names, ["image", "coverage", "weight"]);
        for out in &f.outputs {
            assert_eq!(out.data_type, *ASTRO_FRAME_DATA_TYPE);
        }
    }

    #[test]
    fn auto_stretch_node_is_registered() {
        let lib = astro_funclib();
        let f = func(&lib, "auto_stretch");
        assert_eq!(f.category, "astro");
        assert_eq!(f.inputs.len(), 2);
        assert_eq!(f.inputs[0].name, "image");
        assert_eq!(f.inputs[0].data_type, *ASTRO_FRAME_DATA_TYPE);
        assert!(f.inputs[0].required);
        assert_eq!(f.inputs[1].name, "method");
        assert_eq!(f.inputs[1].data_type, *STRETCH_PRESET_DATATYPE);
        assert_eq!(
            f.inputs[1].default_value,
            Some(StaticValue::Enum("auto_asinh".to_string())),
        );
        assert_eq!(f.outputs.len(), 1);
        assert_eq!(f.outputs[0].data_type, *ASTRO_FRAME_DATA_TYPE);
    }

    #[test]
    fn astro_to_image_node_is_registered() {
        let lib = astro_funclib();
        let f = func(&lib, "astro_to_image");
        assert_eq!(f.category, "astro");
        assert_eq!(f.inputs.len(), 1);
        assert_eq!(f.inputs[0].name, "frame");
        assert_eq!(f.inputs[0].data_type, *ASTRO_FRAME_DATA_TYPE);
        assert!(f.inputs[0].required);
        assert_eq!(f.outputs.len(), 1);
        assert_eq!(f.outputs[0].name, "image");
        assert_eq!(f.outputs[0].data_type, *IMAGE_DATA_TYPE);
    }

    #[test]
    fn processing_nodes_are_registered() {
        let lib = astro_funclib();
        // Each in-place op: a required `image` AstroFrame in, an AstroFrame out.
        for name in [
            "background_extract",
            "denoise",
            "scnr",
            "neutralize_background",
            "hdr_compress",
            "local_contrast",
        ] {
            let f = func(&lib, name);
            assert_eq!(f.category, "astro", "{name} category");
            assert_eq!(f.inputs[0].name, "image", "{name} first input");
            assert_eq!(
                f.inputs[0].data_type, *ASTRO_FRAME_DATA_TYPE,
                "{name} in type"
            );
            assert!(f.inputs[0].required, "{name} image required");
            assert_eq!(f.outputs.len(), 1, "{name} one output");
            assert_eq!(
                f.outputs[0].data_type, *ASTRO_FRAME_DATA_TYPE,
                "{name} out type"
            );
        }
        // star_detect analyzes the frame and outputs a count.
        let sd = func(&lib, "star_detect");
        assert_eq!(sd.inputs[0].data_type, *ASTRO_FRAME_DATA_TYPE);
        assert_eq!(sd.outputs.len(), 1);
        assert_eq!(sd.outputs[0].name, "count");
        assert_eq!(sd.outputs[0].data_type, DataType::Int);
    }

    #[test]
    fn scalar_per_frame_nodes_take_optional_config_overrides() {
        let lib = astro_funclib();
        // denoise / hdr_compress / local_contrast keep their inline scalar and
        // gain an optional `config` override fed by the matching build node.
        let cases: [(&str, &str, DataType); 3] = [
            (
                "denoise",
                "build_denoise_config",
                config_data_type::<DenoiseConfigDef>(),
            ),
            (
                "hdr_compress",
                "build_hdr_config",
                config_data_type::<HdrConfigDef>(),
            ),
            (
                "local_contrast",
                "build_local_contrast_config",
                config_data_type::<LocalContrastConfigDef>(),
            ),
        ];
        for (node, builder, ty) in cases {
            let f = func(&lib, node);
            let config = f.inputs.last().unwrap();
            assert_eq!(config.name, "config", "{node} override input");
            assert_eq!(config.data_type, ty, "{node} override type");
            assert!(!config.required, "{node} config is an optional override");

            // The builder node emits that same config type.
            let b = func(&lib, builder);
            assert_eq!(b.category, "astro");
            assert_eq!(b.outputs[0].data_type, ty, "{builder} output type");
            assert!(
                b.inputs.iter().all(|i| i.required),
                "{builder} fields required"
            );
        }
    }

    #[test]
    fn build_background_config_reflects_fields_and_feeds_background_extract() {
        let lib = astro_funclib();
        // The builder exposes one labeled input per BackgroundConfig field, in
        // struct order; all required (none are `Option`s).
        let builder = func(&lib, "build_background_config");
        assert_eq!(builder.category, "astro");
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
        assert_eq!(builder.outputs[0].name, "config");
        assert_eq!(
            builder.outputs[0].data_type,
            config_data_type::<BackgroundConfigDef>()
        );

        // background_extract is image + one `config` input of that type: a mode
        // preset quick-pick (value_options) a builder can wire into to override.
        let bg = func(&lib, "background_extract");
        let bg_names: Vec<&str> = bg.inputs.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(bg_names, ["image", "config"]);
        assert!(bg.inputs[1].required, "config is required (preset-seeded)");
        assert_eq!(
            bg.inputs[1].data_type,
            config_data_type::<BackgroundConfigDef>()
        );
        let modes: Vec<&str> = bg.inputs[1]
            .value_options
            .iter()
            .map(|o| o.name.as_str())
            .collect();
        assert_eq!(modes, ["subtract", "divide"]);
    }
}
