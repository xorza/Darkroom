mod model;
mod view;

use std::cell::RefCell;
use std::rc::Rc;

use glam::Vec2;
use palantir::{WinitHost, WinitHostConfig};
use scenarium::testing::test_graph;

use crate::model::ViewGraph;

fn main() {
    let mut view_graph: ViewGraph = test_graph().into();
    view_graph.auto_layout(220.0, 110.0, Vec2::new(40.0, 40.0));
    let view_graph = Rc::new(RefCell::new(view_graph));

    let vg = view_graph.clone();
    WinitHost::new(WinitHostConfig::new("darkroom"), (), move |ui| {
        view::build(ui, &vg.borrow());
    })
    .run();
}
