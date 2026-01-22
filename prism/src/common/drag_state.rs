//! Generic drag state management for egui.
//!
//! Provides a consistent pattern for handling drag operations that need to:
//! 1. Store an original value when drag starts
//! 2. Track changes during dragging
//! 3. Commit or revert on drag stop

use egui::{Id, Response, Ui};

/// Result of a drag state update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragResult<T> {
    /// No drag activity.
    Idle,
    /// Drag just started, original value stored.
    Started,
    /// Drag in progress.
    Dragging,
    /// Drag stopped, returns the original value for comparison/undo.
    Stopped { start_value: T },
}

/// Manages drag state using egui's temporary data storage.
///
/// This helper encapsulates the common pattern of:
/// - Storing the original value when a drag starts
/// - Retrieving it during dragging for delta calculations
/// - Returning it when drag stops for undo/commit purposes
#[derive(Debug)]
pub struct DragState<T> {
    id: Id,
    _marker: std::marker::PhantomData<T>,
}

impl<T: Clone + Default + Send + Sync + 'static> DragState<T> {
    /// Create a new drag state manager with the given ID.
    pub fn new(id: Id) -> Self {
        Self {
            id,
            _marker: std::marker::PhantomData,
        }
    }

    /// Update drag state based on response.
    ///
    /// - On drag start: stores `current_value` as the start value
    /// - On drag stop: removes and returns the start value
    /// - Otherwise: returns appropriate state
    pub fn update(&self, ui: &mut Ui, response: &Response, current_value: T) -> DragResult<T> {
        if response.drag_started() {
            ui.data_mut(|data| data.insert_temp(self.id, current_value));
            return DragResult::Started;
        }

        if response.drag_stopped() {
            let start_value = ui
                .data_mut(|data| data.remove_temp::<T>(self.id))
                .expect("drag start value must exist on drag_stopped");
            return DragResult::Stopped { start_value };
        }

        if response.dragged() {
            return DragResult::Dragging;
        }

        DragResult::Idle
    }

    /// Get the stored start value during a drag operation.
    ///
    /// Returns `None` if not currently dragging.
    pub fn start_value(&self, ui: &Ui) -> Option<T> {
        ui.data(|data| data.get_temp::<T>(self.id))
    }

    /// Check if a drag is currently active.
    pub fn is_dragging(&self, ui: &Ui) -> bool {
        ui.data(|data| data.get_temp::<T>(self.id).is_some())
    }

    /// Cancel an ongoing drag, removing stored state.
    pub fn cancel(&self, ui: &mut Ui) {
        ui.data_mut(|data| {
            data.remove::<T>(self.id);
        });
    }
}
