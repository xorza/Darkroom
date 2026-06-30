//! The floating run/cancel toggle pinned to the graph view's top-left corner.

use glam::Vec2;
use palantir::{
    Align, Background, Color, Configure, Corners, HAlign, Mesh, Panel, Sense, Shape, Sizing,
    Spacing, Ui, VAlign, WidgetId,
};

use crate::gui::app::AppContext;
use crate::gui::menu_bar::MenuCommand;

/// Side of the square button, in px.
const BUTTON_SIZE: f32 = 30.0;
/// Inset from the graph view's top-left corner.
const BUTTON_MARGIN: f32 = 8.0;
/// Corner radius of the button's rounded-rect background.
const BUTTON_RADIUS: f32 = 6.0;

/// Stable id so the hover state survives across frames.
fn run_button_wid() -> WidgetId {
    WidgetId::from_hash("darkroom.graph.run_button")
}

/// A run/cancel toggle: a play triangle that paints "toggled" (the
/// running-glow color, with an inverted glyph) while the graph executes,
/// and a neutral fill with the green "go" glyph at rest. A click runs the
/// graph, or — while running — cancels it. Returns the [`MenuCommand`] the
/// click implies, reusing the menu bar's run path. Overlaid on the graph
/// view's top-left corner by [`crate::gui::main_window`].
pub(crate) fn show(ui: &mut Ui, ctx: &AppContext<'_>, running: bool) -> Option<MenuCommand> {
    let theme = ctx.theme;
    let wid = run_button_wid();
    let hovered = ui.response_for(wid).hovered;

    // Toggled (running) inverts like a filled chip — running-glow fill, dark
    // glyph — so it reads as "active, click to cancel". Idle is a neutral fill
    // (lighter on hover) with the green "go" glyph.
    let (fill, glyph) = if running {
        (theme.exec_running_glow, theme.chrome_fill)
    } else if hovered {
        (theme.header_fill, theme.exec_executed_glow)
    } else {
        (theme.node_fill, theme.exec_executed_glow)
    };

    // Right-pointing play triangle, optically centered in the button box.
    let s = BUTTON_SIZE;
    let tri = Mesh::filled_triangle(
        Vec2::new(s * 0.38, s * 0.30),
        Vec2::new(s * 0.38, s * 0.70),
        Vec2::new(s * 0.70, s * 0.50),
        glyph,
    );
    let button = Panel::zstack()
        .id(wid)
        .size((Sizing::Fixed(s), Sizing::Fixed(s)))
        .align(Align::new(HAlign::Left, VAlign::Top))
        .margin(Spacing::new(BUTTON_MARGIN, BUTTON_MARGIN, 0.0, 0.0))
        .sense(Sense::CLICK)
        .background(Background {
            fill: fill.into(),
            corners: Corners::all(BUTTON_RADIUS),
            ..Default::default()
        })
        .show(ui, |ui| {
            ui.add_shape(Shape::Mesh {
                mesh: &tri,
                local_rect: None,
                tint: Color::WHITE.into(),
            });
        });
    let command = if running {
        MenuCommand::CancelRun
    } else {
        MenuCommand::Run
    };
    button.response.clicked().then_some(command)
}
