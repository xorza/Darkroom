//! Light-frame calibration, registration, and stacking node.

use lumos::ProgressCallback;
use lumos::{
    AlignStackConfig, CalibrationMasters, LinearImage, QualityPlanes, Reference,
    calibrate_align_stack,
};
use scenarium::async_lambda;
use scenarium::{
    DataType, DynamicValue, Func, FuncId, FuncInput, FuncOutput, Invocation, InvokeError,
    InvokeResult, Library, OutputDemand,
};

use crate::astro::config::preset::Preset;
use crate::astro::config::stacking::{CombineMethodChoice, DetectionPreset, RegistrationPreset};
use crate::astro::masters::{MASTERS_DATA_TYPE, Masters};
use crate::astro::nodes::io::ASTRO_RAW_PATHS_DATA_TYPE;
use crate::astro::nodes::runtime;
use crate::image::{IMAGE_DATA_TYPE, Image};

const STACK_LIGHTS_FUNC_ID: FuncId = FuncId::literal("b02f5c42-7bda-48f6-81dd-81338efbb126");

/// The input the reference frame index arrives on.
const REFERENCE: usize = 8;

/// The ancillary planes a run computes: coverage and weight for a reader of
/// their outputs, and never the variance or the dispersion, which the node
/// does not output.
const fn quality(demand: &[OutputDemand]) -> QualityPlanes {
    QualityPlanes {
        coverage: !demand[1].is_skip(),
        weight: !demand[2].is_skip(),
        variance: false,
        dispersion: false,
    }
}

/// The alignment reference: the frame an index names, or, unset, the
/// richest frame. A negative index names no frame.
fn reference(index: &DynamicValue) -> InvokeResult<Reference> {
    if matches!(index, DynamicValue::Unbound) {
        return Ok(Reference::Auto);
    }
    let index = index.required_i64();
    usize::try_from(index)
        .map(Reference::Index)
        .map_err(|_negative| InvokeError::invalid_input(REFERENCE, "a frame index", index))
}

pub(crate) fn register(library: &mut Library) {
    DetectionPreset::register(library);
    RegistrationPreset::register(library);
    CombineMethodChoice::register(library);
    library.add(
        Func::new(
            STACK_LIGHTS_FUNC_ID,
            "Stack Lights",
            async_lambda!(move |Invocation { ctx, inputs, demand, outputs, .. }| {
                cancel = ctx.cancel_flag(),
                quality = quality(demand),
            } => {
                debug_assert_eq!(inputs.len(), 9);
                debug_assert_eq!(outputs.len(), 3);

                let lights = inputs[0].required_fs_paths().to_vec();
                let masters_value = inputs[1].clone();
                let mut stack = CombineMethodChoice::resolve(&inputs[6], &inputs[7]);
                stack.quality = quality;
                let reference = reference(&inputs[REFERENCE])?;
                let config = AlignStackConfig {
                    detection: DetectionPreset::resolve(&inputs[2], &inputs[3]),
                    registration: RegistrationPreset::resolve(&inputs[4], &inputs[5]),
                    stack,
                    reference,
                    cosmic_ray: None,
                };

                let result = runtime::run_cancellable(cancel, move |cancel| {
                    let empty = CalibrationMasters::default();
                    let masters = masters_value
                        .as_custom::<Masters>()
                        .map_or(&empty, |masters| &masters.masters);
                    calibrate_align_stack(
                        &lights,
                        masters,
                        &config,
                        ProgressCallback::default(),
                        cancel,
                    )
                })
                .await?;

                let product = result.product;
                outputs[0] = DynamicValue::from_custom(Image::from(product.image));
                if quality.coverage {
                    let coverage = product
                        .coverage
                        .expect("the stack produces the coverage it was asked for");
                    outputs[1] =
                        DynamicValue::from_custom(Image::from(LinearImage::from(coverage)));
                }
                if quality.weight {
                    let weight = product
                        .weight
                        .expect("the stack produces the weight it was asked for");
                    outputs[2] =
                        DynamicValue::from_custom(Image::from(LinearImage::from(weight)));
                }
                Ok(())
            }),
        )
        .description("Calibrates, aligns, and stacks selected light frames into one image.")
        .category("Astro")
        .pure()
        .input(
            FuncInput::required("Lights", ASTRO_RAW_PATHS_DATA_TYPE.clone())
                .description("Camera-RAW light frames to stack."),
        )
        .input(
            FuncInput::optional("Masters", MASTERS_DATA_TYPE)
                .description("Optional calibration masters. Unwired means no calibration."),
        )
        .input(DetectionPreset::picker("Detection"))
        .input(DetectionPreset::config_input("Detection Config", 2))
        .input(RegistrationPreset::picker("Registration"))
        .input(RegistrationPreset::config_input("Registration Config", 4))
        .input(CombineMethodChoice::picker("Combine"))
        .input(CombineMethodChoice::config_input("Combine Config", 6))
        .input(
            FuncInput::optional("Reference", DataType::Int)
                .description("Alignment reference frame index; unset picks the richest frame."),
        )
        .output(FuncOutput::new("Image", IMAGE_DATA_TYPE).description("Stacked image."))
        .output(
            FuncOutput::new("Coverage", IMAGE_DATA_TYPE).description("Per-pixel frame-count map."),
        )
        .output(
            FuncOutput::new("Weight", IMAGE_DATA_TYPE)
                .description("Per-pixel accumulated weight map."),
        ),
    );
}

#[cfg(test)]
mod tests {
    use lumos::{QualityPlanes, Reference};
    use scenarium::{DynamicValue, OutputDemand};

    use crate::astro::nodes::stacking::{quality, reference};

    /// Each ancillary plane is computed exactly when its output has a reader.
    #[test]
    fn the_stack_computes_the_planes_its_readers_demand() {
        use OutputDemand::{Produce, Skip};
        let planes = |demand: [OutputDemand; 3]| {
            let QualityPlanes {
                coverage,
                weight,
                variance,
                dispersion,
            } = quality(&demand);
            [coverage, weight, variance || dispersion]
        };
        assert_eq!(planes([Produce, Skip, Skip]), [false, false, false]);
        assert_eq!(planes([Produce, Produce, Skip]), [true, false, false]);
        assert_eq!(planes([Skip, Skip, Produce]), [false, true, false]);
        assert_eq!(planes([Produce, Produce, Produce]), [true, true, false]);
    }

    #[test]
    fn an_unset_reference_is_automatic_and_a_negative_one_is_refused() {
        assert!(matches!(
            reference(&DynamicValue::Unbound),
            Ok(Reference::Auto)
        ));
        assert!(matches!(
            reference(&DynamicValue::from(3_i64)),
            Ok(Reference::Index(3))
        ));
        assert_eq!(
            reference(&DynamicValue::from(-1_i64))
                .unwrap_err()
                .to_string(),
            "input 8 must be a frame index, got -1"
        );
    }
}
