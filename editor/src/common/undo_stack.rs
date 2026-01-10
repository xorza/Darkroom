use std::fmt::Debug;

mod action_undo_stack;
mod full_serde_undo_stack;

pub use action_undo_stack::ActionUndoStack;
pub use full_serde_undo_stack::FullSerdeUndoStack;

pub trait UndoStack<T: Debug>: Debug {
    type Action;
    fn reset_with(&mut self, value: &T);
    fn push_current(&mut self, value: &T, actions: &[Self::Action]);

    fn clear_redo(&mut self);
    fn undo(&mut self, value: &mut T, on_action: &mut dyn FnMut(&Self::Action)) -> bool;
    fn redo(&mut self, value: &mut T, on_action: &mut dyn FnMut(&Self::Action)) -> bool;
}
