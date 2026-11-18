//! The reference tab: every command with its chord, then every gesture of the canvas with its
//! keys, read from the tables the menus and the canvas act by, so the page cannot name a key
//! the editor does not read.

use palantir::{
    Align, Configure, Grid, InternedStr, Scroll, SizeSpec, Spacing, Text, TextStyle, Track, Ui,
    WidgetId, fmt,
};

use crate::gui::command::Command;
use crate::gui::pane::graph_ui::canvas_input::CanvasGesture;
use crate::gui::theme::Theme;
use crate::gui::widgets::support;

/// Cap on the page's column, so a row's name and its keys stay within one glance on a wide pane.
const COLUMN_MAX_WIDTH: f32 = 480.0;

/// The most rows the page holds: a caption for each of its two sections, a row for each command
/// and for each gesture. A command with no chord takes none, so the page reads a prefix.
const ROWS: [Track; 2 + Command::ALL.len() + CanvasGesture::ALL.len()] =
    [Track::HUG; 2 + Command::ALL.len() + CanvasGesture::ALL.len()];

/// Record the reference tab: one grid for both sections, so every row's keys start at one edge,
/// in the monospace face, and a chord's `Ctrl+` reads in one column down the page, as a
/// keyboard reference sets its keys. Refused: the keys pushed to the right edge, whose ragged
/// left edge set no two chords' parts under each other.
pub(crate) fn show(ui: &mut Ui, theme: &Theme) {
    let chords = Command::ALL
        .into_iter()
        .filter(|command| command.keys().is_some())
        .count();
    let row_count = 2 + chords + CanvasGesture::ALL.len();
    Scroll::vertical()
        .id_salt("reference")
        .size(SizeSpec::FILL)
        .show(ui, |ui| {
            Grid::new()
                .id_salt("reference_body")
                .size(SizeSpec::FILL_W)
                .max_size((COLUMN_MAX_WIDTH, f32::INFINITY))
                .padding(theme.pane_padding())
                .cols([Track::FILL, Track::HUG])
                .rows(&ROWS[..row_count])
                .gap(theme.space.md)
                .line_gap(line_gap(theme))
                .show(ui, |ui| {
                    let mut grid_row = 0_u16;
                    caption(ui, theme, "Commands", &mut grid_row);
                    for command in Command::ALL {
                        let Some(keys) = command.keys() else {
                            continue;
                        };
                        let keys = fmt!(ui, "{keys}");
                        row(ui, theme, &mut grid_row, command.spec().label, keys);
                    }
                    caption(ui, theme, "On the canvas", &mut grid_row);
                    for gesture in CanvasGesture::ALL {
                        let hint = gesture.hint();
                        let keys = fmt!(ui, "{}", hint.keys);
                        row(ui, theme, &mut grid_row, hint.does, keys);
                    }
                });
        });
}

/// The space between two rows: a row's padding above and below, and the gap between rows.
fn line_gap(theme: &Theme) -> f32 {
    theme.space.xs + 2.0 * theme.space.xxs
}

/// A section's caption in the name column, at `grid_row`, then the next row: a section after
/// the first stands a section's gap below the last row.
fn caption(ui: &mut Ui, theme: &Theme, caption: &'static str, grid_row: &mut u16) {
    let above = if *grid_row == 0 {
        0.0
    } else {
        theme.space.lg - line_gap(theme)
    };
    Text::new(caption)
        .id_salt(caption)
        .style(&support::caption_text(ui, theme))
        .grid_cell((*grid_row, 0))
        .margin(Spacing::new(0.0, above, 0.0, 0.0))
        .show(ui);
    *grid_row += 1;
}

/// One line at `grid_row`, then the next row: what it does in the name column, its keys in the
/// keys column in the monospace face, both from that column's left edge.
fn row(ui: &mut Ui, theme: &Theme, grid_row: &mut u16, does: &'static str, keys: InternedStr) {
    Text::new(does)
        .id(does_wid(*grid_row))
        .style(&support::body_text(ui, theme))
        .grid_cell((*grid_row, 0))
        .show(ui);
    let style = TextStyle {
        color: theme.ink.muted,
        ..support::mono_text(ui, theme.font.body)
    };
    Text::new(keys)
        .id(keys_wid(*grid_row))
        .style(&style)
        .grid_cell((*grid_row, 1))
        .align(Align::LEFT)
        .show(ui);
    *grid_row += 1;
}

/// What the row at `grid_row` does.
fn does_wid(grid_row: u16) -> WidgetId {
    WidgetId::from_hash(("darkroom.reference.does", grid_row))
}

/// The keys of the row at `grid_row`.
fn keys_wid(grid_row: u16) -> WidgetId {
    WidgetId::from_hash(("darkroom.reference.keys", grid_row))
}

#[cfg(test)]
mod tests {
    use glam::UVec2;
    use palantir::ColorScheme;
    use palantir::Harness;

    use crate::gui::command::Command;
    use crate::gui::pane::graph_ui::canvas_input::CanvasGesture;
    use crate::gui::theme::Theme;

    /// Every row's keys start at one left edge, the commands' and the gestures' alike, right of
    /// every name: the longest chord and the shortest line up at their first letter. Rows 0 and
    /// `1 + chords` are the captions.
    #[test]
    fn every_rows_keys_start_at_one_edge() {
        let theme = Theme::new(ColorScheme::Dark);
        let mut h = Harness::builder(UVec2::new(600, 1600)).mono().build();
        h.prime(2, |ui| super::show(ui, &theme));
        let chords = Command::ALL
            .into_iter()
            .filter(|command| command.keys().is_some())
            .count();
        let rows = (1..=chords).chain(chords + 2..chords + 2 + CanvasGesture::ALL.len());
        let mut edges = Vec::new();
        for row in rows {
            let row = u16::try_from(row).unwrap();
            let does = h.rect(super::does_wid(row)).expect("a name");
            let keys = h.rect(super::keys_wid(row)).expect("its keys");
            assert!(
                does.max().x < keys.min.x,
                "row {row}: {does:?} before {keys:?}"
            );
            edges.push(keys.min.x);
        }
        assert_eq!(edges.len(), chords + CanvasGesture::ALL.len());
        assert!(edges.iter().all(|&edge| edge == edges[0]), "{edges:?}");
        assert_eq!(h.rect(super::keys_wid(0)), None, "a caption has no keys");
    }
}
