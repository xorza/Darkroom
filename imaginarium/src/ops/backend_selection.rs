use crate::common::color_format::ColorFormat;
use crate::common::error::{Error, Result};
use crate::processing_context::{ImageBuffer, ProcessingContext};

/// Result of backend selection for an operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Cpu,
    Gpu,
}

/// Selects the appropriate backend (CPU or GPU) for an operation.
///
/// The logic:
/// 1. Validates that all buffers have the same color format
/// 2. Checks if the format is supported by CPU and/or GPU
/// 3. If any buffer is on GPU and GPU supports the format, use GPU
/// 4. Otherwise, if CPU supports the format, use CPU
/// 5. Otherwise, if GPU supports the format, use GPU
///
/// # Arguments
/// * `ctx` - The processing context (for GPU availability check)
/// * `buffers` - Iterator of image buffers to check
/// * `cpu_formats` - Slice of color formats supported by CPU implementation
/// * `gpu_formats` - Slice of color formats supported by GPU implementation
/// * `op_name` - Name of the operation (for error messages)
///
/// # Returns
/// * `Ok(Backend::Cpu)` - Use CPU backend
/// * `Ok(Backend::Gpu)` - Use GPU backend
/// * `Err` - If format validation fails or format is not supported
pub fn select_backend<'a>(
    ctx: &ProcessingContext,
    buffers: impl IntoIterator<Item = &'a ImageBuffer>,
    cpu_formats: &[ColorFormat],
    gpu_formats: &[ColorFormat],
    op_name: &str,
) -> Result<Backend> {
    let buffers: Vec<&ImageBuffer> = buffers.into_iter().collect();

    debug_assert!(!buffers.is_empty(), "buffers must not be empty");

    let format = buffers[0].desc().color_format;

    // Validate all buffers have same format
    for buf in &buffers {
        if buf.desc().color_format != format {
            return Err(Error::InvalidColorFormat(
                "all inputs and outputs must have the same color format".to_string(),
            ));
        }
    }

    let cpu_supported = cpu_formats.contains(&format);
    let gpu_supported = gpu_formats.contains(&format);

    if !cpu_supported && !gpu_supported {
        return Err(Error::UnsupportedFormat(format!(
            "color format {} is not supported by {}",
            format, op_name
        )));
    }

    let any_on_gpu = buffers.iter().any(|b| b.is_gpu());
    if any_on_gpu {
        debug_assert!(ctx.has_gpu(), "data is on GPU but context has no GPU");
    }

    if any_on_gpu && gpu_supported {
        Ok(Backend::Gpu)
    } else if cpu_supported {
        Ok(Backend::Cpu)
    } else {
        // gpu_supported must be true here
        Ok(Backend::Gpu)
    }
}
