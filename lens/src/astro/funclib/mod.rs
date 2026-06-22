//! `astro_funclib()` — the `lumos`-backed node library (category `astro`):
//! `load_astro_image` (decode), `build_masters` (calibration masters),
//! `stack_lights` (calibrate + align + stack), and per-frame processing
//! nodes like `auto_stretch`. Heavy work runs off the worker via
//! `spawn_blocking`; preset dropdowns live in [`crate::astro::presets`].

use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{Arc, LazyLock};

use common::CancelToken;
use common::file_utils::astro_image_files;
use imaginarium::Buffer2;
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
use scenarium::func_lambda::{FuncLambda, InvokeError, InvokeResult};
use scenarium::function::{Func, FuncInput, FuncLib, ValueVariant};

use crate::astro::configs::{
    BackgroundConfigDef, CombineConfigDef, DenoiseConfigDef, DetectionConfigDef, HdrConfigDef,
    LocalContrastConfigDef, RegistrationConfigDef, ScnrConfigDef, StretchConfigDef,
};
use crate::astro::masters::{MASTERS_DATA_TYPE, Masters};
use crate::astro::presets::{
    BackgroundModeKind, CombinePreset, DetectionPreset, RegistrationPreset, ScnrKind, StretchPreset,
};
use crate::config_node::{ConfigValue, NodeConfig, config_builder_func, config_data_type};
use crate::image::{IMAGE_DATA_TYPE, Image};
use imaginarium::ProcessingContext;

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
            .output("image", IMAGE_DATA_TYPE.clone())
            .lambda(FuncLambda::new(move |_, _, _, inputs, _, outputs| {
                Box::pin(async move {
                    assert_eq!(inputs.len(), 1);
                    assert_eq!(outputs.len(), 1);

                    let path = inputs[0].value.as_fs_path().unwrap().to_owned();
                    // Decoding (FITS parse / libraw / demosaic) is heavy
                    // synchronous CPU work — keep it off the worker thread.
                    let image = tokio::task::spawn_blocking(move || {
                        AstroImage::from_file(&path).map(|astro| RawImage::from(&astro))
                    })
                    .await
                    .map_err(anyhow::Error::from)?
                    .map_err(anyhow::Error::from)?;

                    outputs[0] = DynamicValue::from_custom(Image::from(image));

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
            .lambda(FuncLambda::new(move |ctx, _, _, inputs, _, outputs| {
                let cancel = ctx.cancel_flag();
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

                    // Stacking many full-resolution CFA frames is heavy CPU work
                    // (polls `cancel` between decode frames + combine rows); a
                    // cached master is loaded instead when present. A cancel
                    // propagates out as `InvokeError::Cancelled` via `?`.
                    let masters = run_cancellable(cancel, move |c| {
                        build_masters_cached(dirs, sigma, cache, c)
                    })
                    .await?;

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
            // Each stage is one input: a preset quick-pick (the `value_variants`
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
            .output("image", IMAGE_DATA_TYPE.clone())
            .output("coverage", IMAGE_DATA_TYPE.clone())
            .output("weight", IMAGE_DATA_TYPE.clone())
            .lambda(FuncLambda::new(move |ctx, _, _, inputs, _, outputs| {
                // Grab the run's cancel flag so the heavy lumos op can poll it.
                let cancel = ctx.cancel_flag();
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
                    // heavy synchronous CPU work that polls `cancel` throughout.
                    // A cancel propagates out as `InvokeError::Cancelled` via `?`.
                    let result = run_cancellable(cancel, move |c| {
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
                        calibrate_align_stack(&lights, masters, &config, c)
                            .map_err(anyhow::Error::from)
                    })
                    .await?;

                    outputs[0] =
                        DynamicValue::from_custom(Image::from(RawImage::from(&result.image)));
                    outputs[1] = DynamicValue::from_custom(Image::from(RawImage::from(
                        &plane_to_frame(result.coverage),
                    )));
                    outputs[2] = DynamicValue::from_custom(Image::from(RawImage::from(
                        &plane_to_frame(result.weight),
                    )));

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
            .input(preset_config_input::<StretchConfigDef>(
                "method",
                StretchPreset::variant_names(),
            ))
            .output("image", IMAGE_DATA_TYPE.clone())
            .lambda(FuncLambda::new(move |_, _, _, inputs, _, outputs| {
                Box::pin(async move {
                    assert_eq!(inputs.len(), 2);
                    assert_eq!(outputs.len(), 1);

                    // A wired build_stretch_config overrides the picked preset.
                    let config = inputs[1]
                        .value
                        .as_custom::<ConfigValue<StretchConfigDef>>()
                        .map(|c| c.0.clone().into())
                        .or_else(|| {
                            inputs[1]
                                .value
                                .as_enum()
                                .and_then(|s| StretchPreset::from_str(s).ok())
                                .map(|preset| preset.config())
                        })
                        .expect("method is required");
                    let value = inputs[0].value.clone();
                    outputs[0] = run_frame_op(value, move |img| stretch(img, config)).await?;

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

    // build_stretch_config / build_scnr_config: detailed overrides for the
    // auto_stretch / scnr preset quick-picks.
    func_lib.add(config_builder_func::<StretchConfigDef>(
        "82f271d4-d047-459a-83aa-0bf8288787cf",
        "build_stretch_config",
        "Builds a detailed display-stretch config",
    ));
    func_lib.add(config_builder_func::<ScnrConfigDef>(
        "d07742d1-4469-4739-b2ff-78b4dcf64132",
        "build_scnr_config",
        "Builds a detailed SCNR (green-removal) config",
    ));

    // --- per-frame processing nodes (Image → Image) ---

    // background_extract: a quick `mode` preset, or a `config` wired from
    // build_background_config (which overrides the preset when present).
    func_lib.add(processing_func(
        "e27c2a02-ec2a-4c6d-afea-60d1276ff8e1",
        "background_extract",
        "Fits and removes a smooth sky-background gradient",
        vec![
            frame_input("image"),
            // One `config` input: pick a `mode` preset (value_variants dropdown)
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
            preset_config_input::<ScnrConfigDef>("method", ScnrKind::variant_names()),
        ],
        FuncLambda::new(move |_, _, _, inputs, _, outputs| {
            Box::pin(async move {
                // A wired build_scnr_config overrides the picked preset.
                let method = inputs[1]
                    .value
                    .as_custom::<ConfigValue<ScnrConfigDef>>()
                    .map(|c| c.0.clone().into())
                    .or_else(|| {
                        inputs[1]
                            .value
                            .as_enum()
                            .and_then(|s| ScnrKind::from_str(s).ok())
                            .map(|preset| preset.config())
                    })
                    .expect("method is required");
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
            .input(preset_config_input::<DetectionConfigDef>(
                "detection",
                DetectionPreset::variant_names(),
            ))
            .output("count", DataType::Int)
            .lambda(FuncLambda::new(move |_, _, _, inputs, _, outputs| {
                Box::pin(async move {
                    assert_eq!(inputs.len(), 2);
                    assert_eq!(outputs.len(), 1);

                    // A wired build_detection_config overrides the picked preset.
                    let config = inputs[1]
                        .value
                        .as_custom::<ConfigValue<DetectionConfigDef>>()
                        .map(|c| c.0.clone().into())
                        .or_else(|| {
                            inputs[1]
                                .value
                                .as_enum()
                                .and_then(|s| DetectionPreset::from_str(s).ok())
                                .map(|preset| preset.config())
                        })
                        .expect("detection is required");
                    // Star detection works on planar channels, so bring the
                    // image back to a CPU `AstroImage` for the detector.
                    let cpu = image_to_cpu(&inputs[0].value)?;
                    let count = tokio::task::spawn_blocking(move || {
                        let astro = AstroImage::from(cpu);
                        StarDetector::from_config(config).detect(&astro).stars.len()
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

/// A single config input that's a config `T`'s wire (so a `build_*_config` node
/// can drive it) *and* offers `presets` as a quick-pick dropdown via
/// `value_variants` (seeded to the first). The node resolves a wired
/// `ConfigValue<T>` if present, else the picked preset name.
fn preset_config_input<T: NodeConfig>(name: &str, presets: Vec<String>) -> FuncInput {
    let variants = presets
        .iter()
        .map(|preset| ValueVariant {
            name: preset.clone(),
            value: StaticValue::Enum(preset.clone()),
        })
        .collect();
    let mut input = FuncInput::required(name, config_data_type::<T>()).variants(variants);
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
/// `AstroImage` so it can ride an `Image` wire (it is converted to `Image` at the node).
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

/// A required `Image` input port (the astro nodes' image currency).
fn frame_input(name: &str) -> FuncInput {
    FuncInput::required(name, IMAGE_DATA_TYPE.clone())
}

/// A required float parameter input seeded with `default`.
fn float_input(name: &str, default: f32) -> FuncInput {
    FuncInput::required(name, DataType::Float).default(default as f64)
}

/// Assemble a `Func` for an `Image → Image` processing node:
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
        .output("image", IMAGE_DATA_TYPE.clone())
        .lambda(lambda)
}

/// Pull the input `Image` to a CPU `imaginarium::Image`, apply the lumos `op`
/// off the worker thread, and wrap the result as an `Image`. The astro pipeline
/// is CPU-backed, so the CPU extraction is a no-op transfer; a GPU-resident
/// input (e.g. straight out of a GPU image-node) would error here — promote it
/// to CPU upstream. (A future version can thread `VisionCtx` to read it back.)
async fn run_frame_op<F>(value: DynamicValue, op: F) -> Result<DynamicValue, anyhow::Error>
where
    F: FnOnce(&mut RawImage) + Send + 'static,
{
    let cpu = image_to_cpu(&value)?;
    let out = tokio::task::spawn_blocking(move || {
        let mut cpu = cpu;
        op(&mut cpu);
        cpu
    })
    .await
    .map_err(anyhow::Error::from)?;
    Ok(DynamicValue::from_custom(Image::from(out)))
}

/// Extract an owned CPU `imaginarium::Image` from a node's `Image` input.
fn image_to_cpu(value: &DynamicValue) -> anyhow::Result<RawImage> {
    let image = value.as_custom::<Image>().expect("image input is an Image");
    let cpu = ProcessingContext::cpu_only();
    Ok(image
        .buffer
        .make_cpu(&cpu)
        .map_err(anyhow::Error::from)?
        .clone())
}

/// Build (or load from cache) the four calibration masters for `dirs`
/// (darks / flats / bias / flat_darks, in order). With `cache` on, a master is
/// loaded from `master_<role>.lcm` next to its frames when present, and a
/// freshly-stacked one is written there for next time. Delete the `.lcm` to
/// force a rebuild (a changed frame set is not auto-detected).
/// Run a cancellable blocking lumos op off the worker. Centralizes the
/// `spawn_blocking` + join handling and the "a cancelled op is a cancel, not a
/// failure" rule the heavy astro nodes share: a lumos op reports cancellation as
/// its own error, so when the token is set its failure is surfaced as
/// [`InvokeError::Cancelled`] (the executor turns that into `Error::Cancelled` —
/// no output, re-runs next time), and a genuine failure as `Err` otherwise.
async fn run_cancellable<T, F>(cancel: CancelToken, op: F) -> InvokeResult<T>
where
    F: FnOnce(CancelToken) -> anyhow::Result<T> + Send + 'static,
    T: Send + 'static,
{
    let cancel_for_op = cancel.clone();
    match tokio::task::spawn_blocking(move || op(cancel_for_op))
        .await
        .map_err(anyhow::Error::from)?
    {
        Ok(value) => Ok(value),
        Err(_) if cancel.is_cancelled() => Err(InvokeError::Cancelled),
        Err(err) => Err(err.into()),
    }
}

fn build_masters_cached(
    dirs: [Option<PathBuf>; 4],
    sigma: f32,
    cache: bool,
    cancel: CancelToken,
) -> anyhow::Result<CalibrationMasters> {
    let [darks, flats, bias, flat_darks] = dirs;
    let role = |dir: Option<PathBuf>,
                config: StackConfig,
                file: &str|
     -> anyhow::Result<Option<CfaImage>> {
        // Bail between roles (a cancel during the cached-master loads stops the
        // next load); a real load/stack error propagates as itself.
        if cancel.is_cancelled() {
            anyhow::bail!("cancelled");
        }
        let Some(dir) = dir else {
            return Ok(None);
        };
        let cache_path = dir.join(file);
        if cache && cache_path.exists() {
            return Ok(Some(CfaImage::load(&cache_path)?));
        }
        let frames = astro_image_files(&dir);
        let master =
            stack_cfa_master(&frames, config, cancel.clone()).map_err(anyhow::Error::from)?;
        if cache && let Some(master) = &master {
            master.save(&cache_path)?;
        }
        Ok(master)
    };

    let masters = CalibrationMasters::from_images(
        CalibrationImages {
            dark: role(darks, StackConfig::dark(), "master_dark.lcm")?,
            flat: role(flats, StackConfig::flat(), "master_flat.lcm")?,
            bias: role(bias, StackConfig::bias(), "master_bias.lcm")?,
            flat_dark: role(flat_darks, StackConfig::dark(), "master_flat_dark.lcm")?,
        },
        sigma,
        cancel.clone(),
    );
    // `from_images` returns a *partial* defect map if cancelled mid-scan, so
    // turn that into an error here — otherwise the bail would look like success.
    if cancel.is_cancelled() {
        anyhow::bail!("cancelled");
    }
    Ok(masters)
}

#[cfg(test)]
mod tests;
