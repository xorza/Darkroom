use std::rc::Rc;

use crate::math::{FVec4, UVec2, UVec4};
use crate::renderer::{Draw, Renderer};
use crate::view::*;

pub struct Canvas {
    children: Vec<Rc<dyn View>>,
}

impl View for Canvas {

}


impl ContentView for Canvas {
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
