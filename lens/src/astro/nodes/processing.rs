//! Per-frame astronomical processing nodes.
//!
//! Each takes an image and one quick knob — a preset picker, or for an op
//! tuned by a single strength that strength — beside an optional `Config`
//! port declared to override the knob, so a wired config is what runs and the
//! editor shows the knob set aside.

use std::fmt;
use std::mem;

use common::Introspect;
use lumos::{
    BackgroundMode, Denoise, ExtractBackground, Hdr, LinearImage, LocalContrast,
    NeutralizeBackground, OpError, Scnr, Stretch,
};
use scenarium::{DataType, Func, FuncId, FuncInput, FuncLambda, FuncOutput, Invocation, Library};

use crate::astro::config::preset::Preset;
use crate::astro::config::processing::{ScnrMethodChoice, StretchMethodChoice};
use crate::astro::nodes::runtime;
use crate::config_node::ConfigValue;
use crate::image::{IMAGE_DATA_TYPE, Image};

const AUTO_STRETCH_FUNC_ID: FuncId = FuncId::literal("c15248e0-006a-4a4a-9aae-b1fc7886dea1");
const EXTRACT_BACKGROUND_FUNC_ID: FuncId = FuncId::literal("e27c2a02-ec2a-4c6d-afea-60d1276ff8e1");
const DENOISE_FUNC_ID: FuncId = FuncId::literal("61c17dfa-8369-446b-b6e7-d91d62d344ee");
const SCNR_FUNC_ID: FuncId = FuncId::literal("ef0c2661-8553-4302-9251-95b2d383af19");
const NEUTRALIZE_BACKGROUND_FUNC_ID: FuncId =
    FuncId::literal("5a8c9043-61ca-4a5a-8e55-ce27c804e84b");
const HDR_COMPRESSION_FUNC_ID: FuncId = FuncId::literal("300a2ec5-0ccd-47ec-b282-030eea41441c");
const LOCAL_CONTRAST_FUNC_ID: FuncId = FuncId::literal("6a28b732-2704-454b-8afd-0a91d385458a");

pub(crate) fn register(library: &mut Library) {
    StretchMethodChoice::register(library);
    BackgroundMode::register(library);
    ScnrMethodChoice::register(library);
    library.add(preset_func::<StretchMethodChoice>(
        AUTO_STRETCH_FUNC_ID,
        "Auto Stretch",
        "Auto-stretches a linear frame to a viewable image (display tone curve).",
        "Method",
    ));
    library.add(preset_func::<BackgroundMode>(
        EXTRACT_BACKGROUND_FUNC_ID,
        "Extract Background",
        "Fits and removes a smooth sky-background gradient.",
        "Mode",
    ));
    library.add(preset_func::<ScnrMethodChoice>(
        SCNR_FUNC_ID,
        "SCNR",
        "Removes the residual green cast (SCNR).",
        "Method",
    ));
    library.add(strength_func(
        DENOISE_FUNC_ID,
        "Denoise",
        "Wavelet denoise (starlet coefficient thresholding).",
        Knob {
            name: "Strength",
            description: "Denoise strength in [0, 1].",
            default: Denoise::default().strength,
        },
        |strength| Denoise {
            strength,
            ..Default::default()
        },
    ));
    library.add(strength_func(
        HDR_COMPRESSION_FUNC_ID,
        "HDR Compression",
        "Compresses large-scale dynamic range (multiscale HDR).",
        Knob {
            name: "Amount",
            description: "Compression amount in [0, 1].",
            default: Hdr::default().amount,
        },
        |amount| Hdr {
            amount,
            ..Default::default()
        },
    ));
    library.add(strength_func(
        LOCAL_CONTRAST_FUNC_ID,
        "Local Contrast",
        "Local contrast enhancement (CLAHE).",
        Knob {
            name: "Strength",
            description: "Local-contrast strength in [0, 1].",
            default: LocalContrast::default().strength,
        },
        |strength| LocalContrast {
            strength,
            ..Default::default()
        },
    ));
    library.add(processing_func(
        NEUTRALIZE_BACKGROUND_FUNC_ID,
        "Neutralize Background",
        "Shifts each channel so the background reads neutral gray.",
        vec![Image::input("Image")],
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

/// A `lumos` op that rewrites a frame in place.
trait FrameOp: Send + 'static {
    fn run(&self, image: &mut LinearImage) -> Result<(), OpError>;
}

impl FrameOp for Stretch {
    fn run(&self, image: &mut LinearImage) -> Result<(), OpError> {
        self.apply(image)
    }
}

impl FrameOp for ExtractBackground {
    fn run(&self, image: &mut LinearImage) -> Result<(), OpError> {
        self.apply(image)
    }
}

impl FrameOp for Scnr {
    fn run(&self, image: &mut LinearImage) -> Result<(), OpError> {
        self.apply(image)
    }
}

impl FrameOp for Denoise {
    fn run(&self, image: &mut LinearImage) -> Result<(), OpError> {
        self.apply(image)
    }
}

impl FrameOp for Hdr {
    fn run(&self, image: &mut LinearImage) -> Result<(), OpError> {
        self.apply(image)
    }
}

impl FrameOp for LocalContrast {
    fn run(&self, image: &mut LinearImage) -> Result<(), OpError> {
        self.apply(image)
    }
}

/// A node over `P`'s configs: the image, `P`'s picker named `pick`, and the
/// config overriding it.
fn preset_func<P>(id: FuncId, name: &str, description: &str, pick: &str) -> Func
where
    P: Preset,
    P::Config: FrameOp,
{
    processing_func(
        id,
        name,
        description,
        vec![
            Image::input("Image"),
            P::picker(pick),
            P::config_input("Config", 1),
        ],
        FuncLambda::new(
            move |Invocation {
                      inputs, outputs, ..
                  }| {
                Box::pin(async move {
                    let config = P::resolve(&inputs[1], &inputs[2]);
                    let value = mem::take(&mut inputs[0]);
                    outputs[0] =
                        runtime::run_frame_op(value, move |image| config.run(image)).await?;
                    Ok(())
                })
            },
        ),
    )
}

/// The one strength an op is tuned by, as a node's quick knob.
#[derive(Debug, Clone, Copy)]
struct Knob {
    name: &'static str,
    description: &'static str,
    /// The op's own default, so the knob starts where the op does.
    default: f32,
}

/// A node over a config tuned by one strength: the image, the strength, and
/// the config overriding it; `with` builds the config a strength stands for.
fn strength_func<T>(
    id: FuncId,
    name: &str,
    description: &str,
    knob: Knob,
    with: fn(f32) -> T,
) -> Func
where
    T: FrameOp + Introspect + Clone + fmt::Debug + Sync,
{
    processing_func(
        id,
        name,
        description,
        vec![
            Image::input("Image"),
            FuncInput::required(knob.name, DataType::Float)
                .const_only()
                .description(knob.description)
                .default(f64::from(knob.default)),
            ConfigValue::<T>::input("Config", 1),
        ],
        FuncLambda::new(
            move |Invocation {
                      inputs, outputs, ..
                  }| {
                Box::pin(async move {
                    let config = match inputs[2].as_custom::<ConfigValue<T>>() {
                        Some(config) => config.0.clone(),
                        None => with(inputs[1].required_f64() as f32),
                    };
                    let value = mem::take(&mut inputs[0]);
                    outputs[0] =
                        runtime::run_frame_op(value, move |image| config.run(image)).await?;
                    Ok(())
                })
            },
        ),
    )
}

fn processing_func(
    id: FuncId,
    name: &str,
    description: &str,
    inputs: Vec<FuncInput>,
    lambda: FuncLambda,
) -> Func {
    Func::new(id, name, lambda)
        .category("Astro")
        .description(description)
        .pure()
        .inputs(inputs)
        .output(FuncOutput::new("Image", IMAGE_DATA_TYPE).description("Processed image."))
}
