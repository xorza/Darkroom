use std::sync::LazyLock;

use scenarium::context::ContextType;

#[derive(Debug)]
pub struct VisionCtx {
    pub processing_ctx: imaginarium::ProcessingContext,
}

impl Default for VisionCtx {
    fn default() -> Self {
        Self {
            processing_ctx: imaginarium::ProcessingContext::new(),
        }
    }
}

pub static VISION_CTX_TYPE: LazyLock<ContextType> = LazyLock::new(|| {
    ContextType::new(
        "46a85022-e3c7-4c80-aa90-cf1a77251286".into(),
        VisionCtx::default,
    )
});
