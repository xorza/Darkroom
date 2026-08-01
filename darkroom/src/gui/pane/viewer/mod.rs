//! Full-resolution viewers for preview nodes' runtime images, one editor tab
//! per node ([`TabRef::ImageViewer`], deduped on open). Each visible viewer
//! borrows its node's registered texture from the centralized preview
//! store and keeps only navigation state. Opening or restoring a tab therefore
//! shows an already-received value without an editor-driven notification path.
//!
//! The store materializes the full RGBA8 texture before the viewer records and
//! releases the source value immediately after registration. It does that only
//! for the nodes a pane will actually show
//! (`Document::visible_viewer_nodes`), so a viewer stacked behind another tab
//! holds no full-resolution texture until it is activated.
//!
//! Split the way the graph pane is, by what each part does rather than by what
//! it draws: [`camera`] is the affine algebra between texels, logical px and
//! the pane, [`glyph`] is the drawn vocabulary, and [`controls`] is the
//! floating chrome the panel stamps out. This file is [`ImageViewer`] and
//! nothing else — the state one viewer tab keeps across frames, and the record
//! pass that drives it.
//!
//! [`TabRef::ImageViewer`]: crate::core::document::TabRef::ImageViewer

pub(crate) mod camera;
pub(crate) mod controls;
pub(crate) mod glyph;

use scenarium::NodeId;
use std::fmt::Write as _;

use glam::{UVec2, Vec2};
use imaginarium::ColorFormat;
use palantir::{
    Align, Background, Color, Configure, HAlign, ImageFilter, ImageFit, ImageHandle, Panel, Sense,
    Shape, Sizing, Spacing, Ui, VAlign, WidgetId,
};

use crate::core::document::{Document, Viewport};
use crate::core::io::preferences::{ViewerBackground, ViewerPreferences};
use crate::gui::pane::graph::gesture::pan_zoom::fold_scroll_zoom;
use crate::gui::pane::viewer::camera::{
    VIEWER_MAX_ZOOM, VIEWER_MIN_ZOOM, draw_rect, fit_viewport, logical_image_size,
    zoom_about_pane_center,
};
use crate::gui::pane::viewer::controls::{BACKDROPS, control_wid, filter_toggle, readout_pill};
use crate::gui::state::preview_store::{FullImage, StoredContent};
use crate::gui::theme::Theme;
use crate::gui::widgets::toolbar::{BUTTON_GAP, Chip, TOOLBAR_MARGIN, pill, pill_rule};

/// One image-viewer tab's state: what it shows and how it's framed.
/// Lives in the `MainWindow`'s per-node viewer map, keyed by (and
/// carrying) the [`NodeId`] its tab binds to; content is runtime-only
/// (never persisted).
#[derive(Debug)]
pub(crate) struct ImageViewer {
    /// The preview node this viewer shows — keys the pane's widget id so
    /// two viewer tabs never share gesture responses.
    node_id: NodeId,
    /// Texture dimensions used to decide whether a new revision needs a refit.
    source_size: Option<UVec2>,
    /// Explicit viewport once the user pans/zooms; `None` = fit-to-pane
    /// (recomputed each frame, so it tracks pane resizes). The image's
    /// top-left offset in pane-local logical px plus the zoom (physical
    /// display px per texture texel). Texture dimensions are converted to
    /// their 1:1 logical footprint before applying it.
    view: Option<Viewport>,
    /// Pan-drag bookkeeping: the viewport pan at drag start. A bare
    /// `Option` — one viewer is one surface, so there is no pane to key
    /// it by the way the canvas has to.
    pan_anchor: Option<Vec2>,
    /// Lazily registered checkerboard tile for the `Checker` backdrop.
    /// The backdrop choice and magnification filter live in
    /// [`ViewerPreferences`] — one persisted setting shared by every
    /// viewer tab, threaded into [`Self::show`] each frame.
    checker: Option<ImageHandle>,
}

/// The texture a viewer paints this frame, once the store's content has
/// cleared the viewer's own bar.
#[derive(Clone, Copy, Debug)]
struct ShownImage<'a> {
    handle: &'a ImageHandle,
    /// Source dimensions before the texture-cap downscale.
    native_size: UVec2,
    /// Source pixel format before the RGBA8 view conversion.
    native_format: ColorFormat,
}

/// What [`StoredContent`] means to a viewer: at most one of a paintable
/// texture and a reason there isn't one. Both empty is the legitimate
/// nothing-yet case, which draws the standing hint rather than a message.
#[derive(Clone, Copy, Debug)]
struct ShownSource<'a> {
    shown: Option<ShownImage<'a>>,
    message: Option<&'a str>,
}

impl<'a> ShownSource<'a> {
    /// Read the store's entry for this viewer's node.
    ///
    /// Only the full-resolution upload is worth showing here — the pin card
    /// is happy with the thumbnail — so a stored image that hasn't finished
    /// materializing still resolves to a message rather than a texture.
    fn resolve(source: Option<&'a StoredContent>) -> Self {
        let Some(value) = source else {
            return Self {
                shown: None,
                message: None,
            };
        };
        let Some(image) = value.image() else {
            // No image to show. A failure reports its own reason; a
            // perfectly good non-image value doesn't get one, because
            // "7" is not what a viewer tab is for — the preview card
            // renders that.
            let message = match value {
                StoredContent::Error(message) => message.as_str(),
                _ => "this preview has no image value",
            };
            return Self {
                shown: None,
                message: Some(message),
            };
        };
        match &image.full {
            FullImage::Resident(handle) => Self {
                shown: Some(ShownImage {
                    handle,
                    native_size: image.native_size,
                    native_format: image.native_format,
                }),
                message: None,
            },
            FullImage::Failed(message) => Self {
                shown: None,
                message: Some(message.as_str()),
            },
            FullImage::Deferred(_) => {
                // This pane is its group's visible tab, and the reconcile
                // pass runs every frame ahead of the record, so it covered
                // this viewer — unless the tab became visible after that
                // pass, within this same frame.
                debug_assert!(
                    false,
                    "visible image viewer source was not materialized: \
                     the frame's reconcile pass did not cover it"
                );
                Self {
                    shown: None,
                    message: Some("image is being prepared"),
                }
            }
        }
    }
}

impl ImageViewer {
    /// An empty viewer for `node_id` (shows the hint until content arrives).
    pub(crate) fn new(node_id: NodeId) -> Self {
        Self {
            node_id,
            source_size: None,
            view: None,
            pan_anchor: None,
            checker: None,
        }
    }

    /// Back to fit-to-pane framing (and cancel any pan in progress) for a
    /// source change, the fit button, or a double-click.
    fn reset_framing(&mut self) {
        self.view = None;
        self.pan_anchor = None;
    }

    /// The framing to draw with: the user's explicit viewport, else the
    /// recomputed fit — the single source for the draw rect, the zoom
    /// readout, and the gesture/button math.
    fn effective_view(&self, img: Vec2, pane: Vec2) -> Viewport {
        self.view.unwrap_or_else(|| fit_viewport(img, pane))
    }

    /// Keep framing across same-size revisions and refit when the displayed
    /// texture dimensions change or the source disappears.
    fn sync_source(&mut self, source_size: Option<UVec2>) {
        if source_size != self.source_size {
            self.reset_framing();
            self.source_size = source_size;
        }
    }

    /// Draw the viewer pane (the whole tab content). Borrows the centralized
    /// texture, applies last frame's pan/zoom gestures, then paints the image
    /// (or message), header, and controls. Returns `true` when the shared
    /// viewer preferences changed.
    pub(crate) fn show(
        &mut self,
        ui: &mut Ui,
        theme: &Theme,
        prefs: &mut ViewerPreferences,
        title: &str,
        source: Option<&StoredContent>,
    ) -> bool {
        let ShownSource { shown, message } = ShownSource::resolve(source);
        self.sync_source(shown.map(|image| image.handle.size()));
        self.apply_gestures(ui, shown);

        let pane = pane_size(ui, self.node_id);
        let display_scale = ui.display().scale_factor;
        let fill = match prefs.background {
            ViewerBackground::Theme | ViewerBackground::Checker => theme.canvas.bg,
            ViewerBackground::Black => Color::BLACK,
            ViewerBackground::White => Color::WHITE,
        };
        let mut prefs_changed = false;
        Panel::zstack()
            .id(pane_wid(self.node_id))
            .size((Sizing::FILL, Sizing::FILL))
            .sense(Sense::CLICK | Sense::DRAG | Sense::SCROLL | Sense::PINCH)
            .clip_rect()
            .background(Background::fill(fill))
            .show(ui, |ui| {
                if prefs.background == ViewerBackground::Checker
                    && let Some(pane) = pane
                {
                    self.draw_checker(ui, pane);
                }
                match (shown, pane) {
                    (Some(shown), Some(pane)) => {
                        let img = logical_image_size(shown.handle.size(), display_scale);
                        let v = self.effective_view(img, pane);
                        ui.add_shape(
                            Shape::image(shown.handle.clone())
                                .at(draw_rect(img, v))
                                .fit(ImageFit::Fill)
                                .min_filter(ImageFilter::Linear)
                                .mag_filter(prefs.mag_filter),
                        );
                    }
                    // Pane not measured yet (first frame): let palantir fit it.
                    (Some(shown), None) => {
                        ui.add_shape(
                            Shape::image(shown.handle.clone())
                                .fit(ImageFit::Contain)
                                .min_filter(ImageFilter::Linear)
                                .mag_filter(prefs.mag_filter),
                        );
                    }
                    (None, _) => {
                        let hint = message
                            .unwrap_or("the port's image appears here after the next graph run");
                        // On the frosted readout pill, so the hint stays
                        // legible over the checker/white backdrops too.
                        let text = ui.intern(hint);
                        readout_pill(
                            ui,
                            theme,
                            Panel::hstack().id_salt("viewer_hint").align(Align::CENTER),
                            text,
                        );
                    }
                }
                if let Some(shown) = shown {
                    self.header(ui, theme, pane, title, shown);
                    prefs_changed = self.controls(ui, theme, pane, prefs, shown);
                }
            });
        prefs_changed
    }

    /// The screen-fixed checkerboard backdrop across the whole pane. One
    /// tiled 2×2 texture; `Nearest` keeps the squares crisp at any pane
    /// size and DPI.
    fn draw_checker(&mut self, ui: &mut Ui, pane: Vec2) {
        let handle = self
            .checker
            .get_or_insert_with(|| {
                ui.register_image(glyph::checker_image())
                    .expect("checker image fits every supported GPU")
            })
            .clone();
        ui.add_shape(
            Shape::image(handle)
                .fit(ImageFit::Tile {
                    offset: Vec2::ZERO,
                    // The 2×2 tile is one checker period = 2 squares across.
                    scale: pane / (2.0 * glyph::CHECKER_SQUARE_PX),
                })
                .min_filter(ImageFilter::Nearest)
                .mag_filter(ImageFilter::Nearest),
        );
    }

    /// The top-left readout: source node, native dimensions and pixel
    /// format, whether the view is texture-capped, and the current zoom.
    /// (`title` is never empty — [`node_label`] supplies the fallback.)
    fn header(
        &self,
        ui: &mut Ui,
        theme: &Theme,
        pane: Option<Vec2>,
        title: &str,
        shown: ShownImage<'_>,
    ) {
        let mut text = format!(
            "{} · {} × {} · {}",
            title, shown.native_size.x, shown.native_size.y, shown.native_format,
        );
        if shown.handle.size() != shown.native_size {
            text.push_str(" · downscaled view");
        }
        let img = logical_image_size(shown.handle.size(), ui.display().scale_factor);
        let zoom = match (self.view, pane) {
            (Some(v), _) => Some(v.zoom),
            (None, Some(pane)) => Some(self.effective_view(img, pane).zoom),
            // Pane not measured yet (first frame): no fit zoom to report.
            (None, None) => None,
        };
        if let Some(zoom) = zoom {
            let _ = write!(text, " · {:.0}%", zoom * 100.0);
        }
        readout_pill(
            ui,
            theme,
            Panel::hstack()
                .id_salt("viewer_header")
                .margin(Spacing::new(TOOLBAR_MARGIN, TOOLBAR_MARGIN, 0.0, 0.0)),
            text,
        );
    }

    /// The floating control panel in the pane's top-right corner — the
    /// viewer twin of the graph toolbar: function groups on stacked
    /// frosted pills, opaque chip buttons raised off each pill. The top
    /// pill frames the view (fit, 100%); the column below edits the
    /// shared appearance preferences — the backdrop radio stack and,
    /// past a rule, the sampling toggle. Returns `true` when `prefs`
    /// changed. Drawn after the image so the buttons hit-test above the
    /// pane's gesture surface. Framing clicks land next frame (responses
    /// lag the record by one frame) — imperceptible.
    fn controls(
        &mut self,
        ui: &mut Ui,
        theme: &Theme,
        pane: Option<Vec2>,
        prefs: &mut ViewerPreferences,
        shown: ShownImage<'_>,
    ) -> bool {
        let node_id = self.node_id;
        let mut changed = false;
        Panel::vstack()
            .id(control_wid(node_id, "panel"))
            .size((Sizing::HUG, Sizing::HUG))
            .align(Align::new(HAlign::Right, VAlign::Top))
            .child_align(Align::new(HAlign::Right, VAlign::Top))
            .margin(Spacing::new(0.0, TOOLBAR_MARGIN, TOOLBAR_MARGIN, 0.0))
            .gap(BUTTON_GAP)
            .show(ui, |ui| {
                let framing = Panel::hstack().id(control_wid(node_id, "pill_framing"));
                pill(ui, theme, framing, |ui| {
                    if Chip::new(control_wid(node_id, "fit"), "Fit to view").show(
                        ui,
                        theme,
                        glyph::draw_fit,
                    ) {
                        self.reset_framing();
                    }
                    if Chip::new(control_wid(node_id, "100"), "Zoom to 100%").show(
                        ui,
                        theme,
                        glyph::draw_100,
                    ) && let Some(pane) = pane
                    {
                        let img =
                            logical_image_size(shown.handle.size(), ui.display().scale_factor);
                        let v = self.effective_view(img, pane);
                        self.view = Some(zoom_about_pane_center(v, 1.0, pane));
                    }
                });
                let appearance = Panel::vstack().id(control_wid(node_id, "pill_appearance"));
                pill(ui, theme, appearance, |ui| {
                    for (mode, key, tip) in BACKDROPS {
                        let selected = prefs.background == mode;
                        if Chip::new(control_wid(node_id, key), tip).show(ui, theme, |ui, s, _| {
                            glyph::draw_swatch(ui, s, theme, mode, selected)
                        }) && !selected
                        {
                            prefs.background = mode;
                            changed = true;
                        }
                    }
                    // Rule between the backdrop radio stack and the
                    // sampling toggle — two concepts, one pill.
                    pill_rule(ui, theme);
                    changed |= filter_toggle(ui, theme, node_id, &mut prefs.mag_filter);
                });
            });
        changed
    }

    /// Fold last frame's pane gestures into the viewport: left/middle-drag
    /// pans, wheel/pinch zooms about the cursor, two-finger scroll pans,
    /// double-click resets to fit. The fit viewport materializes into an
    /// explicit one on the first adjusting gesture.
    fn apply_gestures(&mut self, ui: &Ui, shown: Option<ShownImage<'_>>) {
        let Some(shown) = shown else {
            return;
        };
        // Registered images have non-zero dims by construction, so the
        // texel size is always a valid divisor.
        let img = logical_image_size(shown.handle.size(), ui.display().scale_factor);
        let resp = ui.response_for(pane_wid(self.node_id));
        let Some(pane) = pane_size(ui, self.node_id) else {
            return;
        };
        if resp.left.double_clicked() {
            self.reset_framing();
            return;
        }
        let adjusting = resp.left.drag.started()
            || resp.middle.drag.started()
            || resp.scroll.pixels != Vec2::ZERO
            || resp.scroll.lines.y.abs() > f32::EPSILON
            || (resp.scroll.zoom - 1.0).abs() > f32::EPSILON;
        if self.view.is_none() && !adjusting {
            return;
        }
        let mut v = self.effective_view(img, pane);

        if resp.left.drag.started() || resp.middle.drag.started() {
            self.pan_anchor = Some(v.pan);
        }
        let drag = resp.left.drag.delta().or_else(|| resp.middle.drag.delta());
        // Measured from the latch, not integrated per frame, so a pan
        // lands where the pointer says however many frames it took; a
        // missing delta after a latch is the release edge.
        if let Some(start) = self.pan_anchor {
            match drag {
                Some(d) => v.pan = start + d,
                None => self.pan_anchor = None,
            }
        }
        fold_scroll_zoom(&mut v, ui, &resp, VIEWER_MIN_ZOOM, VIEWER_MAX_ZOOM);
        self.view = Some(v);
    }
}

/// Display label for a viewer tab / pane header: the node's name (falling
/// back to "image" for an unnamed node) plus a compact port tag, so
/// several ports of one node stay tellable apart — e.g. "stack · out 1".
/// The one formatter for both the tab strip and the viewer title.
///
/// A recursive whole-document node search plus a fresh `String`, so
/// resolve it once per tab per frame rather than once per reader.
pub(crate) fn node_label(doc: &Document, node_id: NodeId) -> String {
    doc.graph
        .find(node_id)
        .map(|n| n.name.as_str())
        .filter(|n| !n.is_empty())
        .unwrap_or("image")
        .to_owned()
}

/// Last frame's measured pane size, `None` before the first layout.
fn pane_size(ui: &Ui, node_id: NodeId) -> Option<Vec2> {
    let size = ui.response_for(pane_wid(node_id)).layout_rect?.size;
    (size.w > 0.0 && size.h > 0.0).then(|| Vec2::new(size.w, size.h))
}

/// Stable id for a viewer's pane — keyed by node so switching between two
/// viewer tabs can't cross-feed their gesture responses.
fn pane_wid(node_id: NodeId) -> WidgetId {
    WidgetId::from_hash(("image_viewer.pane", node_id))
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// The three ways a store entry fails to yield a texture each carry their
    /// own reason, and the absent entry carries none — that last case is what
    /// draws the standing "after the next graph run" hint rather than an
    /// error, so it must stay distinguishable from a real failure.
    #[test]
    fn resolve_separates_no_entry_from_a_reason_there_is_no_image() {
        let nothing = ShownSource::resolve(None);
        assert!(nothing.shown.is_none() && nothing.message.is_none());

        let errored = StoredContent::Error("boom".to_owned());
        let resolved = ShownSource::resolve(Some(&errored));
        assert!(resolved.shown.is_none());
        assert_eq!(resolved.message, Some("boom"));
    }
}
