//! [`Icon`]: every glyph a node, a toolbar and the image viewer draw, as one set.

use std::f32::consts::PI;

use palantir::{Rect, RgbaF32, Stroke, Ui};

use glam::Vec2;
use palantir::widget::{CircularArc, LineCap, LineJoin, Shape};

use crate::gui::widgets::support;

/// The side of the grid an icon's artwork is drawn on, scaled to the box it paints in.
///
/// 24 units with a 2-unit stroke, the grid and weight of the open icon sets (Lucide, Feather,
/// Material Symbols at weight 400), so a 16 px icon strokes at 1.33 px and a 18 px one at 1.5 px.
const GRID: f32 = 24.0;
const STROKE: f32 = 2.0;

/// The span of [`Icon::Cross`]'s arms on the grid, centered.
const CROSS_SPAN: f32 = 12.0;

/// The stroke an icon drawn in a `side` box wears.
pub(crate) const fn stroke_width(side: f32) -> f32 {
    side / GRID * STROKE
}

/// The span of [`Icon::Cross`]'s arms in a `side` box, for a widget that draws the cross itself.
pub(crate) const fn cross_span(side: f32) -> f32 {
    side / GRID * CROSS_SPAN
}

/// One glyph of the editor's icon set: stroke artwork on a 24-unit grid, one stroke width, round
/// caps and joins, painted in one ink. Refused: letters as icons (`R`, `D`, `i`), and font glyphs
/// (`■`, `~`, `↻`), which sit on a font's baseline and weight rather than on the grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Icon {
    /// Run: a play triangle.
    Run,
    /// Run on events: a bar before a play triangle.
    RunEvents,
    /// Inspect: an `i` in a circle.
    Inspect,
    /// The node's output cache: three stacked disks.
    Cache,
    /// Disable a sink: a power symbol.
    Disable,
    /// A sink, which runs for its effect: an arrow into a bar, where the flow ends (Lucide
    /// `arrow-right-to-line`).
    Sink,
    /// Impure, recomputed every run: a lightning bolt.
    Impure,
    /// Reset the view: a ring around a dot.
    ResetView,
    /// Show all nodes: a frame around four dots.
    ShowAll,
    /// Show the selection: a frame around one square.
    ShowSelected,
    /// Fit the image to the view: four corner brackets.
    Fit,
    /// Show the image at its actual size: a ratio of one to one, drawn as the set's strokes and
    /// dots.
    ActualSize,
    /// Nearest sampling: a 2 × 2 grid of squares.
    Pixels,
    /// Done: a check mark.
    Check,
    /// Failed: a cross.
    Cross,
    /// Keep in place: a push pin.
    Pin,
}

impl Icon {
    /// Paint the icon into the square `box_` of the current panel's space, in `color`.
    pub(crate) fn draw(self, ui: &mut Ui, box_: Rect, color: RgbaF32) {
        let k = box_.size.w.min(box_.size.h) / GRID;
        let origin = box_.min + (Vec2::new(box_.size.w, box_.size.h) - Vec2::splat(GRID * k)) * 0.5;
        let pen = Pen {
            origin,
            k,
            stroke: Stroke::new(color, STROKE * k),
            color,
        };
        match self {
            Self::Run => pen.closed(ui, &[(7.0, 4.5), (19.0, 12.0), (7.0, 19.5)]),
            Self::RunEvents => {
                pen.line(ui, (5.0, 5.0), (5.0, 19.0));
                pen.closed(ui, &[(10.0, 5.0), (20.0, 12.0), (10.0, 19.0)]);
            }
            Self::Inspect => {
                pen.circle(ui, (12.0, 12.0), 9.0);
                pen.line(ui, (12.0, 11.0), (12.0, 16.5));
                pen.dot(ui, (12.0, 7.75), 1.25);
            }
            Self::Cache => {
                for y in [3.5, 10.0, 16.5] {
                    pen.frame(ui, (4.0, y, 16.0, 4.0), 2.0);
                }
            }
            Self::Disable => {
                // Open at the top, where the stem stands: from 300° round to 240°, clockwise on
                // screen.
                pen.arc(ui, (12.0, 13.0), 8.0, -PI / 3.0, 5.0 * PI / 3.0);
                pen.line(ui, (12.0, 3.0), (12.0, 12.0));
            }
            Self::Sink => {
                pen.line(ui, (3.0, 12.0), (17.0, 12.0));
                pen.open(ui, &[(11.0, 6.0), (17.0, 12.0), (11.0, 18.0)]);
                pen.line(ui, (21.0, 5.0), (21.0, 19.0));
            }
            Self::Impure => pen.closed(
                ui,
                &[
                    (13.0, 2.5),
                    (5.0, 13.5),
                    (11.5, 13.5),
                    (10.5, 21.5),
                    (19.0, 10.5),
                    (12.5, 10.5),
                ],
            ),
            Self::ResetView => {
                pen.circle(ui, (12.0, 12.0), 7.0);
                pen.dot(ui, (12.0, 12.0), 2.0);
            }
            Self::ShowAll => {
                pen.frame(ui, (3.5, 3.5, 17.0, 17.0), 3.0);
                for center in [(9.0, 9.0), (15.0, 9.0), (9.0, 15.0), (15.0, 15.0)] {
                    pen.dot(ui, center, 1.5);
                }
            }
            Self::ShowSelected => {
                pen.frame(ui, (3.5, 3.5, 17.0, 17.0), 3.0);
                pen.fill(ui, (8.5, 8.5, 7.0, 7.0), 1.5);
            }
            Self::Fit => {
                pen.open(ui, &[(4.0, 9.0), (4.0, 4.0), (9.0, 4.0)]);
                pen.open(ui, &[(15.0, 4.0), (20.0, 4.0), (20.0, 9.0)]);
                pen.open(ui, &[(20.0, 15.0), (20.0, 20.0), (15.0, 20.0)]);
                pen.open(ui, &[(9.0, 20.0), (4.0, 20.0), (4.0, 15.0)]);
            }
            Self::ActualSize => {
                for x in [6.0, 18.0] {
                    pen.open(ui, &[(x - 2.0, 8.0), (x, 6.0), (x, 18.0)]);
                }
                pen.dot(ui, (12.0, 10.0), 1.25);
                pen.dot(ui, (12.0, 15.0), 1.25);
            }
            Self::Check => pen.open(ui, &[(4.5, 12.5), (9.5, 17.5), (19.5, 6.5)]),
            Self::Cross => {
                let (center, half) = (GRID * 0.5, CROSS_SPAN * 0.5);
                let (near, far) = (center - half, center + half);
                pen.line(ui, (near, near), (far, far));
                pen.line(ui, (far, near), (near, far));
            }
            Self::Pin => {
                pen.line(ui, (9.0, 3.5), (15.0, 3.5));
                pen.closed(
                    ui,
                    &[
                        (10.0, 3.5),
                        (10.0, 10.0),
                        (6.5, 14.5),
                        (17.5, 14.5),
                        (14.0, 10.0),
                        (14.0, 3.5),
                    ],
                );
                pen.line(ui, (12.0, 14.5), (12.0, 20.5));
            }
            Self::Pixels => {
                for (x, y) in [(5.0, 5.0), (13.0, 5.0), (5.0, 13.0), (13.0, 13.0)] {
                    pen.fill(ui, (x, y, 6.0, 6.0), 1.0);
                }
            }
        }
    }
}

/// The icon grid mapped onto a box: a point `(x, y)` in grid units lands at `origin + (x, y) * k`.
#[derive(Clone, Copy, Debug)]
struct Pen {
    origin: Vec2,
    k: f32,
    stroke: Stroke,
    color: RgbaF32,
}

impl Pen {
    fn at(self, point: impl Into<Vec2>) -> Vec2 {
        self.origin + point.into() * self.k
    }

    fn rect(self, (x, y, w, h): (f32, f32, f32, f32)) -> Rect {
        let min = self.at((x, y));
        Rect::new(min.x, min.y, w * self.k, h * self.k)
    }

    fn line(self, ui: &mut Ui, a: impl Into<Vec2>, b: impl Into<Vec2>) {
        ui.add_shape(Shape::line(self.at(a), self.at(b), self.stroke).cap(LineCap::Round));
    }

    /// An open stroke through `points`; at most four, the most any icon draws in one stroke.
    fn open(self, ui: &mut Ui, points: &[(f32, f32)]) {
        let mut placed = [Vec2::ZERO; 4];
        for (to, &from) in placed.iter_mut().zip(points) {
            *to = self.at(from);
        }
        ui.add_shape(
            Shape::polyline(&placed[..points.len()], self.stroke)
                .cap(LineCap::Round)
                .join(LineJoin::Round),
        );
    }

    /// A closed stroke through `points` and back to the first; at most six.
    fn closed(self, ui: &mut Ui, points: &[(f32, f32)]) {
        let mut placed = [Vec2::ZERO; 7];
        for (to, &from) in placed.iter_mut().zip(points) {
            *to = self.at(from);
        }
        placed[points.len()] = placed[0];
        ui.add_shape(
            Shape::polyline(&placed[..=points.len()], self.stroke)
                .cap(LineCap::Round)
                .join(LineJoin::Round),
        );
    }

    fn circle(self, ui: &mut Ui, center: impl Into<Vec2>, radius: f32) {
        ui.add_shape(Shape::circle(self.at(center), radius * self.k, self.stroke));
    }

    fn arc(self, ui: &mut Ui, center: impl Into<Vec2>, radius: f32, start_angle: f32, sweep: f32) {
        let arc = CircularArc {
            center: self.at(center),
            radius: radius * self.k,
            start_angle,
            sweep,
        };
        ui.add_shape(Shape::arc(arc, self.stroke).cap(LineCap::Round));
    }

    fn dot(self, ui: &mut Ui, center: impl Into<Vec2>, radius: f32) {
        ui.add_shape(support::dot(self.at(center), radius * self.k).fill(self.color));
    }

    /// A rounded-rect outline, its stroke centered on the rect's edge as a polyline's is.
    fn frame(self, ui: &mut Ui, rect: (f32, f32, f32, f32), radius: f32) {
        let half = STROKE * 0.5;
        let (x, y, w, h) = rect;
        let outer = self.rect((x - half, y - half, w + STROKE, h + STROKE));
        ui.add_shape(
            Shape::rect(outer)
                .corners((radius + half) * self.k)
                .border(self.stroke),
        );
    }

    fn fill(self, ui: &mut Ui, rect: (f32, f32, f32, f32), radius: f32) {
        ui.add_shape(
            Shape::rect(self.rect(rect))
                .corners(radius * self.k)
                .fill(self.color),
        );
    }
}
