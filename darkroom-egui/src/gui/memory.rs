//! Persistence helpers keyed by [`StableId`]. Returned from
//! [`Gui::memory`] so the `Gui` surface stays focused on layout/scope/scale.

use crate::common::StableId;

/// Thin view over `egui::Memory::data` keyed by [`StableId`].
/// Returned by [`Gui::memory`].
#[derive(Debug, Clone, Copy)]
pub struct Memory<'a> {
    ctx: &'a egui::Context,
}

impl<'a> Memory<'a> {
    pub(crate) fn new(ctx: &'a egui::Context) -> Self {
        Self { ctx }
    }

    /// Load a persisted value (survives app restarts). Returns `default`
    /// when the slot is empty.
    pub fn load_persistent<T>(&self, id: StableId, default: T) -> T
    where
        T: 'static + Clone + Send + Sync + serde::Serialize + for<'de> serde::Deserialize<'de>,
    {
        self.ctx
            .data_mut(|d| d.get_persisted::<T>(id.id()).unwrap_or(default))
    }

    /// Write a persisted value.
    pub fn store_persistent<T>(&self, id: StableId, value: T)
    where
        T: 'static + Clone + Send + Sync + serde::Serialize + for<'de> serde::Deserialize<'de>,
    {
        self.ctx.data_mut(|d| d.insert_persisted(id.id(), value));
    }

    /// Load a temporary value (cleared on app restart).
    pub fn load_temp<T: 'static + Clone + Send + Sync>(&self, id: StableId) -> Option<T> {
        self.ctx.data_mut(|d| d.get_temp::<T>(id.id()))
    }

    /// Write a temporary value.
    pub fn store_temp<T: 'static + Clone + Send + Sync>(&self, id: StableId, value: T) {
        self.ctx.data_mut(|d| d.insert_temp(id.id(), value));
    }

    /// Delete a temporary value of type `T`.
    pub fn remove_temp<T: 'static + Send + Sync>(&self, id: StableId) {
        self.ctx.data_mut(|d| d.remove::<T>(id.id()));
    }
}
