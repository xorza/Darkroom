//! [`PortTheme`]: how a node's input and output ports are drawn.

use palantir::Color;

use crate::gui::theme::palette::Palette;

/// A node's ports: the circles straddling the body edge, their labels,
/// and the column geometry that lays them out.
///
/// Positional colours only — a *typed* port takes its hue from
/// [`TypeColors`](crate::gui::theme::type_colors::TypeColors) instead,
/// resolved by `gui::pane::graph::node::port_color`, which needs this
/// roster and the type roster together and so stays a function over the
/// whole [`Theme`](crate::gui::theme::Theme). Every colour here is a
/// resting one; the pointer lift is applied at that call site.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct PortTheme {
    /// Positional colour for untyped input ports.
    pub(crate) input: Color,
    /// Positional colour for untyped output ports.
    pub(crate) output: Color,
    /// Event emitter glyphs, subscription pins, and event wires — distinct
    /// from the type-coloured data ports.
    pub(crate) event: Color,
    /// Port + event label ink — de-emphasized against the full-strength
    /// value/editor text so each port row has one strong element. Its own
    /// slot rather than `text_muted` at the call site: which of the row's
    /// three inks recedes is the interface's decision to change.
    pub(crate) label: Color,
    /// Side of the port circle quad; the circle's radius is derived as half
    /// this (see [`Self::radius`]).
    pub(crate) size: f32,
    /// Horizontal inset on each side of the ports row. Circles overhang by
    /// `-Theme::port_overhang()` (which folds in this inset and the card
    /// border) so their centre sits on the body edge regardless of it.
    pub(crate) col_pad_x: f32,
    /// The column's vertical rhythm, spent twice over: the inset below the
    /// header band before the first port, and the gap between adjacent ports.
    /// One field because equal spacing is the point — a distinct top inset
    /// would read as a misaligned first row.
    pub(crate) gap: f32,
    /// Horizontal gap between the input and output columns.
    pub(crate) cols_gap: f32,
}

impl PortTheme {
    pub(super) fn from_palette(p: &Palette) -> Self {
        Self {
            input: p.input_port,
            output: p.output_port,
            event: p.event_port,
            label: p.port_label,
            size: 13.0,
            col_pad_x: 8.0,
            gap: 6.0,
            cols_gap: 12.0,
        }
    }

    /// Derived radius for port circles — half the port side. A method rather
    /// than a stored field so the two can't drift if someone bumps `size`.
    #[inline]
    pub(crate) fn radius(&self) -> f32 {
        self.size * 0.5
    }
}
