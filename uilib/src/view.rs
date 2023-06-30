use std::rc::Rc;

use crate::math::FVec4;
use crate::renderer::Renderer;

pub trait Draw {
    fn draw(&self, renderer: &mut Renderer);
}

pub trait View {
    fn update(&mut self) {}
}

pub trait ContentView: View {
    fn children(&mut self) -> &mut Vec<Rc<dyn View>>;
    fn add_child(&mut self, child: Rc<dyn View>) {
        self.children().push(child);
    }
    fn remove_child(&mut self, child: Rc<dyn View>) {
        self.children().retain(|c| Rc::ptr_eq(c, &child));
    }
}

