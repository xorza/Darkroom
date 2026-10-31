//! Full-resolution viewers for preview nodes' runtime images, one editor tab
//! per node ([`TabRef::ImageViewer`], deduped on open). Each visible viewer
//! borrows its node's registered texture from the centralized preview
//! store and keeps only navigation state. Opening or restoring a tab therefore
//! shows an already-received value without an editor-driven notification path.
//!
//! The full RGBA8 texture is converted by the store on demand, the first time a
//! viewer asks for it while recording, and the image gives its source up to
//! the conversion. Asking is what scopes it: a pane records only while its tab
//! is the visible one, so a viewer stacked behind another holds no
//! full-resolution texture until it is activated. One activated mid-record
//! frames itself at full size in the pass that puts it on screen, with the
//! thumbnail standing in until the conversion lands.
//!
//! Split the way the graph pane is, by what each part does rather than by what
//! it draws: [`camera`] is the affine algebra between texels, logical px and
//! the pane, [`glyph`] is the drawn vocabulary, and [`toolbar`] is the
//! floating toolbar the panel stamps out. This file is [`ImageViewer`] and
//! nothing else — the state one viewer tab keeps across frames, and the record
//! pass that drives it.
//!
//! [`TabRef::ImageViewer`]: crate::core::document::tab_ref::TabRef::ImageViewer

mod camera;
mod glyph;
mod toolbar;

use scenarium::NodeId;
use std::fmt;
use std::fmt::Display;
use std::fmt::Formatter;

use glam::{UVec2, Vec2};
use palantir::widget::Shape;
use palantir::{
    Align, Chrome, Configure, HAlign, ImageDownsample, ImageFilter, ImageFit, ImageHandle, Panel,
    Sense, Sizing, Spacing, Ui, VAlign, WidgetId, fmt,
};

use crate::core::document::Document;
use crate::core::document::viewport::Viewport;
use crate::core::io::preferences::{ViewerBg, ViewerPreferences};
use crate::gui::frame::app_command::{AppCommand, PreferencesCommand};
use crate::gui::frame::requests::Requests;
use crate::gui::pane::image_viewer::camera::VIEWER_ZOOM;
use crate::gui::pane::image_viewer::glyph::Swatch;
use crate::gui::pane::image_viewer::toolbar::{BACKDROPS, Controls, Readout};
use crate::gui::state::preview_store::error::DrawablePreviewError;
use crate::gui::state::preview_store::{
    DrawableImage, FullView, PreviewMessage, PreviewStore, StoredContent,
};
use crate::gui::state::run_state::ExecStatus;
use crate::gui::theme::Theme;
use crate::gui::widgets::chip::Chip;
use crate::gui::widgets::format;
use crate::gui::widgets::format::percent::Percent;
use crate::gui::widgets::icon::Icon;
use crate::gui::widgets::pan_drag;
use crate::gui::widgets::pill;
use crate::gui::widgets::zoom_range;

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
    /// Pan-drag bookkeeping: the viewport pan at drag start.
    pan_anchor: Option<Vec2>,
    /// Lazily registered checkerboard tile for the `Checker` backdrop.
    /// The backdrop choice and magnification filter live in
    /// [`ViewerPreferences`] — one persisted setting shared by every
    /// viewer tab, threaded into [`Self::show`] each frame.
    checker: Option<ImageHandle>,
}

/// Drawn when a viewer's node has published nothing yet and no run has said why. An invitation
/// rather than a diagnosis, which is why [`ShownSource::Nothing`] is its own state and not a
/// message.
const NOTHING_YET_HINT: &str = "Run the graph to see this image.";

/// Drawn for nothing published when the last run skipped the node: it ran, so the invitation
/// above would say the wrong thing.
const STOPPED_BEFORE_HINT: &str = "The last run stopped before this node.";

/// Drawn for nothing published when the node failed in the last run.
const FAILED_HINT: &str = "This node failed in the last run.";

/// Drawn for nothing published when the last run could not feed the node an input.
const MISSING_INPUT_HINT: &str = "The last run could not feed this node an input.";

/// Drawn for nothing published while the node runs.
const RUNNING_HINT: &str = "Running\u{2026}";

/// Drawn for nothing published after the node ran: its value is still on its way, or it
/// published none.
const NO_IMAGE_YET_HINT: &str = "The last run has published no image yet.";

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
    Image(FullView),
    /// Why this value has no picture. Carries the reason rather than its
    /// wording, so the pane decides how to say it — including for a perfectly
    /// good non-image value, which is [`DrawablePreviewError::NotAnImage`] and
    /// not a fault.
    NoImage(DrawablePreviewError),
}

impl ShownSource {
    /// Read the store's entry for this viewer's node, queueing its
    /// full-resolution conversion if this is the first pass to ask.
    ///
    /// Asking here is what scopes the conversion: this runs from the record of
    /// a pane that is being drawn, so only a viewer actually on screen ever
    /// costs one. The preview card never asks; its thumbnail is already
    /// resident, and stands in until the full texture lands.
    fn resolve(source: Option<&StoredContent>, ui: &Ui) -> Self {
        match source {
            None => Self::Nothing,
            Some(StoredContent::Image(image)) => match image.full(ui) {
                Ok(view) => Self::Image(view),
                Err(error) => Self::NoImage(error),
            },
            // A stored failure keeps its own reason. A perfectly good
            // non-image value is `NotAnImage`: "7" is not what a viewer tab is
            // for, and the preview card is what renders it.
            Some(StoredContent::Error(error)) => Self::NoImage(error.clone()),
            Some(StoredContent::Text(_)) => Self::NoImage(DrawablePreviewError::NotAnImage),
        }
    }

    const fn image(&self) -> Option<&FullView> {
        match self {
            Self::Image(view) => Some(view),
            Self::Nothing | Self::NoImage(_) => None,
        }
    }

    /// The line drawn in place of an image; `status` says why nothing was published.
    const fn hint(&self, status: &ExecStatus) -> Option<PreviewMessage<'_>> {
        match self {
            Self::Image(_) => None,
            Self::NoImage(error) => Some(PreviewMessage::Failure(error)),
            Self::Nothing => Some(PreviewMessage::Text(match status {
                ExecStatus::None => NOTHING_YET_HINT,
                ExecStatus::Running(_) => RUNNING_HINT,
                ExecStatus::Executed(_) | ExecStatus::Cached => NO_IMAGE_YET_HINT,
                ExecStatus::MissingInputs(_) => MISSING_INPUT_HINT,
                ExecStatus::Skipped => STOPPED_BEFORE_HINT,
                ExecStatus::Errored(_) => FAILED_HINT,
            })),
        }
    }
}

/// The pane a viewer draws into this frame, and what it draws from.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ViewerPane<'a> {
    pub(crate) theme: &'a Theme,
    /// The tab's title, never empty — [`node_label`] supplies the fallback.
    pub(crate) title: &'a str,
    pub(crate) previews: &'a PreviewStore,
    /// The node's last-run status, which says why a viewer with nothing to show has nothing.
    pub(crate) status: &'a ExecStatus,
    /// The pane's size, `None` before it was ever arranged.
    pub(crate) size: Option<Vec2>,
}

/// What a viewer writes this frame: the shared viewer preferences its controls edit, and the
/// queue their change is reported on.
#[derive(Debug)]
pub(crate) struct ViewerWrites<'a> {
    pub(crate) preferences: &'a mut ViewerPreferences,
    pub(crate) out: &'a mut Requests,
}

/// A frame that has an image to show, in its pane.
#[derive(Debug, Clone, Copy)]
struct Shown<'a> {
    pane: ViewerPane<'a>,
    view: &'a FullView,
}

impl ImageViewer {
    /// An empty viewer for `node_id` (shows the hint until content arrives).
    pub(crate) const fn new(node_id: NodeId) -> Self {
        Self {
            node_id,
            source_size: None,
            view: None,
            pan_anchor: None,
            checker: None,
        }
    }

    const fn reset_framing(&mut self) {
        self.view = None;
        self.pan_anchor = None;
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
    /// [`camera::fit_viewport`] is defined to match it — so nothing is owed a later
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
            .or_else(|| Some(camera::fit_viewport(logical_size(image, ui), pane?)))
    }

    fn sync_source(&mut self, source_size: Option<UVec2>) {
        if source_size != self.source_size {
            self.reset_framing();
            self.source_size = source_size;
        }
    }

    /// Draw the viewer into `view` (the whole tab content). Borrows the
    /// centralized texture, applies last frame's pan/zoom gestures, then
    /// paints the image (or message), header, and controls. A change to the shared viewer
    /// preferences queues [`PreferencesCommand::Changed`], as the Preferences tab's does.
    pub(crate) fn show(
        &mut self,
        ui: &mut Ui,
        pane_view: ViewerPane<'_>,
        writes: ViewerWrites<'_>,
    ) {
        let ViewerWrites { preferences, out } = writes;
        let ViewerPane {
            theme,
            previews,
            status,
            size: pane,
            ..
        } = pane_view;
        // The store rather than the entry: this viewer *is* its node, so
        // handing it the whole store is one lookup by an id that cannot
        // disagree with the one keying the pane's widgets.
        //
        // Resolving is also what queues a not-yet-converted full-resolution
        // texture. The thumbnail stands in until it lands, and the converter
        // wakes the host when it does, so the pane owes itself no later frame.
        let source = ShownSource::resolve(previews.get(self.node_id), ui);
        self.sync_source(source.image().map(|view| view.image().texels));
        self.apply_gestures(ui, source.image().map(FullView::image), pane);

        // The checker draws over the theme's backdrop.
        let fill = glyph::flat_fill(theme, preferences.bg).unwrap_or(theme.canvas.bg);
        let mut preferences_changed = false;
        Panel::zstack()
            .id(pane_wid(self.node_id))
            .size((Sizing::FILL, Sizing::FILL))
            .sense(Sense::CLICK | Sense::DRAG | Sense::SCROLL | Sense::PINCH)
            .clip_rect()
            .chrome(Chrome::fill(fill))
            .show(ui, |ui| {
                if preferences.bg == ViewerBg::Checker
                    && let Some(pane) = pane
                {
                    self.draw_checker(ui, theme, pane);
                }
                if let Some(view) = source.image() {
                    let shown = view.image();
                    let image = Shape::image(shown.handle.clone())
                        .min_filter(ImageFilter::Linear)
                        .mag_filter(preferences.mag_filter)
                        // Area averaging preserves source brightness when minifying.
                        .downsample(ImageDownsample::Mean);
                    ui.add_shape(match self.effective_view(ui, shown, pane) {
                        Some(v) => image
                            .at(camera::draw_rect(logical_size(shown, ui), v))
                            .fit(ImageFit::Fill),
                        // No framing yet — no zoom of its own and no pane to
                        // fit into. Palantir's own contain draws exactly what
                        // the fit would.
                        None => image.fit(ImageFit::Contain),
                    });
                    let shown = Shown {
                        pane: pane_view,
                        view,
                    };
                    self.header(ui, shown);
                    preferences_changed = self.controls(ui, shown, preferences);
                }
                // Complementary to the image above, so this is the same
                // either/or the enum states — never a second thing drawn over
                // the picture. On the frosted readout pill, so the line stays
                // legible over the checker/white backdrops too.
                if let Some(hint) = source.hint(status) {
                    // A failure reads in the error ink; every other line invites, muted.
                    let ink = match hint {
                        PreviewMessage::Failure(_) => theme.status.error,
                        PreviewMessage::Text(_) => theme.ink.muted,
                    };
                    let max_width = pane_view
                        .size
                        .map_or(f32::INFINITY, |pane| pane.x - 2.0 * theme.space.lg);
                    Readout {
                        panel: Panel::hstack().id_salt("viewer_hint").align(Align::CENTER),
                        text: fmt!(ui, "{hint}").into(),
                        ink,
                        max_width,
                    }
                    .show(ui, theme);
                }
            });
        if preferences_changed {
            out.push_app(AppCommand::Preferences(PreferencesCommand::Changed));
        }
    }

    /// The screen-fixed checkerboard backdrop across the whole pane. One
    /// tiled 2×2 texture; `Nearest` keeps the squares crisp at any pane
    /// size and DPI.
    fn draw_checker(&mut self, ui: &mut Ui, theme: &Theme, pane: Vec2) {
        let handle = self
            .checker
            .get_or_insert_with(|| {
                ui.load_image(&glyph::checker_image(theme))
                    .expect("checker image fits every supported GPU")
            })
            .clone();
        ui.add_shape(
            Shape::image(handle)
                .fit(ImageFit::Tile {
                    offset: Vec2::ZERO,
                    scale: pane / (2.0 * theme.viewer.checker_square),
                })
                .min_filter(ImageFilter::Nearest)
                .mag_filter(ImageFilter::Nearest),
        );
    }

    /// The top-left readout: source node, native dimensions and pixel
    /// format, whether the view is texture-capped or still converting, and the
    /// current zoom.
    fn header(&self, ui: &mut Ui, shown: Shown<'_>) {
        let readout = HeaderReadout {
            title: shown.pane.title,
            shown: shown.view,
            zoom: self
                .effective_view(ui, shown.view.image(), shown.pane.size)
                .map(|v| v.zoom),
        };
        let theme = shown.pane.theme;
        let margin = theme.space.sm;
        // Cut before the control panel, a gap short of it, as it stood last frame.
        let controls = ui
            .response_for(toolbar::controls_wid(self.node_id, "panel"))
            .rect
            .map_or(0.0, |rect| rect.size.w);
        let max_width = shown
            .pane
            .size
            .map_or(f32::INFINITY, |pane| pane.x - 3.0 * margin - controls);
        Readout {
            panel: Panel::hstack()
                .id(header_wid(self.node_id))
                .margin(Spacing::new(margin, margin, 0.0, 0.0)),
            text: fmt!(ui, "{readout}").into(),
            ink: theme.ink.muted,
            max_width,
        }
        .show(ui, theme);
    }

    /// The floating control panel in the pane's top-right corner — the
    /// viewer twin of the graph toolbar: function groups on stacked
    /// frosted pills, opaque chip buttons raised off each pill. The top
    /// pill frames the view (fit, 100%); the column below edits the
    /// shared appearance preferences — the backdrop radio stack and,
    /// past a rule, the sampling toggle. Returns `true` when `preferences`
    /// changed. Drawn after the image so the buttons hit-test above the
    /// pane's gesture surface. Framing clicks land next frame (responses
    /// lag the record by one frame) — imperceptible.
    fn controls(
        &mut self,
        ui: &mut Ui,
        shown: Shown<'_>,
        preferences: &mut ViewerPreferences,
    ) -> bool {
        let (theme, pane) = (shown.pane.theme, shown.pane.size);
        let controls = Controls {
            node_id: self.node_id,
            theme,
        };
        let mut changed = false;
        Panel::vstack()
            .id(controls.wid("panel"))
            .size((Sizing::HUG, Sizing::HUG))
            .align(Align::new(HAlign::Right, VAlign::Top))
            .child_align(Align::new(HAlign::Right, VAlign::Top))
            .margin(Spacing::new(0.0, theme.space.sm, theme.space.sm, 0.0))
            .gap(theme.space.sm)
            .show(ui, |ui| {
                let framing = Panel::hstack().id(controls.wid("pill_framing"));
                pill::dress(framing, theme).show(ui, |ui| {
                    if Chip::new(controls.wid("fit"), "Fit to view").show_icon(ui, theme, Icon::Fit)
                    {
                        self.reset_framing();
                    }
                    if Chip::new(controls.wid("100"), "Zoom to 100%").show_icon(
                        ui,
                        theme,
                        Icon::ActualSize,
                    ) && let Some(pane) = pane
                        && let Some(v) = self.effective_view(ui, shown.view.image(), Some(pane))
                    {
                        self.view = Some(camera::zoom_about_pane_center(v, 1.0, pane));
                    }
                });
                let appearance = Panel::vstack().id(controls.wid("pill_appearance"));
                pill::dress(appearance, theme).show(ui, |ui| {
                    for (mode, key, tooltip) in BACKDROPS {
                        let selected = preferences.bg == mode;
                        if Chip::new(controls.wid(key), tooltip).show(ui, theme, |ui, s, _| {
                            Swatch { mode, selected }.draw(ui, s, theme);
                        }) && !selected
                        {
                            preferences.bg = mode;
                            changed = true;
                        }
                    }
                    pill::rule(ui, theme);
                    changed |= controls.filter_toggle(ui, &mut preferences.mag_filter);
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
            resp.left.drag.started() || resp.middle.drag.started() || zoom_range::scrolled(&resp);
        if self.view.is_none() && !adjusting {
            return;
        }
        let Some(mut v) = self.effective_view(ui, shown, Some(pane)) else {
            return;
        };

        if resp.left.drag.started() || resp.middle.drag.started() {
            self.pan_anchor = Some(v.pan);
        }
        let drag = resp.left.drag.delta().or_else(|| resp.middle.drag.delta());
        pan_drag::fold(&mut self.pan_anchor, drag, &mut v.pan);
        VIEWER_ZOOM.fold_scroll(&mut v, ui, &resp);
        self.view = Some(v);
    }
}

/// The viewer's top-left readout: source node, native dimensions and pixel
/// format, then the conditional clauses — whether the texture is capped below
/// the source, whether the full texture is still converting, and the zoom.
///
/// Rendered on demand rather than assembled into a `String`: the header
/// records every frame the pane does, and `fmt!` puts the finished line
/// straight into the record pass's text arena.
#[derive(Debug)]
struct HeaderReadout<'a> {
    /// Never empty — [`node_label`] supplies the fallback.
    title: &'a str,
    shown: &'a FullView,
    /// `None` only while a fit-mode viewer waits for its first arranged
    /// pane — there is no fit zoom to report yet.
    zoom: Option<f32>,
}

impl Display for HeaderReadout<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let DrawableImage {
            texels,
            native_size,
            native_format,
            ..
        } = self.shown.image();
        write!(
            f,
            "{} · {} × {} · {native_format}",
            self.title, native_size.x, native_size.y
        )?;
        if texels != native_size {
            f.write_str(" · downscaled view")?;
        }
        if matches!(self.shown, FullView::Converting(_)) {
            f.write_str(" · preparing full resolution")?;
        }
        if let Some(zoom) = self.zoom {
            write!(f, " · {}", Percent::new(zoom))?;
        }
        Ok(())
    }
}

/// Display label for a viewer tab / pane header: the node's name through
/// [`format::node_name`]. The one formatter for both the tab strip and the viewer title.
///
/// Borrowed from the document rather than assembled: both readers record
/// every frame, and both arms are text something already owns. Each reader
/// resolves its own — the strip labels the tabs it is drawing, the pane header
/// labels the one tab it is — because one hash probe per label is cheaper than
/// a table keyed by every tab.
pub(crate) fn node_label(document: &Document, node_id: NodeId) -> &str {
    let node = document
        .graph
        .find(node_id)
        .expect("a viewer tab's node is in the graph");
    format::node_name(&node.name)
}

/// A texture's 1:1 logical footprint on the current display — one texel per
/// physical pixel, which is the space every viewport in this module is
/// expressed in, so the framing math never sees raw texels.
fn logical_size(image: &DrawableImage, ui: &Ui) -> Vec2 {
    image.texels.as_vec2() / ui.display().scale_factor()
}

/// The header readout of `node_id`'s viewer.
fn header_wid(node_id: NodeId) -> WidgetId {
    WidgetId::from_hash(("darkroom.image_viewer.header", node_id))
}

/// Stable id for a viewer's pane — keyed by node so switching between two
/// viewer tabs can't cross-feed their gesture responses.
fn pane_wid(node_id: NodeId) -> WidgetId {
    WidgetId::from_hash(("darkroom.image_viewer.pane", node_id))
}

#[cfg(test)]
mod tests;
