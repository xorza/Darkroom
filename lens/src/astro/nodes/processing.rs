//! Per-frame astronomical processing nodes.

use lumos::{Denoise, ExtractBackground, Hdr, LocalContrast, NeutralizeBackground};
use scenarium::FuncId;
use scenarium::{DataType, Func, FuncInput, FuncLambda, FuncOutput, Library};

use crate::astro::config::preset;
use crate::astro::config::processing::{
    BackgroundModeKind, ScnrKind, ScnrKnobs, StretchKnobs, StretchPreset,
};
use crate::astro::nodes::runtime;
use common::Introspect;

use crate::config_node::{ConfigValue, config_data_type};
use crate::image::IMAGE_DATA_TYPE;
use scenarium::Invocation;
use std::mem;

const AUTO_STRETCH_FUNC_ID: FuncId = FuncId::literal("c15248e0-006a-4a4a-9aae-b1fc7886dea1");
const EXTRACT_BACKGROUND_FUNC_ID: FuncId = FuncId::literal("e27c2a02-ec2a-4c6d-afea-60d1276ff8e1");
const DENOISE_FUNC_ID: FuncId = FuncId::literal("61c17dfa-8369-446b-b6e7-d91d62d344ee");
const SCNR_FUNC_ID: FuncId = FuncId::literal("ef0c2661-8553-4302-9251-95b2d383af19");
const NEUTRALIZE_BACKGROUND_FUNC_ID: FuncId =
    FuncId::literal("5a8c9043-61ca-4a5a-8e55-ce27c804e84b");
const HDR_COMPRESSION_FUNC_ID: FuncId = FuncId::literal("300a2ec5-0ccd-47ec-b282-030eea41441c");
const LOCAL_CONTRAST_FUNC_ID: FuncId = FuncId::literal("6a28b732-2704-454b-8afd-0a91d385458a");

pub(crate) fn register(library: &mut Library) {
    register_stretch(library);
    register_background(library);
    register_denoise(library);
    register_scnr(library);
    register_neutralize(library);
    register_hdr(library);
    register_local_contrast(library);
}

fn register_stretch(library: &mut Library) {
    library.add(
        Func::new(AUTO_STRETCH_FUNC_ID, "Auto Stretch")
            .description("Auto-stretches a linear frame to a viewable image (display tone curve).")
            .category("Astro")
            .pure()
            .input(frame_input("Image"))
            .input(preset::input::<StretchKnobs, StretchPreset>("Method"))
            .output(
                FuncOutput::new("Image", IMAGE_DATA_TYPE.clone())
                    .description("Stretched, display-ready image."),
            )
            .lambda(FuncLambda::new(
                move |Invocation {
                          inputs, outputs, ..
                      }| {
                    Box::pin(async move {
                        debug_assert_eq!(inputs.len(), 2);
                        debug_assert_eq!(outputs.len(), 1);
                        let config = preset::resolve::<StretchKnobs, StretchPreset>(&inputs[1]);
                        let value = mem::take(&mut inputs[0]);
                        outputs[0] =
                            runtime::run_frame_op(value, move |image| config.apply(image)).await?;
                        Ok(())
                    })
                },
            )),
    );
}

fn register_background(library: &mut Library) {
    library.add(processing_func(
        EXTRACT_BACKGROUND_FUNC_ID,
        "Extract Background",
        "Fits and removes a smooth sky-background gradient.",
        vec![
            frame_input("Image"),
            preset::input::<ExtractBackground, BackgroundModeKind>("Config"),
        ],
        FuncLambda::new(
            move |Invocation {
                      inputs, outputs, ..
                  }| {
                Box::pin(async move {
                    let config =
                        preset::resolve::<ExtractBackground, BackgroundModeKind>(&inputs[1]);
                    let value = mem::take(&mut inputs[0]);
                    outputs[0] =
                        runtime::run_frame_op(value, move |image| config.apply(image)).await?;
                    Ok(())
                })
            },
        ),
    ));
}

fn register_denoise(library: &mut Library) {
    library.add(processing_func(
        DENOISE_FUNC_ID,
        "Denoise",
        "Wavelet denoise (starlet coefficient thresholding).",
        vec![
            frame_input("Image"),
            float_input("Strength", 0.85, "Denoise strength in [0, 1]."),
            config_override_input::<Denoise>(),
        ],
        FuncLambda::new(
            move |Invocation {
                      inputs, outputs, ..
                  }| {
                Box::pin(async move {
                    let config = inputs[2].as_custom::<ConfigValue<Denoise>>().map_or_else(
                        || Denoise {
                            strength: inputs[1]
                                .as_f64()
                                .map(|value| value as f32)
                                .expect("strength input type is validated at the compile boundary"),
                            ..Default::default()
                        },
                        |config| config.0,
                    );
                    let value = mem::take(&mut inputs[0]);
                    outputs[0] =
                        runtime::run_frame_op(value, move |image| config.apply(image)).await?;
                    Ok(())
                })
            },
        ),
    ));
}

fn register_scnr(library: &mut Library) {
    library.add(processing_func(
        SCNR_FUNC_ID,
        "SCNR",
        "Removes the residual green cast (SCNR).",
        vec![
            frame_input("Image"),
            preset::input::<ScnrKnobs, ScnrKind>("Method"),
        ],
        FuncLambda::new(
            move |Invocation {
                      inputs, outputs, ..
                  }| {
                Box::pin(async move {
                    let method = preset::resolve::<ScnrKnobs, ScnrKind>(&inputs[1]);
                    let value = mem::take(&mut inputs[0]);
                    outputs[0] =
                        runtime::run_frame_op(value, move |image| method.apply(image)).await?;
                    Ok(())
                })
            },
        ),
    ));
}

fn register_neutralize(library: &mut Library) {
    library.add(processing_func(
        NEUTRALIZE_BACKGROUND_FUNC_ID,
        "Neutralize Background",
        "Shifts each channel so the background reads neutral gray.",
        vec![frame_input("Image")],
        FuncLambda::new(
            move |Invocation {
                      inputs, outputs, ..
                  }| {
                Box::pin(async move {
                    let value = mem::take(&mut inputs[0]);
                    outputs[0] =
                        runtime::run_frame_op(value, |image| NeutralizeBackground.apply(image))
                            .await?;
                    Ok(())
                })
            },
        ),
    ));
}

fn register_hdr(library: &mut Library) {
    library.add(processing_func(
        HDR_COMPRESSION_FUNC_ID,
        "HDR Compression",
        "Compresses large-scale dynamic range (multiscale HDR).",
        vec![
            frame_input("Image"),
            float_input("Amount", 0.5, "Compression amount in [0, 1]."),
            config_override_input::<Hdr>(),
        ],
        FuncLambda::new(
            move |Invocation {
                      inputs, outputs, ..
                  }| {
                Box::pin(async move {
                    let config = inputs[2].as_custom::<ConfigValue<Hdr>>().map_or_else(
                        || Hdr {
                            amount: inputs[1]
                                .as_f64()
                                .map(|value| value as f32)
                                .expect("amount input type is validated at the compile boundary"),
                            ..Default::default()
                        },
                        |config| config.0,
                    );
                    let value = mem::take(&mut inputs[0]);
                    outputs[0] =
                        runtime::run_frame_op(value, move |image| config.apply(image)).await?;
                    Ok(())
                })
            },
        ),
    ));
}

fn register_local_contrast(library: &mut Library) {
    library.add(processing_func(
        LOCAL_CONTRAST_FUNC_ID,
        "Local Contrast",
        "Local contrast enhancement (CLAHE).",
        vec![
            frame_input("Image"),
            float_input("Strength", 0.8, "Local-contrast strength in [0, 1]."),
            config_override_input::<LocalContrast>(),
        ],
        FuncLambda::new(
            move |Invocation {
                      inputs, outputs, ..
                  }| {
                Box::pin(async move {
                    let config = inputs[2]
                        .as_custom::<ConfigValue<LocalContrast>>()
                        .map_or_else(
                            || LocalContrast {
                                strength: inputs[1].as_f64().map(|value| value as f32).expect(
                                    "strength input type is validated at the compile boundary",
                                ),
                                ..Default::default()
                            },
                            |config| config.0,
                        );
                    let value = mem::take(&mut inputs[0]);
                    outputs[0] =
                        runtime::run_frame_op(value, move |image| config.apply(image)).await?;
                    Ok(())
                })
            },
        ),
    ));
}

fn config_override_input<T: Introspect>() -> FuncInput {
    FuncInput::optional("Config", config_data_type::<T>())
        .description("Optional detailed config; overrides the inline knob when wired.")
}

fn frame_input(name: &str) -> FuncInput {
    FuncInput::required(name, IMAGE_DATA_TYPE.clone()).description("Image to process.")
}

fn float_input(name: &str, default: f32, description: &str) -> FuncInput {
    FuncInput::required(name, DataType::Float)
        .description(description)
        .default(f64::from(default))
}

fn processing_func(
    id: FuncId,
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
