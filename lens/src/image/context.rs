use scenarium::ContextType;

#[derive(Debug)]
pub(super) struct VisionCtx {
    pub(super) processing_ctx: imaginarium::ProcessingContext,
}

impl Default for VisionCtx {
    fn default() -> Self {
        // Lens is CPU-only: skip GPU init entirely (every op has a CPU path).
        Self {
            processing_ctx: imaginarium::ProcessingContext::cpu_only(),
        }
    }
}

pub(super) const VISION_CTX_TYPE: ContextType<VisionCtx> = ContextType::new(VisionCtx::default);
