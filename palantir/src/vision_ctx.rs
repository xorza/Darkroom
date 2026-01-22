use std::sync::LazyLock;

use scenarium::context::ContextType;

#[derive(Debug, Default)]
pub struct VisionCtx {
    pub processing_ctx: imaginarium::ProcessingContext,
}

pub static VISION_CTX_TYPE: LazyLock<ContextType> = LazyLock::new(|| {
    ContextType::new(
        "46a85022-e3c7-4c80-aa90-cf1a77251286".into(),
        VisionCtx::default,
    )
});
