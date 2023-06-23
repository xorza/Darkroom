use std::rc::Rc;

use crate::view::*;

pub struct Canvas {
    children: Vec<Rc<dyn View>>,
}

impl View for Canvas {

}

impl ViewWithChildren for Canvas {
    fn children(&mut self) -> &mut Vec<Rc<dyn View>> {
        &mut self.children
    }
}

impl Canvas {
    pub fn new() -> Self {
        Self {
            children: Vec::new(),
        }
    }
}