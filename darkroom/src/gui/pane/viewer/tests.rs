use super::*;
use std::sync::Arc;

use imaginarium::ColorFormat;
use palantir::internals::UiHarness;

use crate::core::document::harness::DocFixture;
use crate::core::preview::preview_func;
use crate::gui::state::preview_store::internals::opaque_image_value;

/// The header line: the fixed head, then the two clauses that come and go
/// — the capped-texture note and the zoom readout. Both are conditional,
/// so the only way to know they land in the right order is to render all
/// four combinations.
#[test]
fn header_readout_adds_its_downscale_and_zoom_clauses_only_when_they_apply() {
    let mut h = UiHarness::arena();
    let handle = h
        .ui()
        .load_image(&glyph::checker_image())
        .expect("a 2x2 checker fits every supported GPU");
    // The texture is the 2×2 checker either way; `native_size` is what
    // says whether the view is capped below its source.
    let shown = |native_size| DrawableImage {
        handle: handle.clone(),
        native_size,
        native_format: ColorFormat::RGBA_U8,
    };
    let line = |image: &DrawableImage, zoom| {
        HeaderReadout {
            title: "img",
            shown: image,
            zoom,
        }
        .to_string()
    };

    let full = shown(UVec2::new(2, 2));
    assert_eq!(line(&full, None), "img · 2 × 2 · RGBA u8");
    assert_eq!(line(&full, Some(1.25)), "img · 2 × 2 · RGBA u8 · 125%");

    let capped = shown(UVec2::new(8192, 4096));
    assert_eq!(
        line(&capped, None),
        "img · 8192 × 4096 · RGBA u8 · downscaled view"
    );
    assert_eq!(
        line(&capped, Some(0.5)),
        "img · 8192 × 4096 · RGBA u8 · downscaled view · 50%"
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

    let errored = StoredContent::Error(PreviewImageError::Empty);
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

/// One pass is the whole story: the frame that first draws a viewer
/// uploads its texture and frames it against the pane it was handed.
///
/// The pane is a parameter because a viewer cannot measure itself — its
/// own widget is at most one pass old, and a tab switch destroys it. The
/// dock measures its content area instead, which outlives the tab in it,
/// so a switched-to viewer is handed a size on the very pass it records.
/// Nothing is owed a later pass or a later frame, which is what stops the
/// scale and the zoom readout arriving a frame behind.
#[test]
fn the_frame_that_first_draws_a_viewer_uploads_and_frames_it() {
    let mut h = UiHarness::arena();
    let mut fixture = DocFixture::default();
    let node = fixture.add(&preview_func(Arc::default()));
    let mut store = PreviewStore::default();
    // 2×1 (see `opaque_image_value`) — the full texture keeps the source's
    // own dimensions, so the assertion can name them.
    store.ingest_preview(h.ui(), node, opaque_image_value());

    let theme = Theme::default();
    let mut prefs = ViewerPreferences::default();
    let mut viewer = ImageViewer::new(node);
    let pane = Some(Vec2::new(800.0, 600.0));
    // The record closure runs once per pass, so counting it is what
    // says the frame settled in one — palantir keeps its pass structure
    // to itself, and `FrameReport` no longer names it.
    let mut passes = 0u32;
    let first = h.frame(|ui| {
        passes += 1;
        viewer.show(ui, &theme, &mut prefs, "img", &store, pane);
    });

    assert_eq!(
        viewer.source_size,
        Some(UVec2::new(2, 1)),
        "the first pass framed itself against the uploaded texture, \
         not against an absent one"
    );
    assert!(
        !first.repaint_requested,
        "a viewer handed its pane owes the host nothing"
    );
    assert_eq!(passes, 1, "and needs no second pass to settle its framing");
}
