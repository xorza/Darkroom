use std::any::{Any, TypeId};

use hashbrown::HashMap;

use crate::prelude::*;

/// Trait marker for GPU pipelines that can be cached.
pub trait GpuPipeline: Any + std::fmt::Debug + Send + Sync {}

/// Cache for GPU pipelines.
///
/// Lazily initializes pipelines on first use to avoid startup cost
/// for unused operations. Pipelines are stored by their TypeId.
#[derive(Debug)]
pub struct GpuContext {
    gpu: Gpu,
    pipelines: HashMap<TypeId, Box<dyn GpuPipeline>>,
}

impl GpuContext {
    /// Creates a new PipelineCache with no pipelines initialized.
    pub fn new(gpu: Gpu) -> Self {
        Self {
            gpu,
            pipelines: HashMap::new(),
        }
    }

    /// Returns the pipeline of type T, creating it with the provided function if needed.
    pub fn get_or_create<T, F>(&mut self, create: F) -> Result<&T>
    where
        T: GpuPipeline,
        F: FnOnce(&Gpu) -> Result<T>,
    {
        let type_id = TypeId::of::<T>();

        if !self.pipelines.contains_key(&type_id) {
            let pipeline = create(&self.gpu)?;
            self.pipelines.insert(type_id, Box::new(pipeline));
        }

        Ok(self
            .pipelines
            .get(&type_id)
            .and_then(|p| (p.as_ref() as &dyn Any).downcast_ref::<T>())
            .expect("pipeline type mismatch - this is a bug"))
    }

    /// Returns a reference to the GPU context.
    pub fn gpu(&self) -> &Gpu {
        &self.gpu
    }
}
