use std::fmt::Debug;

pub trait UndoStack<T: Debug>: Debug {
    type Action;
    fn reset_with(&mut self, value: &T);
    fn push_current(&mut self, value: &T, actions: &[Self::Action]);

    fn clear_redo(&mut self);
    fn undo(&mut self, value: &mut T, on_action: &mut dyn FnMut(&Self::Action)) -> bool;
    fn redo(&mut self, value: &mut T, on_action: &mut dyn FnMut(&Self::Action)) -> bool;
}
