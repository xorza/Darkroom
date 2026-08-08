use std::any::{Any, TypeId};

use hashbrown::HashMap;
use hashbrown::hash_map::Entry;

use crate::common::error::Result;
use crate::gpu::Gpu;

/// Trait marker for GPU pipelines that can be cached.
pub trait GpuPipeline: Any + std::fmt::Debug + Send + Sync {}

/// Cache for GPU pipelines, and the [`Gpu`] they were built against.
///
/// Lazily initializes pipelines on first use to avoid startup cost
/// for unused operations. Pipelines are stored by their TypeId.
///
/// Purely a cache: it decides nothing about where an image lives or which backend an op runs on.
/// A caller that wants the GPU builds one of these, uploads with [`crate::GpuImage::from_image`],
/// and calls the op's `apply_gpu`.
#[derive(Debug)]
pub struct GpuContext {
    pub gpu: Gpu,
    pipelines: HashMap<TypeId, Box<dyn GpuPipeline>>,
}

impl GpuContext {
    /// Creates a new GpuContext with no pipelines initialized.
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
        if let Entry::Vacant(e) = self.pipelines.entry(type_id) {
            e.insert(Box::new(create(&self.gpu)?));
        }

        let pipeline = self
            .pipelines
            .get(&type_id)
            .expect("pipeline was just inserted");
        Ok((pipeline.as_ref() as &dyn Any)
            .downcast_ref::<T>()
            .expect("pipeline type mismatch - this is a bug"))
    }
}
