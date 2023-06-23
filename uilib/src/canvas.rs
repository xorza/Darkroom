use std::rc::Rc;

use crate::math::{FVec4, UVec2, UVec4};
use crate::renderer::{Draw, Renderer};
use crate::view::*;

pub struct Canvas {
    children: Vec<Rc<dyn View>>,
}

impl View for Canvas {
    fn draw(&self, renderer: &mut Renderer) {
        let draw = Draw::Rect {
            pos: UVec2 { x: 0, y: 0 },
            size: UVec2 { x: 200, y: 200 },
            color: FVec4 { x: 0.0, y: 0.0, z: 0.0, w: 0.0 },
        };
        renderer.draw(draw);
    }
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
