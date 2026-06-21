//! `AstroFuncLib` — the `lumos`-backed node library (category `astro`):
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
    AlignStackConfig, AstroImage, BackgroundConfig, CalibrationFrames, CalibrationMasters,
    DEFAULT_SIGMA_THRESHOLD, DenoiseConfig, HdrConfig, ImageDimensions, LocalContrastConfig,
    Reference, StarDetector, calibrate_align_stack, compress_dynamic_range, denoise,
    enhance_local_contrast, extract_background, neutralize_background, scnr, stretch,
};
use scenarium::data::{DataType, DynamicValue, FsPathConfig, FsPathMode};
use scenarium::func_lambda::FuncLambda;
use scenarium::function::{Func, FuncBehavior, FuncInput, FuncLib, FuncOutput};
use scenarium::graph::NodeBehavior;

use crate::astro_frame::{ASTRO_FRAME_DATA_TYPE, AstroFrame};
use crate::astro_presets::{
    BACKGROUND_MODE_DATATYPE, BackgroundModeKind, COMBINE_PRESET_DATATYPE, CombinePreset,
    DETECTION_PRESET_DATATYPE, DetectionPreset, REGISTRATION_PRESET_DATATYPE, RegistrationPreset,
    SCNR_METHOD_DATATYPE, STRETCH_PRESET_DATATYPE, ScnrKind, StretchPreset,
};
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

#[derive(Debug)]
pub struct AstroFuncLib {
    func_lib: FuncLib,
}

impl AstroFuncLib {
    pub fn func_lib(&self) -> &FuncLib {
        &self.func_lib
    }

    pub fn into_func_lib(self) -> FuncLib {
        self.func_lib
    }
}

impl From<AstroFuncLib> for FuncLib {
    fn from(astro: AstroFuncLib) -> Self {
        astro.func_lib
    }
}

impl Default for AstroFuncLib {
    fn default() -> Self {
        let mut func_lib = FuncLib::default();

        // load_astro_image
        func_lib.add(Func {
            id: "fbcc8899-efc3-40e0-a6fd-8743f86edbd3".into(),
            name: "load_astro_image".to_string(),
            description: Some("Loads a FITS/RAW/standard astronomical image".to_string()),
            behavior: FuncBehavior::Impure,
            node_default_behavior: NodeBehavior::Once,
            terminal: false,
            category: "astro".to_string(),
            inputs: vec![FuncInput {
                name: "path".to_string(),
                required: true,
                data_type: ASTRO_IMAGE_PATH_DATA_TYPE.clone(),
                default_value: None,
                value_options: vec![],
            }],
            outputs: vec![FuncOutput {
                name: "image".to_string(),
                data_type: ASTRO_FRAME_DATA_TYPE.clone(),
            }],
            events: vec![],
            required_contexts: vec![],
            lambda: FuncLambda::new(move |_, _, _, inputs, _, outputs| {
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
            }),
        });

        // build_masters
        func_lib.add(Func {
            id: "f2f6f1ff-5b10-409c-900f-d6b48750a529".into(),
            name: "build_masters".to_string(),
            description: Some(
                "Stacks raw calibration frames (darks/flats/bias/flat-darks) into calibration \
                 masters"
                    .to_string(),
            ),
            behavior: FuncBehavior::Impure,
            node_default_behavior: NodeBehavior::Once,
            terminal: false,
            category: "astro".to_string(),
            inputs: vec![
                dir_input("darks"),
                dir_input("flats"),
                dir_input("bias"),
                dir_input("flat_darks"),
                FuncInput {
                    name: "sigma".to_string(),
                    required: false,
                    data_type: DataType::Float,
                    default_value: Some((DEFAULT_SIGMA_THRESHOLD as f64).into()),
                    value_options: vec![],
                },
            ],
            outputs: vec![FuncOutput {
                name: "masters".to_string(),
                data_type: MASTERS_DATA_TYPE.clone(),
            }],
            events: vec![],
            required_contexts: vec![],
            lambda: FuncLambda::new(move |_, _, _, inputs, _, outputs| {
                Box::pin(async move {
                    assert_eq!(inputs.len(), 5);
                    assert_eq!(outputs.len(), 1);

                    // Each optional folder globs to its astro frames (empty
                    // when the port is unbound or the directory is unreadable).
                    let frames_in = |idx: usize| -> Vec<PathBuf> {
                        inputs[idx]
                            .value
                            .as_fs_path()
                            .map(|dir| astro_image_files(Path::new(dir)))
                            .unwrap_or_default()
                    };
                    let darks = frames_in(0);
                    let flats = frames_in(1);
                    let bias = frames_in(2);
                    let flat_darks = frames_in(3);
                    let sigma = inputs[4]
                        .value
                        .as_f64()
                        .map(|v| v as f32)
                        .unwrap_or(DEFAULT_SIGMA_THRESHOLD);

                    // Stacking many full-resolution CFA frames is heavy CPU work.
                    let masters = tokio::task::spawn_blocking(move || {
                        CalibrationMasters::from_files(
                            CalibrationFrames {
                                darks: &darks,
                                flats: &flats,
                                bias: &bias,
                                flat_darks: &flat_darks,
                            },
                            sigma,
                        )
                    })
                    .await
                    .map_err(anyhow::Error::from)?
                    .map_err(anyhow::Error::from)?;

                    outputs[0] = DynamicValue::from_custom(Masters::from(masters));

                    Ok(())
                })
            }),
        });

        // stack_lights
        func_lib.add(Func {
            id: "b02f5c42-7bda-48f6-81dd-81338efbb126".into(),
            name: "stack_lights".to_string(),
            description: Some(
                "Calibrates, aligns and stacks a folder of light frames into one image".to_string(),
            ),
            behavior: FuncBehavior::Impure,
            node_default_behavior: NodeBehavior::Once,
            terminal: false,
            category: "astro".to_string(),
            inputs: vec![
                FuncInput {
                    name: "lights".to_string(),
                    required: true,
                    data_type: ASTRO_DIR_DATA_TYPE.clone(),
                    default_value: None,
                    value_options: vec![],
                },
                FuncInput {
                    name: "masters".to_string(),
                    required: false,
                    data_type: MASTERS_DATA_TYPE.clone(),
                    default_value: None,
                    value_options: vec![],
                },
                preset_input("detection", &DETECTION_PRESET_DATATYPE),
                preset_input("registration", &REGISTRATION_PRESET_DATATYPE),
                preset_input("combine", &COMBINE_PRESET_DATATYPE),
                FuncInput {
                    name: "reference".to_string(),
                    required: false,
                    // < 0 picks the frame with the most stars (auto); >= 0 is a
                    // 0-based index into the (directory-sorted) light frames.
                    data_type: DataType::Int,
                    default_value: Some((-1_i64).into()),
                    value_options: vec![],
                },
            ],
            outputs: vec![
                frame_output("image"),
                frame_output("coverage"),
                frame_output("weight"),
            ],
            events: vec![],
            required_contexts: vec![],
            lambda: FuncLambda::new(move |_, _, _, inputs, _, outputs| {
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
                    let detection = inputs[2]
                        .value
                        .as_enum()
                        .and_then(|s| DetectionPreset::from_str(s).ok())
                        .unwrap_or(DetectionPreset::WideField)
                        .config();
                    let registration = inputs[3]
                        .value
                        .as_enum()
                        .and_then(|s| RegistrationPreset::from_str(s).ok())
                        .unwrap_or(RegistrationPreset::Default)
                        .config();
                    let stack = inputs[4]
                        .value
                        .as_enum()
                        .and_then(|s| CombinePreset::from_str(s).ok())
                        .unwrap_or(CombinePreset::SigmaClipped)
                        .config();
                    let reference = match inputs[5].value.as_i64() {
                        Some(index) if index >= 0 => Reference::Index(index as usize),
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
            }),
        });

        // auto_stretch
        func_lib.add(Func {
            id: "c15248e0-006a-4a4a-9aae-b1fc7886dea1".into(),
            name: "auto_stretch".to_string(),
            description: Some(
                "Auto-stretches a linear frame to a viewable image (display tone curve)"
                    .to_string(),
            ),
            behavior: FuncBehavior::Pure,
            terminal: false,
            category: "astro".to_string(),
            inputs: vec![
                FuncInput {
                    name: "image".to_string(),
                    required: true,
                    data_type: ASTRO_FRAME_DATA_TYPE.clone(),
                    default_value: None,
                    value_options: vec![],
                },
                preset_input("method", &STRETCH_PRESET_DATATYPE),
            ],
            outputs: vec![frame_output("image")],
            events: vec![],
            required_contexts: vec![],
            lambda: FuncLambda::new(move |_, _, _, inputs, _, outputs| {
                Box::pin(async move {
                    assert_eq!(inputs.len(), 2);
                    assert_eq!(outputs.len(), 1);

                    let config = inputs[1]
                        .value
                        .as_enum()
                        .and_then(|s| StretchPreset::from_str(s).ok())
                        .unwrap_or(StretchPreset::AutoAsinh)
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
            }),
            ..Default::default()
        });

        // astro_to_image: bridge an `AstroFrame` into a `lens::Image` so the
        // imaginarium image nodes (brightness/contrast, blend, convert, save…)
        // can consume astro output.
        func_lib.add(Func {
            id: "7a0265e1-9631-45bd-8ecd-1e923b67a58c".into(),
            name: "astro_to_image".to_string(),
            description: Some(
                "Converts an astro frame to an image (for the imaginarium image nodes)".to_string(),
            ),
            behavior: FuncBehavior::Pure,
            terminal: false,
            category: "astro".to_string(),
            inputs: vec![FuncInput {
                name: "frame".to_string(),
                required: true,
                data_type: ASTRO_FRAME_DATA_TYPE.clone(),
                default_value: None,
                value_options: vec![],
            }],
            outputs: vec![FuncOutput {
                name: "image".to_string(),
                data_type: IMAGE_DATA_TYPE.clone(),
            }],
            events: vec![],
            required_contexts: vec![],
            lambda: FuncLambda::new(move |_, _, _, inputs, _, outputs| {
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
            }),
            ..Default::default()
        });

        // --- per-frame processing nodes (AstroFrame → AstroFrame) ---

        // background_extract
        func_lib.add(processing_func(
            "e27c2a02-ec2a-4c6d-afea-60d1276ff8e1",
            "background_extract",
            "Fits and removes a smooth sky-background gradient",
            vec![
                frame_input("image"),
                preset_input("mode", &BACKGROUND_MODE_DATATYPE),
            ],
            FuncLambda::new(move |_, _, _, inputs, _, outputs| {
                Box::pin(async move {
                    let mode = inputs[1]
                        .value
                        .as_enum()
                        .and_then(|s| BackgroundModeKind::from_str(s).ok())
                        .unwrap_or(BackgroundModeKind::Subtract)
                        .config();
                    let value = inputs[0].value.clone();
                    outputs[0] = run_frame_op(value, move |img| {
                        extract_background(
                            img,
                            &BackgroundConfig {
                                mode,
                                ..Default::default()
                            },
                        );
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
            vec![frame_input("image"), float_input("strength", 0.85)],
            FuncLambda::new(move |_, _, _, inputs, _, outputs| {
                Box::pin(async move {
                    let strength = inputs[1].value.as_f64().map(|v| v as f32).unwrap_or(0.85);
                    let value = inputs[0].value.clone();
                    outputs[0] = run_frame_op(value, move |img| {
                        denoise(
                            img,
                            DenoiseConfig {
                                strength,
                                ..Default::default()
                            },
                        );
                    })
                    .await?;
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
                        .unwrap_or(ScnrKind::AverageNeutral)
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
            vec![frame_input("image"), float_input("amount", 0.5)],
            FuncLambda::new(move |_, _, _, inputs, _, outputs| {
                Box::pin(async move {
                    let amount = inputs[1].value.as_f64().map(|v| v as f32).unwrap_or(0.5);
                    let value = inputs[0].value.clone();
                    outputs[0] = run_frame_op(value, move |img| {
                        compress_dynamic_range(
                            img,
                            HdrConfig {
                                amount,
                                ..Default::default()
                            },
                        );
                    })
                    .await?;
                    Ok(())
                })
            }),
        ));

        // local_contrast
        func_lib.add(processing_func(
            "6a28b732-2704-454b-8afd-0a91d385458a",
            "local_contrast",
            "Local contrast enhancement (CLAHE)",
            vec![frame_input("image"), float_input("strength", 0.8)],
            FuncLambda::new(move |_, _, _, inputs, _, outputs| {
                Box::pin(async move {
                    let strength = inputs[1].value.as_f64().map(|v| v as f32).unwrap_or(0.8);
                    let value = inputs[0].value.clone();
                    outputs[0] = run_frame_op(value, move |img| {
                        enhance_local_contrast(
                            img,
                            LocalContrastConfig {
                                strength,
                                ..Default::default()
                            },
                        );
                    })
                    .await?;
                    Ok(())
                })
            }),
        ));

        // star_detect → star count
        func_lib.add(Func {
            id: "eb93559d-370c-4bea-aef0-c43897f3416a".into(),
            name: "star_detect".to_string(),
            description: Some("Detects stars and outputs the count".to_string()),
            behavior: FuncBehavior::Pure,
            terminal: false,
            category: "astro".to_string(),
            inputs: vec![
                frame_input("image"),
                preset_input("detection", &DETECTION_PRESET_DATATYPE),
            ],
            outputs: vec![FuncOutput {
                name: "count".to_string(),
                data_type: DataType::Int,
            }],
            events: vec![],
            required_contexts: vec![],
            lambda: FuncLambda::new(move |_, _, _, inputs, _, outputs| {
                Box::pin(async move {
                    assert_eq!(inputs.len(), 2);
                    assert_eq!(outputs.len(), 1);

                    let config = inputs[1]
                        .value
                        .as_enum()
                        .and_then(|s| DetectionPreset::from_str(s).ok())
                        .unwrap_or(DetectionPreset::WideField)
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
            }),
            ..Default::default()
        });

        Self { func_lib }
    }
}

/// A preset dropdown input seeded to the enum's first variant.
fn preset_input(name: &str, datatype: &DataType) -> FuncInput {
    FuncInput {
        name: name.to_string(),
        required: false,
        data_type: datatype.clone(),
        default_value: datatype.default_value(),
        value_options: vec![],
    }
}

/// An `AstroFrame` output port.
fn frame_output(name: &str) -> FuncOutput {
    FuncOutput {
        name: name.to_string(),
        data_type: ASTRO_FRAME_DATA_TYPE.clone(),
    }
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
    FuncInput {
        name: name.to_string(),
        required: false,
        data_type: ASTRO_DIR_DATA_TYPE.clone(),
        default_value: None,
        value_options: vec![],
    }
}

/// A required `AstroFrame` input port.
fn frame_input(name: &str) -> FuncInput {
    FuncInput {
        name: name.to_string(),
        required: true,
        data_type: ASTRO_FRAME_DATA_TYPE.clone(),
        default_value: None,
        value_options: vec![],
    }
}

/// An optional float parameter input seeded with `default`.
fn float_input(name: &str, default: f32) -> FuncInput {
    FuncInput {
        name: name.to_string(),
        required: false,
        data_type: DataType::Float,
        default_value: Some((default as f64).into()),
        value_options: vec![],
    }
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
    Func {
        id: id.into(),
        name: name.to_string(),
        description: Some(description.to_string()),
        behavior: FuncBehavior::Pure,
        terminal: false,
        category: "astro".to_string(),
        inputs,
        outputs: vec![frame_output("image")],
        events: vec![],
        required_contexts: vec![],
        lambda,
        ..Default::default()
    }
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

#[cfg(test)]
mod tests {
    use scenarium::data::StaticValue;

    use super::*;

    fn func<'a>(lib: &'a AstroFuncLib, name: &str) -> &'a Func {
        lib.func_lib()
            .funcs
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
        let lib = AstroFuncLib::default();
        let f = func(&lib, "load_astro_image");
        assert_eq!(f.category, "astro");
        assert_eq!(f.inputs.len(), 1);
        assert_eq!(f.outputs.len(), 1);
        assert_eq!(f.inputs[0].data_type, *ASTRO_IMAGE_PATH_DATA_TYPE);
        assert_eq!(f.outputs[0].data_type, *ASTRO_FRAME_DATA_TYPE);
    }

    #[test]
    fn build_masters_node_is_registered() {
        let lib = AstroFuncLib::default();
        let f = func(&lib, "build_masters");
        assert_eq!(f.category, "astro");
        assert_eq!(f.outputs.len(), 1);
        assert_eq!(f.outputs[0].data_type, *MASTERS_DATA_TYPE);

        // Four optional calibration-frame folders, then sigma.
        assert_eq!(f.inputs.len(), 5);
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
    }

    #[test]
    fn stack_lights_node_is_registered() {
        let lib = AstroFuncLib::default();
        let f = func(&lib, "stack_lights");
        assert_eq!(f.category, "astro");

        assert_eq!(f.inputs.len(), 6);
        assert_eq!(f.inputs[0].name, "lights");
        assert_eq!(f.inputs[0].data_type, *ASTRO_DIR_DATA_TYPE);
        assert!(f.inputs[0].required, "lights folder is required");
        assert_eq!(f.inputs[1].name, "masters");
        assert_eq!(f.inputs[1].data_type, *MASTERS_DATA_TYPE);
        assert!(!f.inputs[1].required, "masters are optional");
        // Preset dropdowns seed to their first variant.
        assert_eq!(f.inputs[2].data_type, *DETECTION_PRESET_DATATYPE);
        assert_eq!(
            f.inputs[2].default_value,
            Some(StaticValue::Enum("wide_field".to_string())),
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
        let lib = AstroFuncLib::default();
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
        let lib = AstroFuncLib::default();
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
        let lib = AstroFuncLib::default();
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
}
