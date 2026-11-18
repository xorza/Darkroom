use scenarium::GraphId;

use crate::core::document::internals::DocumentFixture;
use crate::gui::pane::graph_ui::internals::CanvasHarness;
use crate::gui::pane::graph_ui::scene::detail_level::DetailLevel;
use crate::gui::pane::graph_ui::wid;
use crate::gui::theme::Theme;

/// At the title level a node's words are laid out and not painted, so its body reads the
/// standing title alone, once, and a driver locator finds none of the hidden words. At full
/// detail the body reads its own words, its input's empty field and its footer, the title being
/// the header's own listed widget. Zoom 0.25 lands 14 px body text at 3.5 px, under the 6 px the
/// title level starts at.
#[test]
fn a_node_at_the_title_level_reads_its_title_alone() {
    let mut h = CanvasHarness::new(DocumentFixture::probes(1));
    let body = wid::body(GraphId::Main, h.node(0));
    let body_text = |h: &CanvasHarness| {
        h.ui.widgets(None)
            .into_iter()
            .find(|widget| widget.id == body)
            .expect("the body is listed")
            .text
    };
    h.prime(2);
    assert_eq!(body_text(&h), "\u{2014} Not run");

    let zoom = 0.25;
    assert_eq!(
        DetailLevel::at(zoom, Theme::default().font.body),
        DetailLevel::Title
    );
    h.document_mut().main_view.viewport.zoom = zoom;
    h.prime(2);
    assert_eq!(body_text(&h), "probe");
    assert!(
        h.ui.widgets(Some("Not run")).is_empty(),
        "no widget reads the hidden footer"
    );
}
