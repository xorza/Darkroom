//! [`PortTheme`]: how a node's input and output ports are drawn.

use palantir::Color;

use crate::gui::theme::hover_color::HoverColor;

use crate::core::theme_pref::ThemePreset;
use crate::gui::theme::palette_struct::palette_struct;
use crate::gui::theme::swatches::{dark, light};

palette_struct! {
    /// A node's ports: the circles straddling the body edge, their labels,
    /// and the column geometry that lays them out.
    ///
    /// Positional swatches only — a *typed* port takes its hue from
    /// [`TypeColors`](crate::gui::theme::type_colors::TypeColors) instead, resolved by
    /// `gui::pane::graph::node::port_color`, which needs this roster, the
    /// type roster and the preset together and so stays a function over the
    /// whole [`Theme`](crate::gui::theme::Theme).
    #[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
    pub(crate) struct PortTheme;
    /// Positional swatch for untyped input ports; hover lifts for emphasis.
    input: HoverColor => INPUT_PORT,
    /// Positional swatch for untyped output ports.
    output: HoverColor => OUTPUT_PORT,
    /// Event emitter glyphs, subscription pins, and event wires — neutral,
    /// distinct from the type-coloured data ports; hover lifts it like the
    /// positional colours.
    event: HoverColor => EVENT_PORT,
    /// Port + event label ink — de-emphasized against the full-strength
    /// value/editor text so each port row has one strong element. Its own
    /// slot (not `text_muted`) because the light palette needs a darker value
    /// for legibility on the card fill.
    label: Color => PORT_LABEL,
    ;
    /// Side of the port circle quad; the circle's radius is derived as half
    /// this (see [`Self::radius`]).
    size: f32 = 13.0,
    /// Horizontal inset on each side of the ports row. Circles overhang by
    /// `-Theme::port_overhang()` (which folds in this inset and the card
    /// border) so their centre sits on the body edge regardless of it.
    col_pad_x: f32 = 8.0,
    /// The column's vertical rhythm, spent twice over: the inset below the
    /// header band before the first port, and the gap between adjacent ports.
    /// One field because equal spacing is the point — a distinct top inset
    /// would read as a misaligned first row.
    gap: f32 = 6.0,
    /// Horizontal gap between the input and output columns.
    cols_gap: f32 = 12.0,
}

impl PortTheme {
    /// Derived radius for port circles — half the port side. A method rather
    /// than a stored field so the two can't drift if someone bumps `size`.
    #[inline]
    pub(crate) fn radius(&self) -> f32 {
        self.size * 0.5
    }
}
