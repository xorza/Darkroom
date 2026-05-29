use crate::common::color_format::ColorFormat;
use crate::common::error::{Error, Result};
use crate::processing_context::ProcessingContext;
use crate::processing_context::image_buffer::ImageBuffer;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Backend {
    Cpu,
    #[cfg(feature = "wgpu")]
    Gpu,
}

/// Picks CPU or GPU for an op given the buffers' current location, the format,
/// and which backends support that format. Returns `Err` if formats don't match
/// or no backend supports the format.
pub(crate) fn select_backend(
    ctx: &ProcessingContext,
    buffers: &[&ImageBuffer],
    cpu_formats: &[ColorFormat],
    gpu_formats: &[ColorFormat],
    op_name: &str,
) -> Result<Backend> {
    assert!(!buffers.is_empty(), "select_backend called with no buffers");

    let format = buffers[0].desc.color_format;

    for buf in buffers {
        if buf.desc.color_format != format {
            return Err(Error::InvalidColorFormat(
                "all inputs and outputs must have the same color format".to_string(),
            ));
        }
    }

    let cpu_supported = cpu_formats.contains(&format);
    let gpu_supported = gpu_formats.contains(&format);

    if !cpu_supported && !gpu_supported {
        return Err(Error::UnsupportedFormat(format!(
            "color format {format} is not supported by {op_name}"
        )));
    }

    #[cfg(feature = "wgpu")]
    {
        let any_on_gpu = buffers.iter().any(|b| b.is_gpu());
        if any_on_gpu {
            assert!(ctx.has_gpu(), "data is on GPU but context has no GPU");
        }

        if any_on_gpu && gpu_supported {
            Ok(Backend::Gpu)
        } else if cpu_supported {
            Ok(Backend::Cpu)
        } else if ctx.has_gpu() {
            // GPU-only format (e.g. Transform), data currently on CPU.
            Ok(Backend::Gpu)
        } else {
            Err(Error::UnsupportedFormat(format!(
                "{op_name} requires a GPU for format {format}, but no GPU is available"
            )))
        }
    }

    #[cfg(not(feature = "wgpu"))]
    {
        let _ = (ctx, gpu_supported);
        if cpu_supported {
            Ok(Backend::Cpu)
        } else {
            Err(Error::UnsupportedFormat(format!(
                "color format {format} is not supported by {op_name} (GPU disabled)"
            )))
        }
    }
}
