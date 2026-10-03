//! Full-resolution viewers for preview nodes' runtime images, one editor tab
//! per node ([`TabRef::ImageViewer`], deduped on open). Each visible viewer
//! borrows its node's registered texture from the centralized preview
//! store and keeps only navigation state. Opening or restoring a tab therefore
//! shows an already-received value without an editor-driven notification path.
//!
//! The full RGBA8 texture is uploaded by the store on demand, the first time a
//! viewer asks for it while recording, and the source value is released as it
//! goes. Asking is what scopes it: a pane records only while its tab is the
//! visible one, so a viewer stacked behind another holds no full-resolution
//! texture until it is activated — and one activated mid-record draws at full
//! resolution in the pass that puts it on screen, never a frame later.
//!
//! Split the way the graph pane is, by what each part does rather than by what
//! it draws: [`camera`] is the affine algebra between texels, logical px and
//! the pane, [`glyph`] is the drawn vocabulary, and [`controls`] is the
//! floating chrome the panel stamps out. This file is [`ImageViewer`] and
//! nothing else — the state one viewer tab keeps across frames, and the record
//! pass that drives it.
//!
//! [`TabRef::ImageViewer`]: crate::core::document::TabRef::ImageViewer

mod camera;
mod controls;
mod glyph;

use scenarium::NodeId;
use std::fmt;
use std::fmt::Display;
use std::fmt::Formatter;

use palantir::prelude::*;
use palantir::widget::Shape;
use palantir::{ImageDownsample, ImageFilter, ImageFit, ImageHandle};

use crate::core::document::{Document, Viewport};
use crate::core::io::preferences::{ViewerBackground, ViewerPreferences};
use crate::gui::pane::graph::gesture::pan_zoom;
use crate::gui::pane::graph::gesture::slot::GestureSlot;
use crate::gui::pane::viewer::camera::{
    VIEWER_MAX_ZOOM, VIEWER_MIN_ZOOM, draw_rect, fit_viewport, zoom_about_pane_center,
};
use crate::gui::pane::viewer::controls::{BACKDROPS, control_wid, filter_toggle, readout_pill};
use crate::gui::state::preview_store::error::PreviewImageError;
use crate::gui::state::preview_store::{
    DrawableImage, PreviewMessage, PreviewStore, StoredContent,
};
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
    pan_anchor: GestureSlot<Vec2>,
    /// Lazily registered checkerboard tile for the `Checker` backdrop.
    /// The backdrop choice and magnification filter live in
    /// [`ViewerPreferences`] — one persisted setting shared by every
    /// viewer tab, threaded into [`Self::show`] each frame.
    checker: Option<ImageHandle>,
}

/// Drawn when a viewer's node has published nothing yet. An invitation rather
/// than a diagnosis, which is why [`ShownSource::Nothing`] is its own state and
/// not a message.
const NOTHING_YET_HINT: &str = "the port's image appears here after the next graph run";

/// What [`StoredContent`] means to a viewer: a texture to paint, why there
/// isn't one, or nothing published yet.
///
/// Three states, so not a `Result` — "no value yet" is neither a texture nor a
/// failure, and it is the state a freshly opened viewer sits in. No lifetime
/// either: a texture handle is refcounted and a reason is owned, so nothing
/// here borrows from the entry it was resolved from.
#[derive(Clone, Debug)]
enum ShownSource {
    Nothing,
    Image(DrawableImage),
    /// Why this value has no picture. Carries the reason rather than its
    /// wording, so the pane decides how to say it — including for a perfectly
    /// good non-image value, which is [`PreviewImageError::NotAnImage`] and
    /// not a fault.
    NoImage(PreviewImageError),
}

impl ShownSource {
    /// Read the store's entry for this viewer's node, uploading its
    /// full-resolution texture if this is the first pass to ask.
    ///
    /// Asking here is what scopes the upload: this runs from the record of a
    /// pane that is being drawn, so only a viewer actually on screen ever
    /// costs one — and it costs it *now*, in the pass that draws it, rather
    /// than a frame later. The preview card never asks; its thumbnail is
    /// already resident.
    fn resolve(source: Option<&StoredContent>, ui: &Ui) -> Self {
        match source {
            None => Self::Nothing,
            Some(StoredContent::Image(image)) => match image.full(ui) {
                Ok(drawable) => Self::Image(drawable),
                Err(error) => Self::NoImage(error),
            },
            // A stored failure keeps its own reason. A perfectly good
            // non-image value is `NotAnImage`: "7" is not what a viewer tab is
            // for, and the preview card is what renders it.
            Some(StoredContent::Error(error)) => Self::NoImage(error.clone()),
            Some(StoredContent::Text(_)) => Self::NoImage(PreviewImageError::NotAnImage),
        }
    }

    fn image(&self) -> Option<&DrawableImage> {
        match self {
            Self::Image(image) => Some(image),
            Self::Nothing | Self::NoImage(_) => None,
        }
    }

    fn hint(&self) -> Option<PreviewMessage<'_>> {
        match self {
            Self::Image(_) => None,
            Self::NoImage(error) => Some(PreviewMessage::Failure(error)),
            Self::Nothing => Some(PreviewMessage::Text(NOTHING_YET_HINT)),
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
            pan_anchor: GestureSlot::default(),
            checker: None,
        }
    }

    fn reset_framing(&mut self) {
        self.view = None;
        self.pan_anchor.clear();
    }

    /// The framing to draw with: the user's explicit viewport, else the
    /// recomputed fit — the single source for the draw rect, the zoom
    /// readout, and the gesture/button math.
    ///
    /// **Only the fit needs `pane`.** A viewer carrying its own framing
    /// already knows its rect, so it answers even on a pass where the pane has
    /// never been arranged — which is every pass that puts a viewer tab on
    /// screen for the first time. Gating the whole thing on a measured pane
    /// would make a zoomed viewer draw at fit for a frame and snap to its real
    /// scale afterwards.
    ///
    /// `None` therefore means only "no framing yet": no explicit viewport and
    /// no pane to fit into. [`ImageFit::Contain`] is the right picture there —
    /// [`fit_viewport`] is defined to match it — so nothing is owed a later
    /// pass.
    ///
    /// Converts the texture to logical px itself rather than taking the
    /// figure: every caller wanted the viewport, and only one of them wanted
    /// the size too. Registered images have non-zero dims by construction, so
    /// the fit always has a valid divisor.
    fn effective_view(
        &self,
        ui: &Ui,
        image: &DrawableImage,
        pane: Option<Vec2>,
    ) -> Option<Viewport> {
        self.view
            .or_else(|| Some(fit_viewport(logical_size(image, ui), pane?)))
    }

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
        previews: &PreviewStore,
        pane: Option<Vec2>,
    ) -> bool {
        // The store rather than the entry: this viewer *is* its node, so
        // handing it the whole store is one lookup by an id that cannot
        // disagree with the one keying the pane's widgets.
        //
        // Resolving is also what uploads a not-yet-registered full-resolution
        // texture, so this pass draws it — there is no state where the pane
        // owes itself a later frame.
        let source = ShownSource::resolve(previews.entries.get(&self.node_id), ui);
        self.sync_source(source.image().map(|image| image.handle.size()));
        self.apply_gestures(ui, source.image(), pane);

        // The checker draws over the theme's backdrop.
        let fill = glyph::flat_fill(theme, prefs.background).unwrap_or(theme.canvas.bg);
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
                if let Some(shown) = source.image() {
                    let image = Shape::image(shown.handle.clone())
                        .min_filter(ImageFilter::Linear)
                        .mag_filter(prefs.mag_filter)
                        // Area averaging preserves source brightness when minifying.
                        .downsample(ImageDownsample::Mean);
                    ui.add_shape(match self.effective_view(ui, shown, pane) {
                        Some(v) => image
                            .at(draw_rect(logical_size(shown, ui), v))
                            .fit(ImageFit::Fill),
                        // No framing yet — no zoom of its own and no pane to
                        // fit into. Palantir's own contain draws exactly what
                        // the fit would.
                        None => image.fit(ImageFit::Contain),
                    });
                    self.header(ui, theme, pane, title, shown);
                    prefs_changed = self.controls(ui, theme, pane, prefs, shown);
                }
                // Complementary to the image above, so this is the same
                // either/or the enum states — never a second thing drawn over
                // the picture. On the frosted readout pill, so the line stays
                // legible over the checker/white backdrops too.
                if let Some(hint) = source.hint() {
                    let text = fmt!(ui, "{hint}");
                    readout_pill(
                        ui,
                        theme,
                        Panel::hstack().id_salt("viewer_hint").align(Align::CENTER),
                        text,
                    );
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
                ui.load_image(&glyph::checker_image())
                    .expect("checker image fits every supported GPU")
            })
            .clone();
        ui.add_shape(
            Shape::image(handle)
                .fit(ImageFit::Tile {
                    offset: Vec2::ZERO,
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
        shown: &DrawableImage,
    ) {
        let readout = HeaderReadout {
            title,
            shown,
            zoom: self.effective_view(ui, shown, pane).map(|v| v.zoom),
        };
        let text = fmt!(ui, "{readout}");
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
        shown: &DrawableImage,
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
                        && let Some(v) = self.effective_view(ui, shown, Some(pane))
                    {
                        self.view = Some(zoom_about_pane_center(v, 1.0, pane));
                    }
                });
                let appearance = Panel::vstack().id(control_wid(node_id, "pill_appearance"));
                pill(ui, theme, appearance, |ui| {
                    for (mode, key, tip) in BACKDROPS {
                        let selected = prefs.background == mode;
                        if Chip::new(control_wid(node_id, key), tip).show(ui, theme, |ui, s, _| {
                            glyph::draw_swatch(ui, s, theme, mode, selected);
                        }) && !selected
                        {
                            prefs.background = mode;
                            changed = true;
                        }
                    }
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
    fn apply_gestures(&mut self, ui: &Ui, shown: Option<&DrawableImage>, pane: Option<Vec2>) {
        let Some(shown) = shown else {
            return;
        };
        let resp = ui.response_for(pane_wid(self.node_id));
        let Some(pane) = pane else {
            return;
        };
        if resp.left.double_clicked() {
            self.reset_framing();
            return;
        }
        let adjusting =
            resp.left.drag.started() || resp.middle.drag.started() || pan_zoom::scrolled(&resp);
        if self.view.is_none() && !adjusting {
            return;
        }
        let Some(mut v) = self.effective_view(ui, shown, Some(pane)) else {
            return;
        };

        if resp.left.drag.started() || resp.middle.drag.started() {
            self.pan_anchor.latch(v.pan);
        }
        let drag = resp.left.drag.delta().or_else(|| resp.middle.drag.delta());
        pan_zoom::fold_pan_drag(&mut self.pan_anchor, |&start| start, drag, &mut v.pan);
        pan_zoom::fold_scroll_zoom(&mut v, ui, &resp, VIEWER_MIN_ZOOM, VIEWER_MAX_ZOOM);
        self.view = Some(v);
    }
}

/// The viewer's top-left readout: source node, native dimensions and pixel
/// format, then the two conditional clauses — whether the texture is capped
/// below the source, and the current zoom.
///
/// Rendered on demand rather than assembled into a `String`: the header
/// records every frame the pane does, and `fmt!` puts the finished line
/// straight into the record pass's text arena.
#[derive(Debug)]
struct HeaderReadout<'a> {
    /// Never empty — [`node_label`] supplies the fallback.
    title: &'a str,
    shown: &'a DrawableImage,
    /// `None` only while a fit-mode viewer waits for its first arranged
    /// pane — there is no fit zoom to report yet.
    zoom: Option<f32>,
}

impl Display for HeaderReadout<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let DrawableImage {
            handle,
            native_size,
            native_format,
        } = self.shown;
        write!(
            f,
            "{} · {} × {} · {native_format}",
            self.title, native_size.x, native_size.y
        )?;
        if handle.size() != *native_size {
            f.write_str(" · downscaled view")?;
        }
        if let Some(zoom) = self.zoom {
            write!(f, " · {:.0}%", zoom * 100.0)?;
        }
        Ok(())
    }
}

/// Display label for a viewer tab / pane header: the node's name, falling
/// back to "image" for an unnamed node. The one formatter for both the tab
/// strip and the viewer title.
///
/// Borrowed from the document rather than assembled: both readers record
/// every frame, and both arms are text something already owns. Each reader
/// resolves its own — the strip labels the tabs it is drawing, the pane header
/// labels the one tab it is — because one hash probe per label is cheaper than
/// a table keyed by every tab.
pub(crate) fn node_label(doc: &Document, node_id: NodeId) -> &str {
    doc.graph
        .find(node_id)
        .map(|n| n.name.as_str())
        .filter(|n| !n.is_empty())
        .unwrap_or("image")
}

/// A texture's 1:1 logical footprint on the current display — one texel per
/// physical pixel, which is the space every viewport in this module is
/// expressed in, so the framing math never sees raw texels.
fn logical_size(image: &DrawableImage, ui: &Ui) -> Vec2 {
    image.handle.size().as_vec2() / ui.display().scale_factor()
}

/// Stable id for a viewer's pane — keyed by node so switching between two
/// viewer tabs can't cross-feed their gesture responses.
fn pane_wid(node_id: NodeId) -> WidgetId {
    WidgetId::from_hash(("image_viewer.pane", node_id))
}

#[cfg(test)]
mod tests;
