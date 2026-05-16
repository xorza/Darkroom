mod model;

use palantir::{Background, Color, Configure, Panel, Sizing, Ui, WinitHost, WinitHostConfig};
use scenarium::testing::test_graph;

use crate::model::ViewGraph;

struct AppState {
    #[allow(dead_code)]
    view_graph: ViewGraph,
}

impl AppState {
    fn new() -> Self {
        Self {
            view_graph: test_graph().into(),
        }
    }
}

fn main() {
    WinitHost::new(WinitHostConfig::new("darkroom"), AppState::new(), build_ui).run();
}

fn build_ui(ui: &mut Ui<AppState>) {
    Panel::vstack()
        .auto_id()
        .padding(16.0)
        .size((Sizing::FILL, Sizing::FILL))
        .background(Background {
            fill: Color::hex(0x1e1e1e).into(),
            ..Default::default()
        })
        .show(ui, |_ui| {});
}
