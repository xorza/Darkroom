//! `astro_library()` — the `lumos`-backed node library (category `astro`):
//! `load_astro_image` (decode), `build_masters` (calibration masters),
//! `stack_lights` (calibrate + align + stack), and per-frame processing
//! nodes like `auto_stretch`. Heavy work runs off the worker via
//! `spawn_blocking`; preset dropdowns live in [`crate::astro::presets`].

use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{Arc, LazyLock, RwLock};

use anyhow::Context;
use common::CancelToken;
use common::file_utils;
use imaginarium::Buffer2;
use imaginarium::Image as RawImage;
use lumos::{
    ASTRO_IMAGE_EXTENSIONS, AlignStackConfig, AstroImage, CalibrationMasters, CalibrationSet,
    CfaImage, DEFAULT_SIGMA_THRESHOLD, Denoise, ExtractBackground, Hdr, ImageDimensions,
    LocalContrast, MlError, NeutralizeBackground, OpError, RAW_EXTENSIONS, Reference, StackConfig,
    TiledOnnxConfig, calibrate_align_stack, ml_denoise, remove_stars, remove_stars_starless_only,
    stack_cfa_master,
};
use scenarium::{DataType, DynamicValue, FsPathConfig, FsPathMode};
use scenarium::{Func, FuncInput, FuncOutput, ValueVariant};
use scenarium::{FuncLambda, InvokeError, InvokeResult};
use scenarium::{Library, TypeEntry};

use crate::astro::configs::{
    BackgroundConfigDef, CombineConfigDef, DenoiseConfigDef, DetectionConfigDef, HdrConfigDef,
    LocalContrastConfigDef, RegistrationConfigDef, ScnrConfigDef, StretchConfigDef,
};
use crate::astro::masters::{MASTERS_DATA_TYPE, MASTERS_TYPE_ID, Masters};
use crate::astro::presets::{
    BackgroundModeKind, CombinePreset, DetectionPreset, RegistrationPreset, ScnrKind, StretchPreset,
};
use crate::config_node::{ConfigValue, NodeConfig, add_config_builder, config_data_type};
use crate::image::{IMAGE_DATA_TYPE, Image};
use imaginarium::ProcessingContext;

/// ONNX model paths for the ML nodes (`ml_denoise` / `remove_stars`). lumos ships no models, so these
/// are caller-supplied and configured at runtime — darkroom's settings window sets them via
/// [`set_ml_model_paths`]. The nodes read the current paths each run, so a config change takes effect
/// on the next invocation. Defaults are bare filenames resolved against the working directory.
#[derive(Debug, Clone)]
pub struct MlModelPaths {
    /// ONNX denoiser model (e.g. DeepSNR), used by `ml_denoise`.
    pub denoise: PathBuf,
    /// StarNet-style star-removal ONNX model, used by `remove_stars`.
    pub star_removal: PathBuf,
}

impl Default for MlModelPaths {
    fn default() -> Self {
        Self {
            denoise: PathBuf::from("DeepSNR_weights_v2.onnx"),
            star_removal: PathBuf::from("StarNet2_weights.onnx"),
        }
    }
}

static ML_MODEL_PATHS: LazyLock<RwLock<MlModelPaths>> =
    LazyLock::new(|| RwLock::new(MlModelPaths::default()));

/// Set the ONNX model paths the `ml_denoise` / `remove_stars` nodes use (darkroom's config window).
pub fn set_ml_model_paths(paths: MlModelPaths) {
    *ML_MODEL_PATHS
        .write()
        .expect("ml model paths lock poisoned") = paths;
}

/// The currently-configured ML model paths (so the config window can populate its fields).
pub fn ml_model_paths() -> MlModelPaths {
    ML_MODEL_PATHS
        .read()
        .expect("ml model paths lock poisoned")
        .clone()
}

/// Reusable data type for an astro image file-path input: an existing-file
/// picker filtered to [`ASTRO_IMAGE_EXTENSIONS`]. Shared so every astro
/// loader presents the same filter.
pub(crate) static ASTRO_IMAGE_PATH_DATA_TYPE: LazyLock<DataType> = LazyLock::new(|| {
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
/// which scan the directory for camera-RAW frames.
pub(crate) static ASTRO_DIR_DATA_TYPE: LazyLock<DataType> =
    LazyLock::new(|| DataType::FsPath(Arc::new(FsPathConfig::new(FsPathMode::Directory))));

/// The lumos-backed astro nodes (category `astro`).
pub fn astro_library() -> Library {
    let mut library = Library::default();

    library.register_type(*MASTERS_TYPE_ID, TypeEntry::custom("Masters"));

    library.add(
        Func::new("fbcc8899-efc3-40e0-a6fd-8743f86edbd3", "Load Astro Image")
            .description("Loads a FITS/RAW/standard astronomical image.")
            .category("Astro")
            .pure()
            .input(
                FuncInput::required("Path", ASTRO_IMAGE_PATH_DATA_TYPE.clone())
                    .description("FITS, camera-RAW, or standard image file to load."),
            )
            .output(FuncOutput::new("Image", IMAGE_DATA_TYPE.clone()).description("Decoded frame."))
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

    library.add(
        Func::new("f2f6f1ff-5b10-409c-900f-d6b48750a529", "Build Masters")
            .description(
                "Stacks raw calibration frames (darks/flats/bias/flat-darks) into calibration \
                 masters. With `cache` on, each master is written next to its frames and reused \
                 next run instead of re-stacking.",
            )
            .category("Astro")
            // `Pure`: its digest is the structural fold of its inputs, so the directory-aware
            // `FsPath` resolver keys it on each calibration folder's *contents* (sorted entry
            // `(name, len, mtime)`). A stable folder caches; any add/remove/edit re-keys it and
            // it recomputes. (Its `cache` toggle still reloads masters from the `.lcm` files it
            // writes next to the frames, keeping the recompute cheap.)
            .pure()
            .inputs([
                dir_input("Darks", "dark frames"),
                dir_input("Flats", "flat frames"),
                dir_input("Bias", "bias frames"),
                dir_input("Flat Darks", "flat-dark frames"),
            ])
            .input(
                FuncInput::required("Sigma", DataType::Float)
                    .description("Sigma-clipping rejection threshold when stacking.")
                    .default(DEFAULT_SIGMA_THRESHOLD as f64),
            )
            .input(
                FuncInput::required("Cache", DataType::Bool)
                    .description("Write each master next to its frames and reuse it next run.")
                    .default(true),
            )
            .output(
                FuncOutput::new("Masters", MASTERS_DATA_TYPE.clone())
                    .description("Calibration masters for the wired roles."),
            )
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

    library.add(
        Func::new("b02f5c42-7bda-48f6-81dd-81338efbb126", "Stack Lights")
            .description("Calibrates, aligns, and stacks a folder of light frames into one image.")
            .category("Astro")
            // Pure: the digest folds the `lights` folder's contents (the
            // directory-aware `FsPath` resolver), so an unchanged folder caches
            // and any add/remove/edit re-keys it — no purity override needed.
            .pure()
            .input(
                FuncInput::required("Lights", ASTRO_DIR_DATA_TYPE.clone())
                    .description("Folder of light frames to stack."),
            )
            .input(
                FuncInput::optional("Masters", MASTERS_DATA_TYPE.clone())
                    .description("Optional calibration masters. Unwired means no calibration."),
            )
            // Each stage is one input: a preset quick-pick (the `value_variants`
            // dropdown) that a build_*_config node can wire into to override.
            .input(preset_config_input::<DetectionConfigDef>(
                "Detection",
                DetectionPreset::picker_variants(),
            ))
            .input(preset_config_input::<RegistrationConfigDef>(
                "Registration",
                RegistrationPreset::picker_variants(),
            ))
            .input(preset_config_input::<CombineConfigDef>(
                "Combine",
                CombinePreset::picker_variants(),
            ))
            // reference: < 0 picks the frame with the most stars (auto); >= 0 is
            // a 0-based index into the (directory-sorted) light frames.
            .input(
                FuncInput::required("Reference", DataType::Int)
                    .description(
                        "Alignment reference frame index; −1 auto-picks the richest frame.",
                    )
                    .default(-1_i64),
            )
            .output(FuncOutput::new("Image", IMAGE_DATA_TYPE.clone()).description("Stacked image."))
            .output(
                FuncOutput::new("Coverage", IMAGE_DATA_TYPE.clone())
                    .description("Per-pixel frame-count map."),
            )
            .output(
                FuncOutput::new("Weight", IMAGE_DATA_TYPE.clone())
                    .description("Per-pixel accumulated weight map."),
            )
            .lambda(FuncLambda::new(move |ctx, _, _, inputs, _, outputs| {
                // Grab the run's cancel flag so the heavy lumos op can poll it.
                let cancel = ctx.cancel_flag();
                Box::pin(async move {
                    assert_eq!(inputs.len(), 6);
                    assert_eq!(outputs.len(), 3);

                    let Some(lights_dir) = inputs[0].value.as_fs_path().map(PathBuf::from) else {
                        return Err(InvokeError::External(anyhow::anyhow!(
                            "no light-frame folder is set"
                        )));
                    };
                    let lights = raw_frame_files(&lights_dir).map_err(InvokeError::External)?;
                    if lights.is_empty() {
                        return Err(InvokeError::External(anyhow::anyhow!(
                            "no camera-RAW frames found in light-frame folder '{}'",
                            lights_dir.display()
                        )));
                    }
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
                        .expect("detection config is validated at the compile boundary");
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
                        .expect("registration config is validated at the compile boundary");
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
                        .expect("combine config is validated at the compile boundary");
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
                        let empty = CalibrationMasters::default();
                        let masters = masters_val
                            .as_custom::<Masters>()
                            .map(|m| &m.masters)
                            .unwrap_or(&empty);
                        calibrate_align_stack(&lights, masters, &config, c)
                            .map_err(anyhow::Error::from)
                    })
                    .await?;

                    outputs[0] = DynamicValue::from_custom(Image::from(RawImage::from(
                        &result.product.image,
                    )));
                    outputs[1] = DynamicValue::from_custom(Image::from(RawImage::from(
                        &plane_to_frame(result.product.coverage),
                    )));
                    outputs[2] = DynamicValue::from_custom(Image::from(RawImage::from(
                        &plane_to_frame(result.product.weight),
                    )));

                    Ok(())
                })
            })),
    );

    library.add(
        Func::new("c15248e0-006a-4a4a-9aae-b1fc7886dea1", "Auto Stretch")
            .description("Auto-stretches a linear frame to a viewable image (display tone curve).")
            .category("Astro")
            .pure()
            .input(frame_input("Image"))
            .input(preset_config_input::<StretchConfigDef>(
                "Method",
                StretchPreset::picker_variants(),
            ))
            .output(
                FuncOutput::new("Image", IMAGE_DATA_TYPE.clone())
                    .description("Stretched, display-ready image."),
            )
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
                        .expect("stretch method is validated at the compile boundary");
                    let value = std::mem::take(&mut inputs[0].value);
                    outputs[0] = run_frame_op(value, move |img| config.apply(img)).await?;

                    Ok(())
                })
            })),
    );

    // build_background_config: expose every ExtractBackground field as an input,
    // output a detailed config to wire into background_extract.
    add_config_builder::<BackgroundConfigDef>(
        &mut library,
        "9cda0462-1b8e-4c50-83d6-4db470df22d9",
        "Build Background Config",
        "Builds a detailed background-extraction config",
    );

    // build_detection_config / build_registration_config / build_combine_config:
    // detailed overrides for stack_lights' detection / registration / combine
    // preset dropdowns.
    add_config_builder::<DetectionConfigDef>(
        &mut library,
        "6c6f92e7-0f74-454c-acc4-68691cb8462f",
        "Build Detection Config",
        "Builds a detailed star-detection config",
    );
    add_config_builder::<RegistrationConfigDef>(
        &mut library,
        "adf216fe-baa9-4abd-8c4a-bfb98bb60fbc",
        "Build Registration Config",
        "Builds a detailed registration config",
    );
    add_config_builder::<CombineConfigDef>(
        &mut library,
        "05313ceb-a3b2-4488-92af-c9e228bb1789",
        "Build Combine Config",
        "Builds a detailed frame-combination config",
    );

    // build_denoise_config / build_hdr_config / build_local_contrast_config:
    // full configs for the per-frame nodes whose inline param is one scalar.
    add_config_builder::<DenoiseConfigDef>(
        &mut library,
        "77693298-3531-4858-89ce-03cb347dc3f2",
        "Build Denoise Config",
        "Builds a detailed wavelet-denoise config",
    );
    add_config_builder::<HdrConfigDef>(
        &mut library,
        "dc82d7a9-b7a7-460b-a86d-5dc9055e0d18",
        "Build HDR Config",
        "Builds a detailed HDR dynamic-range-compression config",
    );
    add_config_builder::<LocalContrastConfigDef>(
        &mut library,
        "f9ebdedf-38e3-4a74-8c74-eb207903d327",
        "Build Local Contrast Config",
        "Builds a detailed local-contrast config",
    );

    // build_stretch_config / build_scnr_config: detailed overrides for the
    // auto_stretch / scnr preset quick-picks.
    add_config_builder::<StretchConfigDef>(
        &mut library,
        "82f271d4-d047-459a-83aa-0bf8288787cf",
        "Build Stretch Config",
        "Builds a detailed display-stretch config",
    );
    add_config_builder::<ScnrConfigDef>(
        &mut library,
        "d07742d1-4469-4739-b2ff-78b4dcf64132",
        "Build SCNR Config",
        "Builds a detailed SCNR (green-removal) config",
    );

    // background_extract: a quick `mode` preset, or a `config` wired from
    // build_background_config (which overrides the preset when present).
    library.add(processing_func(
        "e27c2a02-ec2a-4c6d-afea-60d1276ff8e1",
        "Extract Background",
        "Fits and removes a smooth sky-background gradient.",
        vec![
            frame_input("Image"),
            // One `Config` input: pick a `mode` preset (value_variants dropdown)
            // or wire a build_background_config node to override it.
            preset_config_input::<BackgroundConfigDef>(
                "Config",
                BackgroundModeKind::picker_variants(),
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
                            .map(|mode| ExtractBackground {
                                mode: mode.config(),
                                ..Default::default()
                            })
                    })
                    .expect("background config is validated at the compile boundary");
                let value = std::mem::take(&mut inputs[0].value);
                outputs[0] = run_frame_op(value, move |img| config.apply(img)).await?;
                Ok(())
            })
        }),
    ));

    library.add(processing_func(
        "61c17dfa-8369-446b-b6e7-d91d62d344ee",
        "Denoise",
        "Wavelet denoise (starlet coefficient thresholding).",
        vec![
            frame_input("Image"),
            float_input("Strength", 0.85, "Denoise strength in [0, 1]."),
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
                        Denoise {
                            strength,
                            ..Default::default()
                        }
                    });
                let value = std::mem::take(&mut inputs[0].value);
                outputs[0] = run_frame_op(value, move |img| config.apply(img)).await?;
                Ok(())
            })
        }),
    ));

    library.add(processing_func(
        "ef0c2661-8553-4302-9251-95b2d383af19",
        "SCNR",
        "Removes the residual green cast (SCNR).",
        vec![
            frame_input("Image"),
            preset_config_input::<ScnrConfigDef>("Method", ScnrKind::picker_variants()),
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
                    .expect("scnr method is validated at the compile boundary");
                let value = std::mem::take(&mut inputs[0].value);
                outputs[0] = run_frame_op(value, move |img| method.apply(img)).await?;
                Ok(())
            })
        }),
    ));

    library.add(processing_func(
        "5a8c9043-61ca-4a5a-8e55-ce27c804e84b",
        "Neutralize Background",
        "Shifts each channel so the background reads neutral gray.",
        vec![frame_input("Image")],
        FuncLambda::new(move |_, _, _, inputs, _, outputs| {
            Box::pin(async move {
                let value = std::mem::take(&mut inputs[0].value);
                outputs[0] = run_frame_op(value, |img| NeutralizeBackground.apply(img)).await?;
                Ok(())
            })
        }),
    ));

    library.add(processing_func(
        "300a2ec5-0ccd-47ec-b282-030eea41441c",
        "HDR Compression",
        "Compresses large-scale dynamic range (multiscale HDR).",
        vec![
            frame_input("Image"),
            float_input("Amount", 0.5, "Compression amount in [0, 1]."),
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
                        Hdr {
                            amount,
                            ..Default::default()
                        }
                    });
                let value = std::mem::take(&mut inputs[0].value);
                outputs[0] = run_frame_op(value, move |img| config.apply(img)).await?;
                Ok(())
            })
        }),
    ));

    library.add(processing_func(
        "6a28b732-2704-454b-8afd-0a91d385458a",
        "Local Contrast",
        "Local contrast enhancement (CLAHE).",
        vec![
            frame_input("Image"),
            float_input("Strength", 0.8, "Local-contrast strength in [0, 1]."),
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
                        LocalContrast {
                            strength,
                            ..Default::default()
                        }
                    });
                let value = std::mem::take(&mut inputs[0].value);
                outputs[0] = run_frame_op(value, move |img| config.apply(img)).await?;
                Ok(())
            })
        }),
    ));

    // ml_denoise — caller-supplied ONNX denoiser (DeepSNR), display-domain.
    library.add(processing_func(
        "ace786f9-8a02-4ed1-93a0-ad67bf0680f8",
        "ML Denoise",
        "Denoise a stretched image with a caller-supplied ONNX model (DeepSNR).",
        vec![frame_input("Image")],
        FuncLambda::new(move |_, _, _, inputs, _, outputs| {
            Box::pin(async move {
                let model = ml_model_paths().denoise;
                let out = run_ml(std::mem::take(&mut inputs[0].value), move |img| {
                    ml_denoise(&img, &TiledOnnxConfig::new(model))
                })
                .await?;
                outputs[0] = DynamicValue::from_custom(Image::from(out));
                Ok(())
            })
        }),
    ));

    // remove_stars — caller-supplied StarNet ONNX model → starless + recovered stars layers.
    library.add(
        Func::new("60c31a76-eed4-467c-9ba3-5c89d294a91b", "ML Star Removal")
            .description(
                "Removes stars with a caller-supplied StarNet ONNX model (starless + stars).",
            )
            .category("Astro")
            .pure()
            .input(frame_input("Image"))
            .output(
                FuncOutput::new("Starless", IMAGE_DATA_TYPE.clone())
                    .description("The image with stars removed."),
            )
            .output(
                FuncOutput::new("Stars", IMAGE_DATA_TYPE.clone())
                    .description("The recovered star layer."),
            )
            .lambda(FuncLambda::new(
                move |_, _, _, inputs, output_demand, outputs| {
                    // The unscreen pass that recovers "Stars" is a whole-image pixel loop on
                    // top of the ONNX inference — skip it when nothing reads that output.
                    let need_stars = !output_demand[1].is_skip();
                    Box::pin(async move {
                        let model = ml_model_paths().star_removal;
                        if need_stars {
                            let result = run_ml(std::mem::take(&mut inputs[0].value), move |img| {
                                remove_stars(img, &TiledOnnxConfig::new(model))
                            })
                            .await?;
                            outputs[0] = DynamicValue::from_custom(Image::from(result.starless));
                            outputs[1] = DynamicValue::from_custom(Image::from(result.stars));
                        } else {
                            let starless =
                                run_ml(std::mem::take(&mut inputs[0].value), move |img| {
                                    remove_stars_starless_only(&img, &TiledOnnxConfig::new(model))
                                })
                                .await?;
                            outputs[0] = DynamicValue::from_custom(Image::from(starless));
                        }
                        Ok(())
                    })
                },
            )),
    );

    library
}

/// A single config input that's a config `T`'s wire (so a `build_*_config` node
/// can drive it) *and* offers `variants` as a quick-pick dropdown (seeded to the
/// first). The node resolves a wired `ConfigValue<T>` if present, else the picked
/// preset name. Each variant stores the raw preset label but shows its friendly
/// [`ValueVariant::display_name`], so a saved graph keeps working while the picker
/// reads human-friendly (e.g. shows "Wide Field", stores `wide_field`).
fn preset_config_input<T: NodeConfig>(name: &str, variants: Vec<ValueVariant>) -> FuncInput {
    let default_value = variants.first().map(|v| v.value.clone());
    let mut input = FuncInput::required(name, config_data_type::<T>())
        .description("Preset quick-pick; wire a matching build config node to override it.")
        .variants(variants);
    input.default_value = default_value;
    input
}

/// An optional `config` override input of config `T`'s custom type, for nodes
/// whose quick knob is an inline scalar (no presets to enumerate). Unbound → the
/// node uses its scalar param; wired from a `build_*_config` node → the full
/// config overrides it.
fn config_override_input<T: NodeConfig>() -> FuncInput {
    FuncInput::optional("Config", config_data_type::<T>())
        .description("Optional detailed config; overrides the inline knob when wired.")
}

/// Wrap a single-channel result plane (coverage / weight) as a grayscale
/// `AstroImage` so it can ride an `Image` wire (it is converted to `Image` at the node).
fn plane_to_frame(plane: Buffer2<f32>) -> AstroImage {
    let dims = ImageDimensions::new((plane.width(), plane.height()), 1);
    AstroImage::from_planar_channels(dims, [Vec::from(plane)])
}

/// An optional calibration-frame folder input (`Darks`/`Flats`/…): an
/// [`ASTRO_DIR_DATA_TYPE`] directory picker, not required (an unwired role
/// simply yields no master for it). `what` completes the tooltip
/// ("Folder of {what}.").
fn dir_input(name: &str, what: &str) -> FuncInput {
    FuncInput::optional(name, ASTRO_DIR_DATA_TYPE.clone()).description(format!("Folder of {what}."))
}

/// A required `Image` input port (the astro nodes' image currency).
fn frame_input(name: &str) -> FuncInput {
    FuncInput::required(name, IMAGE_DATA_TYPE.clone()).description("Image to process.")
}

/// A required float parameter input seeded with `default`, with a tooltip.
fn float_input(name: &str, default: f32, description: &str) -> FuncInput {
    FuncInput::required(name, DataType::Float)
        .description(description)
        .default(default as f64)
}

/// Assemble a `Func` for an `Image → Image` processing node:
/// `Pure`, category `Astro`, a single `Image` output. The caller supplies
/// the inputs (the frame first) and the lambda.
fn processing_func(
    id: &str,
    name: &str,
    description: &str,
    inputs: Vec<FuncInput>,
    lambda: FuncLambda,
) -> Func {
    Func::new(id, name)
        .category("Astro")
        .description(description)
        .pure()
        .inputs(inputs)
        .output(FuncOutput::new("Image", IMAGE_DATA_TYPE.clone()).description("Processed image."))
        .lambda(lambda)
}

/// Pull the input `Image` to a CPU `imaginarium::Image`, apply the lumos `op`
/// off the worker thread, and wrap the result as an `Image`. The astro pipeline
/// is CPU-backed, so the CPU extraction is a no-op transfer; a GPU-resident
/// input (e.g. straight out of a GPU image-node) would error here — promote it
/// to CPU upstream. (A future version can thread `VisionCtx` to read it back.)
async fn run_frame_op<F>(value: DynamicValue, op: F) -> Result<DynamicValue, anyhow::Error>
where
    F: FnOnce(&mut RawImage) -> Result<(), OpError> + Send + 'static,
{
    let cpu = image_to_cpu(value)?;
    let out = tokio::task::spawn_blocking(move || {
        let mut cpu = cpu;
        op(&mut cpu)?;
        Ok::<_, OpError>(cpu)
    })
    .await
    .map_err(anyhow::Error::from)? // join error
    .map_err(anyhow::Error::from)?; // op rejected the config / format
    Ok(DynamicValue::from_custom(Image::from(out)))
}

/// Run a caller-supplied ONNX op (`ml_denoise` / `remove_stars`) off the worker: pull the input
/// `Image` to CPU, run `op` on a blocking thread (ONNX inference), and surface an [`MlError`]
/// (missing model file, image smaller than the model window) as the node's error. Hands `op`
/// the CPU image by value — this input isn't read again afterward, so `remove_stars` can
/// repurpose its buffer for the `stars` output instead of allocating a fresh one; `op`s that
/// only need a borrow (`ml_denoise`, `remove_stars_starless_only`) just reborrow it.
async fn run_ml<R, F>(value: DynamicValue, op: F) -> anyhow::Result<R>
where
    F: FnOnce(RawImage) -> Result<R, MlError> + Send + 'static,
    R: Send + 'static,
{
    let cpu = image_to_cpu(value)?;
    tokio::task::spawn_blocking(move || op(cpu))
        .await
        .map_err(anyhow::Error::from)? // join error
        .map_err(anyhow::Error::from) // model load / inference failure
}

/// Extract an owned CPU `imaginarium::Image` from a node's `Image` input. A uniquely
/// held input — the executor's move-on-last-use, the steady state for non-RAM astro
/// chains — is consumed without a pixel copy: its buffer is taken whole. A shared one
/// (RAM-cached producer, fan-out, in-flight inspection) falls back to deep-cloning the
/// CPU view, so correctness never depends on the move.
fn image_to_cpu(value: DynamicValue) -> anyhow::Result<RawImage> {
    let cpu = ProcessingContext::cpu_only();
    match value.into_custom::<Image>() {
        Ok(image) => image.buffer.to_cpu(&cpu).map_err(anyhow::Error::from),
        Err(value) => {
            let image = value
                .as_custom::<Image>()
                .expect("image input type is validated at the compile boundary");
            Ok(image
                .buffer
                .make_cpu(&cpu)
                .map_err(anyhow::Error::from)?
                .clone())
        }
    }
}

/// Build (or load from cache) the four calibration masters for `dirs`
/// (darks / flats / bias / flat_darks, in order). With `cache` on, a master is
/// loaded from `master_<role>.lcm` next to its frames when present, and a
/// freshly-stacked one is written there for next time. An unreadable or stale
/// cache is rebuilt from its source folder.
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
        // next load); a real source scan/stack error propagates as itself.
        if cancel.is_cancelled() {
            anyhow::bail!("cancelled");
        }
        let Some(dir) = dir else {
            return Ok(None);
        };
        let cache_path = dir.join(file);
        if cache && cache_path.exists() {
            match CfaImage::load(&cache_path) {
                Ok(master) => return Ok(Some(master)),
                Err(error) => tracing::warn!(
                    path = %cache_path.display(),
                    %error,
                    "failed to load calibration master cache; rebuilding from source frames"
                ),
            }
        }
        let frames = raw_frame_files(&dir)?;
        let master =
            stack_cfa_master(&frames, config, cancel.clone()).map_err(anyhow::Error::from)?;
        if cache && let Some(master) = &master {
            master.save(&cache_path)?;
        }
        Ok(master)
    };

    CalibrationMasters::from_images(
        CalibrationSet {
            dark: role(darks, StackConfig::dark(), "master_dark.lcm")?,
            flat: role(flats, StackConfig::flat(), "master_flat.lcm")?,
            bias: role(bias, StackConfig::bias(), "master_bias.lcm")?,
            flat_dark: role(flat_darks, StackConfig::dark(), "master_flat_dark.lcm")?,
        },
        sigma,
        cancel,
    )
    .map_err(anyhow::Error::from)
}

fn raw_frame_files(dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    file_utils::files_with_extensions(dir, RAW_EXTENSIONS)
        .with_context(|| format!("failed to scan camera-RAW frame folder '{}'", dir.display()))
}

#[cfg(test)]
mod tests;
