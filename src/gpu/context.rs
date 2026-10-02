use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::fmt;

use crate::gpu::Gpu;

/// Trait marker for GPU pipelines that can be cached.
pub trait GpuPipeline: Any + fmt::Debug + Send + Sync {}

/// Cache for GPU pipelines, and the [`Gpu`] they were built against.
///
/// Lazily initializes pipelines on first use to avoid startup cost
/// for unused operations. Pipelines are stored by their `TypeId`.
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
    /// Creates a new `GpuContext` with no pipelines initialized.
    pub fn new(gpu: Gpu) -> Self {
        Self {
            gpu,
            pipelines: HashMap::new(),
        }
    }

    /// Returns the pipeline of type T, creating it with the provided function if needed.
    pub fn get_or_create<T, F>(&mut self, create: F) -> &T
    where
        T: GpuPipeline,
        F: FnOnce(&Gpu) -> T,
    {
        let pipeline: &dyn GpuPipeline = &**self
            .pipelines
            .entry(TypeId::of::<T>())
            .or_insert_with(|| Box::new(create(&self.gpu)));
        (pipeline as &dyn Any)
            .downcast_ref::<T>()
            .expect("a pipeline is stored under its own TypeId")
    }
}
