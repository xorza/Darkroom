//! Per-frame astronomical processing nodes.

use lumos::{Denoise, Hdr, LocalContrast, NeutralizeBackground};
use scenarium::{DataType, Func, FuncInput, FuncLambda, FuncOutput, Library};

use crate::astro::config::preset;
use crate::astro::config::processing::{
    BackgroundConfigDef, BackgroundModeKind, DenoiseConfigDef, HdrConfigDef,
    LocalContrastConfigDef, ScnrConfigDef, ScnrKind, StretchConfigDef, StretchPreset,
};
use crate::astro::nodes::runtime;
use crate::config_node::{ConfigValue, NodeConfig, config_data_type};
use crate::image::IMAGE_DATA_TYPE;

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
        Func::new("c15248e0-006a-4a4a-9aae-b1fc7886dea1", "Auto Stretch")
            .description("Auto-stretches a linear frame to a viewable image (display tone curve).")
            .category("Astro")
            .pure()
            .input(frame_input("Image"))
            .input(preset::input::<StretchConfigDef, StretchPreset>("Method"))
            .output(
                FuncOutput::new("Image", IMAGE_DATA_TYPE.clone())
                    .description("Stretched, display-ready image."),
            )
            .lambda(FuncLambda::new(move |_, _, _, inputs, _, outputs| {
                Box::pin(async move {
                    debug_assert_eq!(inputs.len(), 2);
                    debug_assert_eq!(outputs.len(), 1);
                    let config =
                        preset::resolve::<StretchConfigDef, StretchPreset>(&inputs[1].value);
                    let value = std::mem::take(&mut inputs[0].value);
                    outputs[0] =
                        runtime::run_frame_op(value, move |image| config.apply(image)).await?;
                    Ok(())
                })
            })),
    );
}

fn register_background(library: &mut Library) {
    library.add(processing_func(
        "e27c2a02-ec2a-4c6d-afea-60d1276ff8e1",
        "Extract Background",
        "Fits and removes a smooth sky-background gradient.",
        vec![
            frame_input("Image"),
            preset::input::<BackgroundConfigDef, BackgroundModeKind>("Config"),
        ],
        FuncLambda::new(move |_, _, _, inputs, _, outputs| {
            Box::pin(async move {
                let config =
                    preset::resolve::<BackgroundConfigDef, BackgroundModeKind>(&inputs[1].value);
                let value = std::mem::take(&mut inputs[0].value);
                outputs[0] = runtime::run_frame_op(value, move |image| config.apply(image)).await?;
                Ok(())
            })
        }),
    ));
}

fn register_denoise(library: &mut Library) {
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
                    .map(|config| config.0.clone().into())
                    .unwrap_or_else(|| Denoise {
                        strength: inputs[1]
                            .value
                            .as_f64()
                            .map(|value| value as f32)
                            .expect("strength input type is validated at the compile boundary"),
                        ..Default::default()
                    });
                let value = std::mem::take(&mut inputs[0].value);
                outputs[0] = runtime::run_frame_op(value, move |image| config.apply(image)).await?;
                Ok(())
            })
        }),
    ));
}

fn register_scnr(library: &mut Library) {
    library.add(processing_func(
        "ef0c2661-8553-4302-9251-95b2d383af19",
        "SCNR",
        "Removes the residual green cast (SCNR).",
        vec![
            frame_input("Image"),
            preset::input::<ScnrConfigDef, ScnrKind>("Method"),
        ],
        FuncLambda::new(move |_, _, _, inputs, _, outputs| {
            Box::pin(async move {
                let method = preset::resolve::<ScnrConfigDef, ScnrKind>(&inputs[1].value);
                let value = std::mem::take(&mut inputs[0].value);
                outputs[0] = runtime::run_frame_op(value, move |image| method.apply(image)).await?;
                Ok(())
            })
        }),
    ));
}

fn register_neutralize(library: &mut Library) {
    library.add(processing_func(
        "5a8c9043-61ca-4a5a-8e55-ce27c804e84b",
        "Neutralize Background",
        "Shifts each channel so the background reads neutral gray.",
        vec![frame_input("Image")],
        FuncLambda::new(move |_, _, _, inputs, _, outputs| {
            Box::pin(async move {
                let value = std::mem::take(&mut inputs[0].value);
                outputs[0] =
                    runtime::run_frame_op(value, |image| NeutralizeBackground.apply(image)).await?;
                Ok(())
            })
        }),
    ));
}

fn register_hdr(library: &mut Library) {
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
                    .map(|config| config.0.clone().into())
                    .unwrap_or_else(|| Hdr {
                        amount: inputs[1]
                            .value
                            .as_f64()
                            .map(|value| value as f32)
                            .expect("amount input type is validated at the compile boundary"),
                        ..Default::default()
                    });
                let value = std::mem::take(&mut inputs[0].value);
                outputs[0] = runtime::run_frame_op(value, move |image| config.apply(image)).await?;
                Ok(())
            })
        }),
    ));
}

fn register_local_contrast(library: &mut Library) {
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
                    .map(|config| config.0.clone().into())
                    .unwrap_or_else(|| LocalContrast {
                        strength: inputs[1]
                            .value
                            .as_f64()
                            .map(|value| value as f32)
                            .expect("strength input type is validated at the compile boundary"),
                        ..Default::default()
                    });
                let value = std::mem::take(&mut inputs[0].value);
                outputs[0] = runtime::run_frame_op(value, move |image| config.apply(image)).await?;
                Ok(())
            })
        }),
    ));
}

fn config_override_input<T: NodeConfig>() -> FuncInput {
    FuncInput::optional("Config", config_data_type::<T>())
        .description("Optional detailed config; overrides the inline knob when wired.")
}

fn frame_input(name: &str) -> FuncInput {
    FuncInput::required(name, IMAGE_DATA_TYPE.clone()).description("Image to process.")
}

fn float_input(name: &str, default: f32, description: &str) -> FuncInput {
    FuncInput::required(name, DataType::Float)
        .description(description)
        .default(default as f64)
}

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
