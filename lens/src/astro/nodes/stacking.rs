//! Light-frame calibration, registration, and stacking node.

use std::path::PathBuf;

use imaginarium::Image as RawImage;
use lumos::{AlignStackConfig, CalibrationMasters, LinearImage, Reference, calibrate_align_stack};
use scenarium::{
    DataType, DynamicValue, Func, FuncInput, FuncLambda, FuncOutput, InvokeError, Library,
};

use crate::astro::config::preset;
use crate::astro::config::stacking::{
    CombineConfigDef, CombinePreset, DetectionConfigDef, DetectionPreset, RegistrationConfigDef,
    RegistrationPreset,
};
use crate::astro::masters::{MASTERS_DATA_TYPE, Masters};
use crate::astro::nodes::io::{self as astro_io, ASTRO_DIR_DATA_TYPE, RawFrameScanError};
use crate::astro::nodes::runtime;
use crate::image::{IMAGE_DATA_TYPE, Image};

#[derive(Debug, thiserror::Error)]
enum LightFramesError {
    #[error(transparent)]
    Scan(#[from] RawFrameScanError),
    #[error("no camera-RAW frames found in light-frame folder '{dir}'", dir = .dir.display())]
    Empty { dir: PathBuf },
}

fn light_frames(dir: PathBuf) -> Result<Vec<PathBuf>, LightFramesError> {
    let frames = astro_io::raw_frame_files(&dir)?;
    if frames.is_empty() {
        return Err(LightFramesError::Empty { dir });
    }
    Ok(frames)
}

pub(crate) fn register(library: &mut Library) {
    library.add(
        Func::new("b02f5c42-7bda-48f6-81dd-81338efbb126", "Stack Lights")
            .description("Calibrates, aligns, and stacks a folder of light frames into one image.")
            .category("Astro")
            .pure()
            .input(
                FuncInput::required("Lights", ASTRO_DIR_DATA_TYPE.clone())
                    .description("Folder of light frames to stack."),
            )
            .input(
                FuncInput::optional("Masters", MASTERS_DATA_TYPE.clone())
                    .description("Optional calibration masters. Unwired means no calibration."),
            )
            .input(preset::input::<DetectionConfigDef, DetectionPreset>(
                "Detection",
            ))
            .input(preset::input::<RegistrationConfigDef, RegistrationPreset>(
                "Registration",
            ))
            .input(preset::input::<CombineConfigDef, CombinePreset>("Combine"))
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
                let cancel = ctx.cancel_flag();
                Box::pin(async move {
                    debug_assert_eq!(inputs.len(), 6);
                    debug_assert_eq!(outputs.len(), 3);

                    let lights_dir = inputs[0]
                        .value
                        .as_fs_path()
                        .map(PathBuf::from)
                        .expect("lights input type is validated at the compile boundary");
                    let lights = light_frames(lights_dir).map_err(InvokeError::external)?;

                    let masters_value = inputs[1].value.clone();
                    let detection =
                        preset::resolve::<DetectionConfigDef, DetectionPreset>(&inputs[2].value);
                    let registration = preset::resolve::<RegistrationConfigDef, RegistrationPreset>(
                        &inputs[3].value,
                    );
                    let stack =
                        preset::resolve::<CombineConfigDef, CombinePreset>(&inputs[4].value);
                    let reference = match inputs[5]
                        .value
                        .as_i64()
                        .expect("reference input type is validated at the compile boundary")
                    {
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

                    let result = runtime::run_cancellable(cancel, move |cancel| {
                        let empty = CalibrationMasters::default();
                        let masters = masters_value
                            .as_custom::<Masters>()
                            .map(|masters| &masters.masters)
                            .unwrap_or(&empty);
                        calibrate_align_stack(&lights, masters, &config, cancel)
                    })
                    .await?;

                    let coverage = LinearImage::from(result.product.coverage);
                    let weight = LinearImage::from(result.product.weight);
                    outputs[0] = DynamicValue::from_custom(Image::from(RawImage::from(
                        &result.product.image,
                    )));
                    outputs[1] = DynamicValue::from_custom(Image::from(RawImage::from(&coverage)));
                    outputs[2] = DynamicValue::from_custom(Image::from(RawImage::from(&weight)));
                    Ok(())
                })
            })),
    );
}
