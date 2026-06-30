//! The floating toolbar pinned to the graph view's top-left corner: a
//! run/cancel toggle and an event-loop start/stop toggle. Both are square
//! glyph buttons that paint "toggled" while their action is in flight, and a
//! click maps to the matching [`AppCommand`] (reused by `App`).

use glam::Vec2;
use palantir::{
    Align, Background, Color, Configure, Corners, HAlign, Mesh, Panel, Rect, Sense, Shape, Sizing,
    Spacing, Stroke, Ui, VAlign, WidgetId,
};

use crate::gui::app::AppCommand;
use crate::gui::app::AppContext;
use crate::gui::theme::Theme;

/// Side of each square button, in px.
const BUTTON_SIZE: f32 = 30.0;
/// Inset of the toolbar from the graph view's top-left corner.
const TOOLBAR_MARGIN: f32 = 8.0;
/// Gap between buttons.
const BUTTON_GAP: f32 = 6.0;
/// Corner radius of a button's rounded-rect background.
const BUTTON_RADIUS: f32 = 6.0;

fn run_button_wid() -> WidgetId {
    WidgetId::from_hash("darkroom.graph.run_button")
}

fn events_button_wid() -> WidgetId {
    WidgetId::from_hash("darkroom.graph.events_button")
}

/// Draw the toolbar over the graph view's top-left corner and return the
/// [`AppCommand`] any click implies. It hit-tests above the canvas (drawn
/// after it), so a click on a button never starts a pan.
pub(crate) fn show(ui: &mut Ui, ctx: &AppContext<'_>) -> Option<AppCommand> {
    let mut command = None;
    Panel::hstack()
        .id_salt("graph_toolbar")
        .size((Sizing::Hug, Sizing::Hug))
        .align(Align::new(HAlign::Left, VAlign::Top))
        .margin(Spacing::new(TOOLBAR_MARGIN, TOOLBAR_MARGIN, 0.0, 0.0))
        .gap(BUTTON_GAP)
        .show(ui, |ui| {
            // Run / cancel: toggled while a one-shot run is in flight.
            let running = ctx.run_state.is_running();
            if toggle_button(ui, ctx.theme, run_button_wid(), running, draw_play) {
                command = Some(if running {
                    AppCommand::CancelRun
                } else {
                    AppCommand::Run
                });
            }
            // Event loop start / stop: toggled while the loop runs.
            if toggle_button(
                ui,
                ctx.theme,
                events_button_wid(),
                ctx.events_running,
                draw_play_bar,
            ) {
                command = Some(if ctx.events_running {
                    AppCommand::StopEvents
                } else {
                    AppCommand::StartEvents
                });
            }
        });
    command
}

/// One square glyph toggle. `toggled` inverts it like a filled chip (the
/// running-glow fill with a dark glyph); idle is a neutral fill (lighter on
/// hover) with the green "go" glyph. `draw_glyph` paints the icon centered in
/// the `BUTTON_SIZE` box in the resolved glyph color. Returns whether it was
/// clicked this frame.
fn toggle_button(
    ui: &mut Ui,
    theme: &Theme,
    wid: WidgetId,
    toggled: bool,
    draw_glyph: impl FnOnce(&mut Ui, f32, Color),
) -> bool {
    let hovered = ui.response_for(wid).hovered;
    let (fill, glyph) = if toggled {
        (theme.exec_running_glow, theme.chrome_fill)
    } else if hovered {
        (theme.header_fill, theme.exec_executed_glow)
    } else {
        (theme.node_fill, theme.exec_executed_glow)
    };
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
    button.response.clicked()
}

/// A right-pointing play triangle (run once), optically centered in the box.
fn draw_play(ui: &mut Ui, s: f32, color: Color) {
    let tri = Mesh::filled_triangle(
        Vec2::new(s * 0.38, s * 0.30),
        Vec2::new(s * 0.38, s * 0.70),
        Vec2::new(s * 0.70, s * 0.50),
        color,
    );
    ui.add_shape(Shape::Mesh {
        mesh: &tri,
        local_rect: None,
        tint: Color::WHITE.into(),
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
    let tri = Mesh::filled_triangle(
        Vec2::new(s * 0.46, s * 0.30),
        Vec2::new(s * 0.46, s * 0.70),
        Vec2::new(s * 0.74, s * 0.50),
        color,
    );
    ui.add_shape(Shape::Mesh {
        mesh: &tri,
        local_rect: None,
        tint: Color::WHITE.into(),
    });
}
