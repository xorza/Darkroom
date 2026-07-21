//! ONNX-backed denoise and star-removal nodes.

use std::path::PathBuf;
use std::sync::Arc;

use lumos::{TiledOnnxConfig, ml_denoise, remove_stars, remove_stars_starless_only};
use scenarium::{DataType, DynamicValue, FsPathConfig, FsPathMode, StaticValue};
use scenarium::{Func, FuncId, FuncInput, FuncLambda, FuncOutput, Library};

use crate::astro::nodes::MlModelPaths;
use crate::astro::nodes::runtime;
use crate::image::{IMAGE_DATA_TYPE, Image};

const DENOISE_FUNC_ID: FuncId = FuncId::from_u128(0xace786f98a024ed193a0ad67bf0680f8);
const STAR_REMOVAL_FUNC_ID: FuncId = FuncId::from_u128(0x60c31a76eed4467c9ba35c89d294a91b);

pub(crate) fn register(library: &mut Library, model_paths: &MlModelPaths) {
    register_denoise(library, &model_paths.denoise);
    register_star_removal(library, &model_paths.star_removal);
}

pub(crate) fn replace(library: &mut Library, model_paths: &MlModelPaths) {
    library
        .remove(&DENOISE_FUNC_ID)
        .expect("ML denoise function is registered");
    library
        .remove(&STAR_REMOVAL_FUNC_ID)
        .expect("ML star-removal function is registered");
    register(library, model_paths);
}

fn register_denoise(library: &mut Library, model_path: &std::path::Path) {
    library.add(
        Func::new(DENOISE_FUNC_ID, "ML Denoise")
            .description("Denoises a stretched image with an ONNX model (DeepSNR).")
            .category("Astro")
            .pure()
            .input(frame_input())
            .input(model_input("Model", model_path))
            .output(
                FuncOutput::new("Image", IMAGE_DATA_TYPE.clone()).description("Processed image."),
            )
            .lambda(FuncLambda::new(move |_, _, _, inputs, _, outputs| {
                Box::pin(async move {
                    debug_assert_eq!(inputs.len(), 2);
                    debug_assert_eq!(outputs.len(), 1);
                    let model = PathBuf::from(
                        inputs[1]
                            .value
                            .as_fs_path()
                            .expect("model input type is validated at the compile boundary"),
                    );
                    let output =
                        runtime::run_ml(std::mem::take(&mut inputs[0].value), move |image| {
                            ml_denoise(&image, &TiledOnnxConfig::new(model))
                        })
                        .await?;
                    outputs[0] = DynamicValue::from_custom(Image::from(output));
                    Ok(())
                })
            })),
    );
}

fn register_star_removal(library: &mut Library, model_path: &std::path::Path) {
    library.add(
        Func::new(STAR_REMOVAL_FUNC_ID, "ML Star Removal")
            .description("Removes stars with a StarNet ONNX model (starless + stars).")
            .category("Astro")
            .pure()
            .input(frame_input())
            .input(model_input("Model", model_path))
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
                    let need_stars = !output_demand[1].is_skip();
                    Box::pin(async move {
                        debug_assert_eq!(inputs.len(), 2);
                        debug_assert_eq!(outputs.len(), 2);
                        let model = PathBuf::from(
                            inputs[1]
                                .value
                                .as_fs_path()
                                .expect("model input type is validated at the compile boundary"),
                        );
                        if need_stars {
                            let result = runtime::run_ml(
                                std::mem::take(&mut inputs[0].value),
                                move |image| remove_stars(image, &TiledOnnxConfig::new(model)),
                            )
                            .await?;
                            outputs[0] = DynamicValue::from_custom(Image::from(result.starless));
                            outputs[1] = DynamicValue::from_custom(Image::from(result.stars));
                        } else {
                            let starless = runtime::run_ml(
                                std::mem::take(&mut inputs[0].value),
                                move |image| {
                                    remove_stars_starless_only(&image, &TiledOnnxConfig::new(model))
                                },
                            )
                            .await?;
                            outputs[0] = DynamicValue::from_custom(Image::from(starless));
                        }
                        Ok(())
                    })
                },
            )),
    );
}

fn frame_input() -> FuncInput {
    FuncInput::required("Image", IMAGE_DATA_TYPE.clone()).description("Image to process.")
}

fn model_input(name: &str, default: &std::path::Path) -> FuncInput {
    FuncInput::required(
        name,
        DataType::FsPath(Arc::new(FsPathConfig::with_extensions(
            FsPathMode::ExistingFile,
            vec!["onnx".to_string()],
        ))),
    )
    .description("ONNX model file. Its file identity participates in the node cache key.")
    .default(StaticValue::FsPath(default.display().to_string()))
}
