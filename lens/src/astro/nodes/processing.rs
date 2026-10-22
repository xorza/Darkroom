//! Per-frame astronomical processing nodes.
//!
//! Each takes an image and its config: one input that holds a preset or a
//! wired detailed config, or for an op with a few plain knobs those knobs as
//! inputs of their own.

use std::{fmt, mem};

use common::Introspect;
use lumos::{
    BgMode, Denoise, ExtractBg, Hdr, LinearImage, LocalContrast, NeutralizeBg, OpError, Scnr,
    ScnrProtection, Stretch,
};
use scenarium::{
    Func, FuncId, FuncInput, FuncLambda, FuncOutput, Invocation, Library, async_lambda,
};

use crate::astro::config::preset::Preset;
use crate::astro::config::processing::StretchMethodChoice;
use crate::astro::nodes::runtime;
use crate::config_node::FieldInputs;
use crate::image::{IMAGE_DATA_TYPE, Image};
use crate::node_spec::NodeSpec;

const AUTO_STRETCH_FUNC_ID: FuncId = FuncId::literal("c15248e0-006a-4a4a-9aae-b1fc7886dea1");
const EXTRACT_BG_FUNC_ID: FuncId = FuncId::literal("e27c2a02-ec2a-4c6d-afea-60d1276ff8e1");
const DENOISE_FUNC_ID: FuncId = FuncId::literal("61c17dfa-8369-446b-b6e7-d91d62d344ee");
const SCNR_FUNC_ID: FuncId = FuncId::literal("ef0c2661-8553-4302-9251-95b2d383af19");
const NEUTRALIZE_BG_FUNC_ID: FuncId = FuncId::literal("5a8c9043-61ca-4a5a-8e55-ce27c804e84b");
const HDR_COMPRESSION_FUNC_ID: FuncId = FuncId::literal("300a2ec5-0ccd-47ec-b282-030eea41441c");
const LOCAL_CONTRAST_FUNC_ID: FuncId = FuncId::literal("6a28b732-2704-454b-8afd-0a91d385458a");

pub(crate) fn register(library: &mut Library) {
    library.add(preset_func::<StretchMethodChoice>(
        NodeSpec {
            id: AUTO_STRETCH_FUNC_ID,
            name: "Auto Stretch",
            description: "Auto-stretches a linear frame to a viewable image (display tone curve).",
        },
        "Method",
    ));
    library.add(preset_func::<BgMode>(
        NodeSpec {
            id: EXTRACT_BG_FUNC_ID,
            name: "Extract Background",
            description: "Fits and removes a smooth sky-background gradient.",
        },
        "Mode",
    ));
    library.add(preset_func::<ScnrProtection>(
        NodeSpec {
            id: SCNR_FUNC_ID,
            name: "SCNR",
            description: "Removes the residual green cast (SCNR).",
        },
        "Protection",
    ));
    let denoise = fields_func::<Denoise>(
        library,
        NodeSpec {
            id: DENOISE_FUNC_ID,
            name: "Denoise",
            description: "Wavelet denoise (starlet coefficient thresholding).",
        },
    );
    library.add(denoise);
    let hdr = fields_func::<Hdr>(
        library,
        NodeSpec {
            id: HDR_COMPRESSION_FUNC_ID,
            name: "HDR Compression",
            description: "Compresses large-scale dynamic range (multiscale HDR).",
        },
    );
    library.add(hdr);
    let local_contrast = fields_func::<LocalContrast>(
        library,
        NodeSpec {
            id: LOCAL_CONTRAST_FUNC_ID,
            name: "Local Contrast",
            description: "Local contrast enhancement (CLAHE).",
        },
    );
    library.add(local_contrast);
    library.add(processing_func(
        NodeSpec {
            id: NEUTRALIZE_BG_FUNC_ID,
            name: "Neutralize Background",
            description: "Shifts each channel so the background reads neutral gray.",
        },
        vec![Image::input("Image")],
        async_lambda!(move |Invocation {
                                inputs, outputs, ..
                            }| {
            let value = mem::take(&mut inputs[0]);
            outputs[0] = runtime::run_frame_op(value, |image| NeutralizeBg.apply(image)).await?;
            Ok(())
        }),
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

impl FrameOp for ExtractBg {
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

/// A node over `P`'s configs: the image, and `P`'s input named `name`.
fn preset_func<P>(spec: NodeSpec, name: &str) -> Func
where
    P: Preset,
    P::Config: FrameOp,
{
    processing_func(
        spec,
        vec![Image::input("Image"), P::input(name)],
        async_lambda!(move |Invocation {
                                inputs, outputs, ..
                            }| {
            let config = P::resolve(&inputs[1]);
            let value = mem::take(&mut inputs[0]);
            outputs[0] = runtime::run_frame_op(value, move |image| config.run(image)).await?;
            Ok(())
        }),
    )
}

/// A node over a `T` of a few plain knobs: the image, and `T`'s fields as
/// inputs of their own, each starting at the op's default.
fn fields_func<T>(library: &mut Library, spec: NodeSpec) -> Func
where
    T: FrameOp + Introspect + Clone + fmt::Debug + Sync,
{
    let fields = FieldInputs::<T>::new(library);
    let mut inputs = vec![Image::input("Image")];
    inputs.extend(fields.inputs());
    processing_func(
        spec,
        inputs,
        async_lambda!(move |Invocation {
                                inputs, outputs, ..
                            }| {
            fields = fields.clone(),
        } => {
            let config = fields.build(&inputs[1..])?;
            let value = mem::take(&mut inputs[0]);
            outputs[0] = runtime::run_frame_op(value, move |image| config.run(image)).await?;
            Ok(())
        }),
    )
}

fn processing_func(spec: NodeSpec, inputs: Vec<FuncInput>, lambda: FuncLambda) -> Func {
    spec.builder(lambda)
        .category("Astro")
        .pure()
        .inputs(inputs)
        .output(FuncOutput::new("Image", IMAGE_DATA_TYPE).description("Processed image."))
        .build()
}
