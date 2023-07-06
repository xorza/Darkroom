use std::rc::Rc;

use crate::view::*;

#[derive(Default)]
pub struct Canvas {
    children: Vec<Rc<dyn View>>,
}

impl View for Canvas {}


impl ContentView for Canvas {
    fn children(&mut self) -> &mut Vec<Rc<dyn View>> {
        &mut self.children
    }
}

impl Canvas {

}
