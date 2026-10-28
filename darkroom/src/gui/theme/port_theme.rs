//! [`PortTheme`]: how a node's input and output ports are drawn.

use palantir::RgbaF32;

use crate::gui::theme::palette::Palette;
use crate::gui::theme::space_scale::SpaceScale;

/// A node's ports: the circles straddling the body edge, their labels,
/// and the column geometry that lays them out.
///
/// Positional colors only — a *typed* port takes its hue from
/// [`TypeColors`](crate::gui::theme::type_colors::TypeColors) instead, resolved by
/// [`Theme::port_color`](crate::gui::theme::Theme::port_color), which needs this roster and the
/// type roster together. Every color here is a resting one; the pointer lift is applied at the
/// call site.
#[derive(Clone, Debug)]
pub(crate) struct PortTheme {
    /// Color for an untyped (`Any`) input port. No output is untyped.
    pub(crate) input: RgbaF32,
    /// Event emitter glyphs, subscription pins, and event wires — distinct
    /// from the type-colored data ports.
    pub(crate) event: RgbaF32,
    /// Port + event label ink — de-emphasized against the full-strength
    /// value/editor text so each port row has one strong element. Its own
    /// slot rather than `text_muted` at the call site: which of the row's
    /// three inks recedes is the interface's decision to change.
    pub(crate) label: RgbaF32,
    /// Side of the port circle quad, 12: a whole number of pixels at scales 1, 1.5 and 2 at
    /// 100 % zoom. The circle's radius is derived as half this (see [`Self::radius`]).
    pub(crate) size: f32,
    /// Side of a port's dot in a row that lists ports, the inspector's: under [`Self::size`], as
    /// a row is no place for a wire to land or a press to grab.
    pub(crate) row_dot: f32,
    /// Horizontal inset on each side of the ports row, where every port label starts: the header's
    /// and footer's inset, so a label lines up with the title. Circles hang out by
    /// `Theme::port_margin` (which folds in this inset and the card border)
    /// so their center sits on the body edge regardless of it.
    pub(crate) col_pad_x: f32,
    /// The column's vertical rhythm, spent twice over: the inset below the
    /// header rule before the first port, and the gap between adjacent ports.
    /// One field because equal spacing is the point — a distinct top inset
    /// would read as a misaligned first row.
    pub(crate) gap: f32,
    /// Horizontal gap between the input and output columns.
    pub(crate) cols_gap: f32,
}

impl PortTheme {
    pub(super) const fn from_palette(p: &Palette, space: &SpaceScale) -> Self {
        Self {
            input: p.input_port,
            event: p.event_port,
            label: p.port_label,
            size: 12.0,
            row_dot: 10.0,
            col_pad_x: space.md,
            gap: space.sm,
            cols_gap: space.md,
        }
    }

    /// Derived radius for port circles — half the port side. A method rather
    /// than a stored field so the two can't drift if someone bumps `size`.
    #[inline]
    pub(crate) fn radius(&self) -> f32 {
        self.size * 0.5
    }
}
