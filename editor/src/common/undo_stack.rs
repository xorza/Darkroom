use std::fmt::Debug;

mod full_serde_undo_stack;

pub use full_serde_undo_stack::FullSerdeUndoStack;

pub trait UndoStack<T: Debug>: Debug {
    fn reset_with(&mut self, value: &T);
    fn push_current(&mut self, value: &T);
    fn clear_redo(&mut self);
    fn undo(&mut self) -> Option<T>;
    fn redo(&mut self) -> Option<T>;
}
