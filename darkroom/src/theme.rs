use palantir::Color;

/// Visual palette + layout dimensions for darkroom's UI. Owned by
/// `MainWindow`, handed to every UI subtree through
/// [`crate::app::AppContext`] so call sites read off a single source
/// instead of hard-coded constants. Layout fields live here too —
/// node ports, value editors, etc. — so a theme swap can restyle
/// geometry as well as color.
///
/// Also owns the palantir [`palantir::Theme`] this app wants on its
/// `Ui`. The `WinitHost::with_setup` hook in `main` copies
/// `palantir_theme` into `ui.theme` once before the first frame, so
/// palantir-side widgets (buttons, text edits, menus, scrollbars)
/// read from the same source. Tweak fields on `theme.palantir_theme`
/// before constructing the host to override palantir's defaults.
///
/// Serializable so the whole bundle (palantir palette + darkroom
/// layout + colors) round-trips through Rhai for the Theme → Load /
/// Export menu.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Theme {
    /// Palantir-side widget theme. Pushed onto `Ui::theme` once at
    /// startup so every palantir widget (Button, TextEdit, MenuItem,
    /// Scroll, Tooltip…) reads a darkroom-tuned palette without each
    /// call site restyling per use. Round-trips through Rhai as part
    /// of the exported theme (palantir's tooltip "unbounded" size and
    /// noop chrome now serialize Rhai-safely).
    pub palantir_theme: palantir::Theme,
    // ── canvas ────────────────────────────────────────────────────
    pub canvas_bg: Color,

    // ── connections ──────────────────────────────────────────────
    pub connection_broken: Color,
    pub connection_width: f32,
    pub breaker_stroke: Color,
    pub breaker_stroke_width: f32,

    // ── node chrome ──────────────────────────────────────────────
    pub node_fill: Color,
    pub node_border: Color,
    pub node_border_width: f32,
    pub node_corner_radius: f32,
    /// Minimum content size for a node body. Caps how tightly a node
    /// with very short port labels can shrink horizontally so the
    /// header stays legible at any zoom.
    pub node_min_width: f32,
    pub node_min_height: f32,

    pub header_fill: Color,
    pub header_corner_radius: f32,

    /// Glow color painted behind the selected node — picked brighter
    /// than `canvas_bg` so the halo reads as a highlight, not noise.
    pub selection_glow: Color,

    // ── ports ────────────────────────────────────────────────────
    pub input_port: Color,
    pub output_port: Color,
    pub input_port_hover: Color,
    pub output_port_hover: Color,
    /// Side of the port circle quad. The circle's corner radius is
    /// derived as `port_size * 0.5` (see [`Self::port_radius`]).
    pub port_size: f32,
    /// Vertical inset at the top of each port column (gap below the
    /// header band before the first port).
    pub port_col_pad_top: f32,
    /// Horizontal inset on each side of the ports row. Port circles
    /// overhang by `-(port_radius + port_col_pad_x)` so they straddle
    /// the node body edge regardless of this inset.
    pub port_col_pad_x: f32,
    /// Vertical gap between adjacent ports in a column.
    pub port_gap: f32,
    /// Horizontal gap between the input and output port columns.
    pub port_cols_gap: f32,

    // ── inline editors / popups ─────────────────────────────────
    /// Fixed width of the inline static-value editor that hugs a
    /// `Binding::Const` input port.
    pub value_editor_width: f32,
    /// Cap on the new-node popup's height. Inner scroll handles
    /// overflow when the function list exceeds the cap.
    pub new_node_popup_max_height: f32,
}

impl Theme {
    /// Derived radius for port circles — half the port side. Lives as
    /// a method instead of a stored field so the two can't drift if
    /// someone bumps `port_size` and forgets the radius.
    #[inline]
    pub fn port_radius(&self) -> f32 {
        self.port_size * 0.5
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            palantir_theme: palantir::Theme::default(),

            canvas_bg: Color::hex(0x1e1e1e),

            connection_broken: Color::hex(0xff5a55),
            connection_width: 2.0,
            breaker_stroke: Color::hex(0xff5a55),
            breaker_stroke_width: 2.0,

            node_fill: Color::hex(0x2d2d33),
            node_border: Color::hex(0x5a5a66),
            node_border_width: 1.0,
            node_corner_radius: 6.0,
            node_min_width: 160.0,
            node_min_height: 10.0,

            header_fill: Color::hex(0x3a3a44),
            header_corner_radius: 6.0,

            selection_glow: Color::hex(0x9ec1ff),

            input_port: Color::hex(0x77c97a),
            output_port: Color::hex(0xe39a4a),
            input_port_hover: Color::hex(0xb3e8b6),
            output_port_hover: Color::hex(0xffc878),
            port_size: 10.0,
            port_col_pad_top: 6.0,
            port_col_pad_x: 8.0,
            port_gap: 6.0,
            port_cols_gap: 12.0,

            value_editor_width: 60.0,
            new_node_popup_max_height: 400.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::SerdeFormat;

    /// The whole bundle — darkroom's own fields *and* the nested
    /// palantir palette — must survive a Rhai round-trip; that's the
    /// on-disk format the Theme → Load / Export menu and the config
    /// rely on. The palantir side exercises the two formerly-broken
    /// cases: the tooltip's infinite max-size axis and a noop shadow.
    #[test]
    fn theme_roundtrips_through_rhai() {
        let mut theme = Theme {
            node_min_width: 137.5,
            selection_glow: Color::hex(0x123456),
            ..Theme::default()
        };
        theme.palantir_theme.window_clear = Color::hex(0xabcdef);

        let bytes = common::serialize(&theme, SerdeFormat::Rhai);
        let back: Theme = common::deserialize(&bytes, SerdeFormat::Rhai)
            .expect("theme should deserialize from its own Rhai output");

        assert_eq!(back.node_min_width, 137.5);
        assert_eq!(back.selection_glow, Color::hex(0x123456));
        assert_eq!(back.canvas_bg, theme.canvas_bg);
        // Nested palantir palette now round-trips too.
        assert_eq!(back.palantir_theme.window_clear, Color::hex(0xabcdef));
        // The infinite tooltip-height axis survives via the
        // `unbounded_size` adapter.
        assert!(back.palantir_theme.tooltip.max_size.h.is_infinite());
        assert_eq!(back.palantir_theme.tooltip.max_size.w, 280.0);
    }
}
