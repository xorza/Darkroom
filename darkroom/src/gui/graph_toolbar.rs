//! The floating toolbar pinned to the graph view's top-left corner: a
//! run/cancel toggle and an event-loop start/stop toggle side by side on one
//! chrome pill, with three view-framing buttons (reset view, show all, show
//! selected) stacked beneath on a second pill. The frosted pills keep the
//! toolbar legible over both the canvas and any node under it; the buttons
//! themselves are transparent glyphs until hovered/toggled. All carry hover
//! tooltips; the toggles paint "toggled" while their action is in flight and
//! map to an [`AppCommand`], while the framing buttons emit an
//! `Intent::SetViewport` directly.

use glam::Vec2;
use palantir::{
    Align, Background, Color, Configure, Corners, HAlign, Panel, Rect, Sense, Shape, Sizing,
    Spacing, Stroke, Tooltip, Ui, VAlign, WidgetId,
};

use crate::core::edit::intent::Intent;
use crate::gui::app::AppCommand;
use crate::gui::app::AppContext;
use crate::gui::canvas::pan_zoom::{self, ViewAction};
use crate::gui::scene::Scene;
use crate::gui::theme::Theme;

/// Side of each square button, in px.
const BUTTON_SIZE: f32 = 30.0;
/// Inset of the toolbar from the graph view's top-left corner.
const TOOLBAR_MARGIN: f32 = 8.0;
/// Gap between buttons.
const BUTTON_GAP: f32 = 6.0;
/// Corner radius of a button's rounded-rect background.
const BUTTON_RADIUS: f32 = 6.0;
/// Opacity of a group pill's frosted chrome backdrop. Keeps the toolbar
/// readable over an empty canvas *and* over a node it happens to sit on — the
/// backdrop color sits between the canvas and node fills, so a bit of
/// translucency still contrasts against both while the node stays faintly
/// visible through it.
const PILL_BG_ALPHA: f32 = 0.7;
/// Padding between a group pill's chrome edge and the buttons inside it.
const PILL_PADDING: f32 = 4.0;
/// Corner radius of a group pill's chrome backdrop — the button radius grown
/// by the padding so the pill's rounding stays concentric with the buttons'.
const PILL_RADIUS: f32 = BUTTON_RADIUS + PILL_PADDING;

fn run_button_wid() -> WidgetId {
    WidgetId::from_hash("darkroom.graph.run_button")
}

fn events_button_wid() -> WidgetId {
    WidgetId::from_hash("darkroom.graph.events_button")
}

fn reset_view_wid() -> WidgetId {
    WidgetId::from_hash("darkroom.graph.reset_view_button")
}

fn show_all_wid() -> WidgetId {
    WidgetId::from_hash("darkroom.graph.show_all_button")
}

fn show_selected_wid() -> WidgetId {
    WidgetId::from_hash("darkroom.graph.show_selected_button")
}

/// Draw the toolbar over the graph view's top-left corner. Returns the
/// [`AppCommand`] a run/events click implies; view-framing clicks push an
/// `Intent::SetViewport` onto `out` instead. It hit-tests above the canvas
/// (drawn after it), so a click on a button never starts a pan.
pub(crate) fn show(
    ui: &mut Ui,
    ctx: &AppContext<'_>,
    scene: &Scene,
    out: &mut Vec<Intent>,
) -> Option<AppCommand> {
    let mut command = None;
    Panel::vstack()
        .id_salt("graph_toolbar")
        .size((Sizing::Hug, Sizing::Hug))
        .align(Align::new(HAlign::Left, VAlign::Top))
        .child_align(Align::new(HAlign::Left, VAlign::Top))
        .margin(Spacing::new(TOOLBAR_MARGIN, TOOLBAR_MARGIN, 0.0, 0.0))
        .gap(BUTTON_GAP)
        .show(ui, |ui| {
            // Top row: run/cancel + event-loop toggles, side by side on their
            // own chrome pill.
            Panel::hstack()
                .id_salt("graph_toolbar_run")
                .size((Sizing::Hug, Sizing::Hug))
                .gap(BUTTON_GAP)
                .padding(Spacing::all(PILL_PADDING))
                .background(pill_background(ctx.theme))
                .show(ui, |ui| {
                    // Run / cancel: toggled while a one-shot run is in flight.
                    let running = ctx.run_state.is_running();
                    let run_tip = if running { "Cancel run" } else { "Run" };
                    if toggle_button(ui, ctx.theme, run_button_wid(), running, run_tip, draw_play) {
                        command = Some(if running {
                            AppCommand::CancelRun
                        } else {
                            AppCommand::Run
                        });
                    }
                    // Event loop start / stop: toggled while the loop runs.
                    let events_tip = if ctx.events_running {
                        "Stop events"
                    } else {
                        "Start events"
                    };
                    if toggle_button(
                        ui,
                        ctx.theme,
                        events_button_wid(),
                        ctx.events_running,
                        events_tip,
                        draw_play_bar,
                    ) {
                        command = Some(if ctx.events_running {
                            AppCommand::StopEvents
                        } else {
                            AppCommand::StartEvents
                        });
                    }
                });
            // View-framing actions, stacked under the run row on their own
            // chrome pill. Each emits a `SetViewport` intent (undoable), so they
            // ride the same path as a manual pan/zoom rather than mutating the
            // viewport out of band.
            Panel::vstack()
                .id_salt("graph_toolbar_framing")
                .size((Sizing::Hug, Sizing::Hug))
                .child_align(Align::new(HAlign::Left, VAlign::Top))
                .gap(BUTTON_GAP)
                .padding(Spacing::all(PILL_PADDING))
                .background(pill_background(ctx.theme))
                .show(ui, |ui| {
                    if action_button(ui, ctx.theme, reset_view_wid(), "Reset view", draw_reset) {
                        out.extend(pan_zoom::view_action_intent(ui, scene, ViewAction::Reset));
                    }
                    if action_button(ui, ctx.theme, show_all_wid(), "Show all", draw_show_all) {
                        out.extend(pan_zoom::view_action_intent(ui, scene, ViewAction::ShowAll));
                    }
                    if action_button(
                        ui,
                        ctx.theme,
                        show_selected_wid(),
                        "Show selected",
                        draw_show_selected,
                    ) {
                        out.extend(pan_zoom::view_action_intent(
                            ui,
                            scene,
                            ViewAction::ShowSelected,
                        ));
                    }
                });
        });
    command
}

/// One square glyph toggle sitting on the group pill. `toggled` fills it like a
/// solid chip (the running-glow fill with a dark glyph); hover lifts a lighter
/// patch out of the pill; idle is transparent so the pill shows through. The
/// glyph is the green "go" color except when toggled. Returns whether clicked.
fn toggle_button(
    ui: &mut Ui,
    theme: &Theme,
    wid: WidgetId,
    toggled: bool,
    tip: &'static str,
    draw_glyph: impl FnOnce(&mut Ui, f32, Color),
) -> bool {
    let hovered = ui.response_for(wid).hovered;
    let (fill, glyph) = if toggled {
        (theme.exec_running_glow, theme.chrome_fill)
    } else if hovered {
        (theme.header_fill, theme.exec_executed_glow)
    } else {
        (Color::TRANSPARENT, theme.exec_executed_glow)
    };
    glyph_button(ui, wid, fill, glyph, tip, draw_glyph)
}

/// One square momentary button (no toggled state) sitting on the group pill:
/// transparent until hover, when it lifts a lighter patch out of the pill.
/// Muted glyph. Used by the view-framing actions. Returns whether it was clicked.
fn action_button(
    ui: &mut Ui,
    theme: &Theme,
    wid: WidgetId,
    tip: &'static str,
    draw_glyph: impl FnOnce(&mut Ui, f32, Color),
) -> bool {
    let hovered = ui.response_for(wid).hovered;
    let fill = if hovered {
        theme.header_fill
    } else {
        Color::TRANSPARENT
    };
    glyph_button(ui, wid, fill, theme.text_muted, tip, draw_glyph)
}

/// The frosted chrome backdrop shared by both toolbar group pills.
fn pill_background(theme: &Theme) -> Background {
    Background {
        fill: theme.chrome_fill.with_alpha(PILL_BG_ALPHA).into(),
        corners: Corners::all(PILL_RADIUS),
        ..Default::default()
    }
}

/// Shared square-button body: a `fill` rounded-rect background, the icon
/// painted centered in the `BUTTON_SIZE` box by `draw_glyph` in `glyph`,
/// and a hover `tip`. Returns whether it was clicked this frame.
fn glyph_button(
    ui: &mut Ui,
    wid: WidgetId,
    fill: Color,
    glyph: Color,
    tip: &'static str,
    draw_glyph: impl FnOnce(&mut Ui, f32, Color),
) -> bool {
    let s = BUTTON_SIZE;
    let button = Panel::zstack()
        .id(wid)
        .size((Sizing::Fixed(s), Sizing::Fixed(s)))
        .sense(Sense::CLICK)
        .background(Background {
            fill: fill.into(),
            corners: Corners::all(BUTTON_RADIUS),
            ..Default::default()
        })
        .show(ui, |ui| draw_glyph(ui, s, glyph));
    // Take the owned snapshot + click result so the button's `ui` borrow
    // ends before the tooltip records into `ui`.
    let snapshot = button.response.snapshot();
    let clicked = button.response.clicked();
    Tooltip::for_(&snapshot).text(tip).show(ui);
    clicked
}

/// A right-pointing play triangle (run once), optically centered in the box.
fn draw_play(ui: &mut Ui, s: f32, color: Color) {
    ui.add_shape(Shape::Triangle {
        a: Vec2::new(s * 0.38, s * 0.30),
        b: Vec2::new(s * 0.38, s * 0.70),
        c: Vec2::new(s * 0.70, s * 0.50),
        radius: 0.0,
        fill: color.into(),
        stroke: Stroke::ZERO,
    });
}

/// `|>` — a vertical bar then a play triangle (start the event loop).
fn draw_play_bar(ui: &mut Ui, s: f32, color: Color) {
    // The bar.
    ui.add_shape(Shape::RoundedRect {
        local_rect: Some(Rect::new(s * 0.28, s * 0.30, s * 0.085, s * 0.40)),
        corners: Corners::all(1.0),
        fill: color.into(),
        stroke: Stroke::ZERO,
    });
    // The play triangle, just to its right.
    ui.add_shape(Shape::Triangle {
        a: Vec2::new(s * 0.46, s * 0.30),
        b: Vec2::new(s * 0.46, s * 0.70),
        c: Vec2::new(s * 0.74, s * 0.50),
        radius: 0.0,
        fill: color.into(),
        stroke: Stroke::ZERO,
    });
}

/// Reset view: a target ring with a center dot (recenter to 1:1).
fn draw_reset(ui: &mut Ui, s: f32, color: Color) {
    let d = s * 0.52;
    let o = (s - d) * 0.5;
    stroked_rect(ui, Rect::new(o, o, d, d), d * 0.5, color, s * 0.06);
    dot(ui, s * 0.5, s * 0.5, s * 0.075, color);
}

/// Show all: a frame enclosing a 2×2 field of dots (fit every node).
fn draw_show_all(ui: &mut Ui, s: f32, color: Color) {
    frame(ui, s, color);
    let r = s * 0.055;
    let near = s * 0.5 - s * 0.11;
    let far = s * 0.5 + s * 0.11;
    for &cy in &[near, far] {
        for &cx in &[near, far] {
            dot(ui, cx, cy, r, color);
        }
    }
}

/// Show selected: a frame enclosing one filled square (fit the selection).
fn draw_show_selected(ui: &mut Ui, s: f32, color: Color) {
    frame(ui, s, color);
    let inner = s * 0.24;
    let o = (s - inner) * 0.5;
    ui.add_shape(Shape::RoundedRect {
        local_rect: Some(Rect::new(o, o, inner, inner)),
        corners: Corners::all(s * 0.04),
        fill: color.into(),
        stroke: Stroke::ZERO,
    });
}

/// The shared rounded-rect outline both fit-buttons frame their contents in.
fn frame(ui: &mut Ui, s: f32, color: Color) {
    let w = s * 0.62;
    let o = (s - w) * 0.5;
    stroked_rect(ui, Rect::new(o, o, w, w), s * 0.08, color, s * 0.06);
}

/// A rounded-rect outline (transparent fill, `color` stroke of `width`).
fn stroked_rect(ui: &mut Ui, rect: Rect, radius: f32, color: Color, width: f32) {
    ui.add_shape(Shape::RoundedRect {
        local_rect: Some(rect),
        corners: Corners::all(radius),
        fill: Color::TRANSPARENT.into(),
        stroke: Stroke::solid(color, width),
    });
}

/// A small filled circle of radius `r` centered at `(cx, cy)`.
fn dot(ui: &mut Ui, cx: f32, cy: f32, r: f32, color: Color) {
    ui.add_shape(Shape::RoundedRect {
        local_rect: Some(Rect::new(cx - r, cy - r, 2.0 * r, 2.0 * r)),
        corners: Corners::all(r),
        fill: color.into(),
        stroke: Stroke::ZERO,
    });
}
