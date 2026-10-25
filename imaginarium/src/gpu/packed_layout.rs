use bytemuck::{Pod, Zeroable};

use crate::image::image_desc::ImageDesc;

/// Channel value with no alpha role, as `NO_ALPHA` in `packed.wgsl`.
const NO_ALPHA: u32 = u32::MAX;

/// How an image's channel values sit in its packed buffer — the tail of the uniform block of
/// every shader that walks the buffer word by word.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct PackedLayout {
    total_bytes: u32,
    elem_size: u32,
    channels: u32,
    alpha_channel: u32,
}

impl PackedLayout {
    pub(crate) fn new(desc: ImageDesc) -> Self {
        let format = desc.color_format;
        let channels = u32::try_from(format.channel_count.count()).expect("at most four channels");
        Self {
            total_bytes: u32::try_from(desc.size_in_bytes())
                .expect("a GpuImage fits a storage binding, whose size is a u32"),
            elem_size: u32::try_from(format.sample_type.size()).expect("at most four bytes"),
            channels,
            alpha_channel: if format.has_alpha() {
                channels - 1
            } else {
                NO_ALPHA
            },
        }
    }

    /// The `u32` words the buffer spans, one invocation each.
    pub(crate) const fn words(&self) -> u32 {
        self.total_bytes.div_ceil(4)
    }
}
