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
use crate::astro::nodes::io::ASTRO_RAW_PATHS_DATA_TYPE;
use crate::astro::nodes::runtime;
use crate::image::{IMAGE_DATA_TYPE, Image};

#[derive(Debug, thiserror::Error)]
enum LightFramesError {
    #[error("no light frames selected")]
    Empty,
}

fn light_frames(paths: &[String]) -> Result<Vec<PathBuf>, LightFramesError> {
    let frames: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
    if frames.is_empty() {
        return Err(LightFramesError::Empty);
    }
    Ok(frames)
}

pub(crate) fn register(library: &mut Library) {
    library.add(
        Func::new("b02f5c42-7bda-48f6-81dd-81338efbb126", "Stack Lights")
            .description("Calibrates, aligns, and stacks selected light frames into one image.")
            .category("Astro")
            .pure()
            .input(
                FuncInput::required("Lights", ASTRO_RAW_PATHS_DATA_TYPE.clone())
                    .description("Camera-RAW light frames to stack."),
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

                    let light_paths = inputs[0]
                        .value
                        .as_fs_paths()
                        .expect("lights input type is validated at the compile boundary");
                    let lights = light_frames(light_paths).map_err(InvokeError::external)?;

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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::astro::nodes::stacking::light_frames;

    #[test]
    fn light_frame_selection_requires_input_and_preserves_order() {
        let selected = ["lights/b.raf".to_string(), "lights/a.raf".to_string()];
        assert_eq!(
            light_frames(&selected).unwrap(),
            [PathBuf::from("lights/b.raf"), PathBuf::from("lights/a.raf")]
        );
        assert_eq!(
            light_frames(&[]).unwrap_err().to_string(),
            "no light frames selected"
        );
    }
}
