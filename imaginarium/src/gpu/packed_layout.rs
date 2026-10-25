use bytemuck::{Pod, Zeroable};

use crate::image::image_desc::InterleavedDesc;

/// Channel value with no alpha role, as `NO_ALPHA` in `packed.wgsl`.
const NO_ALPHA: u32 = u32::MAX;

/// How an image's channel values sit in its packed buffer — the tail of the uniform block of
/// every shader that walks the buffer word by word.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct PackedLayout {
    byte_len: u32,
    sample_byte_len: u32,
    channel_count: u32,
    alpha_channel: u32,
}

impl PackedLayout {
    pub(crate) fn new(desc: InterleavedDesc) -> Self {
        let format = desc.color_format();
        let channel_count =
            u32::try_from(format.channel_count.count()).expect("at most four channels");
        Self {
            byte_len: u32::try_from(desc.byte_len())
                .expect("a GpuImage fits a storage binding, whose size is a u32"),
            sample_byte_len: u32::try_from(format.sample_type.byte_len())
                .expect("at most four bytes"),
            channel_count,
            alpha_channel: if format.has_alpha() {
                channel_count - 1
            } else {
                NO_ALPHA
            },
        }
    }

    /// The `u32` words the buffer spans, one invocation each.
    pub(crate) const fn words(&self) -> u32 {
        self.byte_len.div_ceil(4)
    }
}
