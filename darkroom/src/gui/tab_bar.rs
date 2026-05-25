//! The graph-pane tab strip. Renders one chip per open graph (the
//! root + any opened subgraph interiors), highlights the active one,
//! and raises [`UiAction`]s for activate / close. Pure view state — it
//! never touches the document.

use palantir::{
    Align, Background, Configure, Corners, InternedStr, Panel, Sense, Sizing, Spacing, Text,
    TextStyle, Ui, VAlign, WidgetId,
};

use crate::gui::UiAction;
use crate::theme::Theme;

/// One tab's display state, built by `App` from its open-tab list.
pub struct TabLabel {
    pub text: InternedStr,
    /// `false` for the always-present `Main` tab.
    pub closable: bool,
}

/// Draw the strip and push any activate/close actions onto `out`.
pub fn show(ui: &mut Ui, theme: &Theme, tabs: &[TabLabel], active: usize, out: &mut Vec<UiAction>) {
    // The strip shares the menu bar's `chrome_fill`; the active tab
    // below punches through to `canvas_bg` so it reads as one piece with
    // the graph.
    Panel::hstack()
        .id_salt("tab_bar")
        .size((Sizing::FILL, Sizing::Hug))
        .padding(Spacing::new(6.0, 4.0, 6.0, 0.0))
        .gap(3.0)
        .child_align(Align::v(VAlign::Bottom))
        .background(Background {
            fill: theme.chrome_fill.into(),
            ..Default::default()
        })
        .show(ui, |ui| {
            for (i, tab) in tabs.iter().enumerate() {
                tab_chip(ui, theme, tab, i, i == active, out);
            }
        });
}

fn tab_chip(
    ui: &mut Ui,
    theme: &Theme,
    tab: &TabLabel,
    index: usize,
    active: bool,
    out: &mut Vec<UiAction>,
) {
    let r = theme.header_corner_radius;
    // Active tab takes the graph's color so its bottom edge dissolves
    // into the canvas; inactive tabs stay flat on the chrome strip
    // (transparent fill = just a label) so the active one reads clearly.
    let background = if active {
        Background {
            fill: theme.canvas_bg.into(),
            corners: Corners::new(r, r, 0.0, 0.0),
            ..Default::default()
        }
    } else {
        Background::default()
    };
    // A closable tab trades right inset for the top-right close button
    // (equal 4px top/right gaps); a tab without one (Main) stays
    // symmetric so its label is centered.
    let padding = if tab.closable {
        Spacing::new(10.0, 4.0, 4.0, 4.0)
    } else {
        Spacing::xy(10.0, 4.0)
    };
    let mut close_clicked = false;
    let chip = Panel::hstack()
        .id_salt(("tab", index))
        .size((Sizing::Hug, Sizing::Hug))
        .sense(Sense::CLICK)
        .padding(padding)
        .gap(6.0)
        .child_align(Align::v(VAlign::Center))
        .background(background)
        .show(ui, |ui| {
            // Match the menu bar's smaller (13px) label scale.
            Text::new(tab.text.clone())
                .style(TextStyle {
                    font_size_px: 13.0,
                    ..ui.theme.text
                })
                .show(ui);
            if tab.closable {
                let close_wid = WidgetId::from_hash(("graph.tab_close", index));
                // Hover comes from last frame's response; paint a subtle
                // highlight chip behind the `×` when pointed at.
                let hover_bg = if ui.response_for(close_wid).hovered {
                    Background {
                        fill: theme.header_fill.into(),
                        corners: Corners::all(3.0),
                        ..Default::default()
                    }
                } else {
                    Background::default()
                };
                let close = Panel::zstack()
                    .id(close_wid)
                    .size((Sizing::Fixed(16.0), Sizing::Fixed(16.0)))
                    .sense(Sense::CLICK)
                    // Pin to the chip's top edge so it reads as a
                    // top-right corner close (it's already the rightmost
                    // item in the row).
                    .align(Align::v(VAlign::Top))
                    .child_align(Align::CENTER)
                    .background(hover_bg)
                    .show(ui, |ui| {
                        // `×` at a size that fits the 16px box (the
                        // default 16px font overflows and rides high) and
                        // in a legible muted-light ink, not the near-bg
                        // border color it was before.
                        Text::new("\u{00d7}")
                            .style(TextStyle {
                                color: theme.selection_glow,
                                font_size_px: 13.0,
                                // Hug the glyph (no 1.2× leading) so the
                                // line box ≈ the glyph and it centers in
                                // the 16px button instead of riding high.
                                line_height_mult: 1.0,
                                ..ui.theme.text
                            })
                            .text_align(Align::CENTER)
                            .show(ui);
                    });
                close_clicked = close.response.clicked();
            }
        });
    // Close wins over activate when both could read the same press —
    // the close glyph consumes its own hit, so this is just belt-and-
    // braces against the chip also reporting the click.
    if close_clicked {
        out.push(UiAction::CloseTab(index));
    } else if chip.response.clicked() {
        out.push(UiAction::ActivateTab(index));
    }
}
