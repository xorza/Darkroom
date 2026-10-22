use super::*;
use std::cell::Cell;
use std::sync::Arc;

use imaginarium::ColorFormat;
use palantir::internals::ui_harness::UiHarness;

use crate::core::document::internals::DocumentFixture;
use crate::core::preview_sink;
use crate::core::wake;
use crate::gui::state::preview_store;

/// The header line: the fixed head, then the clauses that come and go — the
/// capped-texture note, the still-converting note and the zoom readout. All
/// are conditional, so the only way to know they land in the right order is
/// to render the combinations.
#[test]
fn header_readout_adds_its_conditional_clauses_only_when_they_apply() {
    let mut h = UiHarness::arena();
    let handle = h
        .ui()
        .load_image(&glyph::checker_image())
        .expect("a 2x2 checker fits every supported GPU");
    // The texture is the 2×2 checker throughout; `native_size` against
    // `texels` is what says whether the view is capped below its source.
    let shown = |native_size| DrawableImage {
        handle: handle.clone(),
        texels: UVec2::new(2, 2),
        native_size,
        native_format: ColorFormat::RGBA_U8,
    };
    let line = |view: &FullView, zoom| {
        HeaderReadout {
            title: "img",
            shown: view,
            zoom,
        }
        .to_string()
    };

    let full = FullView::Ready(shown(UVec2::new(2, 2)));
    assert_eq!(line(&full, None), "img · 2 × 2 · RGBA u8");
    assert_eq!(line(&full, Some(1.25)), "img · 2 × 2 · RGBA u8 · 125%");

    let capped = FullView::Ready(shown(UVec2::new(8192, 4096)));
    assert_eq!(
        line(&capped, None),
        "img · 8192 × 4096 · RGBA u8 · downscaled view"
    );
    assert_eq!(
        line(&capped, Some(0.5)),
        "img · 8192 × 4096 · RGBA u8 · downscaled view · 50%"
    );

    let converting = FullView::Converting(shown(UVec2::new(8192, 4096)));
    assert_eq!(
        line(&converting, Some(0.5)),
        "img · 8192 × 4096 · RGBA u8 · downscaled view · preparing full resolution · 50%"
    );
}

fn viewer_node() -> NodeId {
    NodeId::from_u128(1)
}

#[test]
fn sync_source_refits_only_for_size_changes_or_removal() {
    let mut viewer = ImageViewer::new(viewer_node());
    viewer.view = Some(Viewport {
        pan: Vec2::ZERO,
        zoom: 3.0,
    });
    viewer.sync_source(Some(UVec2::new(2, 2)));
    assert!(
        viewer.view.is_none(),
        "first image establishes fresh framing"
    );

    viewer.view = Some(Viewport {
        pan: Vec2::new(4.0, 5.0),
        zoom: 2.0,
    });
    viewer.sync_source(Some(UVec2::new(2, 2)));
    assert_eq!(
        viewer.view,
        Some(Viewport {
            pan: Vec2::new(4.0, 5.0),
            zoom: 2.0,
        }),
        "same-size revisions preserve inspection framing"
    );

    viewer.sync_source(Some(UVec2::new(3, 1)));
    assert!(viewer.view.is_none(), "dimension changes refit");
    viewer.view = Some(Viewport {
        pan: Vec2::ZERO,
        zoom: 4.0,
    });
    viewer.sync_source(None);
    assert!(viewer.view.is_none(), "removing the source clears framing");
}

/// The ways a store entry yields no texture each carry their own reason,
/// and the absent entry carries none — that last case is what draws the
/// standing "after the next graph run" hint rather than an error, so it
/// must stay distinguishable from a real failure.
#[test]
fn resolve_separates_no_entry_from_a_reason_there_is_no_image() {
    let mut h = UiHarness::arena();

    let nothing = ShownSource::resolve(None, h.ui());
    assert!(
        matches!(nothing, ShownSource::Nothing),
        "an absent entry is its own state, not a failure: {nothing:?}"
    );
    assert_eq!(nothing.hint().unwrap().to_string(), NOTHING_YET_HINT);

    let errored = StoredContent::Error(DrawablePreviewError::Empty);
    let resolved = ShownSource::resolve(Some(&errored), h.ui());
    assert!(resolved.image().is_none());
    assert_eq!(resolved.hint().unwrap().to_string(), "image is empty");

    // A good non-image value is not a failure, and reads as one thing a
    // viewer can't show rather than as that value's own formatting.
    let text = StoredContent::Text("7".to_owned());
    let resolved = ShownSource::resolve(Some(&text), h.ui());
    assert!(resolved.image().is_none());
    assert_eq!(
        resolved.hint().unwrap().to_string(),
        "value is not an image"
    );
}

/// One pass is the whole story: the frame that first draws a viewer frames it
/// against the pane it was handed, at the full texture's size, with the
/// thumbnail standing in. The full texture lands on a later frame, and the
/// framing does not move when it does.
///
/// The pane is a parameter because a viewer cannot measure itself — its
/// own widget is at most one pass old, and a tab switch destroys it. The
/// dock measures its content area instead, which outlives the tab in it,
/// so a switched-to viewer is handed a size on the very pass it records.
/// Nothing is owed a later pass, which is what stops the scale and the
/// zoom readout arriving a frame behind.
#[test]
fn the_frame_that_first_draws_a_viewer_frames_it_at_the_full_size() {
    let mut h = UiHarness::arena();
    let mut fixture = DocumentFixture::default();
    let node = fixture.add(&preview_sink::func(Arc::default()));
    let mut store = PreviewStore::new(wake::internals::noop());
    // 512×256: a 256×128 thumbnail, and a deviceless harness caps the full
    // texture at nothing, so it keeps the source's own size.
    store.publish(
        node,
        preview_store::internals::image_value(512, 256, ColorFormat::RGB_U8),
    );
    store.settle(h.ui());

    let theme = Theme::default();
    let mut preferences = ViewerPreferences::default();
    let mut viewer = ImageViewer::new(node);
    let pane = Some(Vec2::new(800.0, 600.0));
    // The record closure runs once per pass, so counting it is what
    // says the frame settled in one — palantir keeps its pass structure
    // to itself, and `FrameReport` does not name it.
    let passes = Cell::new(0u32);
    let mut frame = |h: &mut UiHarness, viewer: &mut ImageViewer, store: &PreviewStore| {
        h.frame(|ui| {
            passes.set(passes.get() + 1);
            let view = ViewerPane {
                theme: &theme,
                title: "img",
                previews: store,
                size: pane,
            };
            viewer.show(ui, view, &mut preferences);
        })
    };
    let first = frame(&mut h, &mut viewer, &store);

    assert_eq!(
        viewer.source_size,
        Some(UVec2::new(512, 256)),
        "the first pass framed itself at the full texture's size, \
         not at the thumbnail standing in for it"
    );
    assert!(
        !first.repaint_requested,
        "a viewer handed its pane owes the host nothing"
    );
    assert_eq!(
        passes.get(),
        1,
        "and needs no second pass to settle its framing"
    );

    let framing = Viewport {
        pan: Vec2::new(10.0, 20.0),
        zoom: 2.0,
    };
    viewer.view = Some(framing);
    store.settle(h.ui());
    frame(&mut h, &mut viewer, &store);
    assert_eq!(viewer.source_size, Some(UVec2::new(512, 256)));
    assert_eq!(
        viewer.view,
        Some(framing),
        "the full texture landing keeps the framing the user set"
    );
}
