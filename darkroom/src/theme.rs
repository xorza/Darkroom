use palantir::Color;

// ── default palette + dimensions ─────────────────────────────────────
// The single source of truth for darkroom's built-in look. `Theme::default`
// is assembled from these; `tests::ayu_graphite_asset_in_sync` serializes
// that default and rewrites `assets/ayu-graphite.toml`, so the checked-in
// theme file is a generated artifact (a reference users can copy / edit
// via Theme → Load), never a parallel source that can drift.

// canvas
const CANVAS_BG: Color = Color::hex(0x1a1a1a);
const SELECTION_RECT: Color = Color::hex(0x9adbfb);
const CANVAS_DOT: Color = Color::hex(0x363636);
const CANVAS_DOT_SPACING: f32 = 18.0;
const CANVAS_DOT_RADIUS: f32 = 0.6;

// connections
const CONNECTION_BROKEN: Color = Color::hex(0xff5e44);
const CONNECTION_WIDTH: f32 = 2.0;
const BREAKER_STROKE: Color = Color::hex(0xff5e44);
const BREAKER_STROKE_WIDTH: f32 = 2.0;

// node chrome
const NODE_FILL: Color = Color::hex(0x343434);
const NODE_BORDER: Color = Color::hex(0x363636);
const NODE_BORDER_WIDTH: f32 = 1.0;
const NODE_CORNER_RADIUS: f32 = 6.0;
const NODE_MIN_WIDTH: f32 = 160.0;
const NODE_MIN_HEIGHT: f32 = 10.0;
const HEADER_FILL: Color = Color::hex(0x414141);
const TAB_CORNER_RADIUS: f32 = 6.0;
const TEXT_MUTED: Color = Color::hex(0xaaaaa8);
const CHROME_FILL: Color = Color::hex(0x252525);

// header badges
const BADGE_SUBGRAPH: Color = Color::hex(0x9adbfb);
const BADGE_TERMINAL: Color = Color::hex(0xff5e44);
const BADGE_CACHE: Color = Color::hex(0xffd44a);

// execution-status glow
const EXEC_EXECUTED_GLOW: Color = Color::hex(0xdaff58);
const EXEC_CACHED_GLOW: Color = Color::hex(0x9adbfb);
const EXEC_MISSING_GLOW: Color = Color::hex(0xffa63d);
const EXEC_ERRORED_GLOW: Color = Color::hex(0xff5e44);

// ports
const INPUT_PORT: Color = Color::hex(0xdaff58);
const OUTPUT_PORT: Color = Color::hex(0xffa63d);
const INPUT_PORT_HOVER: Color = Color::hex(0xe9ff8e);
const OUTPUT_PORT_HOVER: Color = Color::hex(0xffc878);
const PORT_SIZE: f32 = 10.0;
const PORT_COL_PAD_TOP: f32 = 6.0;
const PORT_COL_PAD_X: f32 = 8.0;
const PORT_GAP: f32 = 6.0;
const PORT_COLS_GAP: f32 = 12.0;

// inline editors / popups
const VALUE_EDITOR_WIDTH: f32 = 100.0;
const NEW_NODE_POPUP_MAX_HEIGHT: f32 = 400.0;

// palantir sub-theme tweaks (see `default_palantir_theme`)
const MENU_FONT_SIZE: f32 = 13.0;
const MENU_CHIP_ALPHA: f32 = 0.85;

/// Visual palette + layout dimensions for darkroom's UI. Owned by
/// `MainWindow`, handed to every UI subtree through
/// [`crate::app::AppContext`] so call sites read off a single source
/// instead of hard-coded constants. Layout fields live here too —
/// node ports, value editors, etc. — so a theme swap can restyle
/// geometry as well as color.
///
/// Also owns the palantir [`palantir::Theme`] this app wants on its
/// `Ui`. [`crate::app::App::new`] copies `palantir_theme` into
/// `ui.theme` once before the first frame, so palantir-side widgets
/// (buttons, text edits, menus, scrollbars) read from the same source.
/// Tweak fields on `theme.palantir_theme` during construction to
/// override palantir's defaults.
///
/// Serializable so the whole bundle (palantir palette + darkroom
/// layout + colors) round-trips through Rhai for the Theme → Load /
/// Export menu.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Theme {
    // Scalar (`Color` / `f32`) fields come first; `palantir_theme`
    // (a nested table) is last. TOML serialization requires every
    // scalar value to precede any table at the same level — otherwise
    // the serializer errors with `ValueAfterTable`.

    // ── canvas ────────────────────────────────────────────────────
    pub canvas_bg: Color,
    /// Tint of the rubber-band multi-selection rectangle. Drawn as a
    /// translucent fill plus a near-opaque 1px border, both derived
    /// from this single color (palette accent).
    pub selection_rect: Color,
    /// Dotted backdrop grid: dot color, world-space base spacing
    /// between dots, and on-screen dot radius (px). Spacing is wrapped
    /// by a power-of-2 multiplier as the user zooms so the field never
    /// collapses into noise — see `gui::background`.
    pub canvas_dot: Color,
    pub canvas_dot_spacing: f32,
    pub canvas_dot_radius: f32,

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
    /// Corner radius of the tab-strip tabs (the header derives its own
    /// radius from `node_corner_radius`, so it doesn't read this).
    pub tab_corner_radius: f32,

    /// Muted secondary foreground (palette `text_muted`, `#aaaaa8`). The
    /// de-emphasized accent shared across chrome: the selected-node halo,
    /// inactive/disabled header chips, the pinned-inspector outline, and
    /// active-tab text — visible without competing with the bright accent
    /// (`badge_subgraph`) or full-strength text.
    pub text_muted: Color,

    /// Top-chrome fill behind the menu bar + tab strip. Palette `bg`
    /// (`#252525`) — a notch darker than the node surface, sitting
    /// between the graph (`canvas_bg`) and the nodes, so the chrome
    /// recedes and the active tab (which uses `canvas_bg`) reads as
    /// continuous with the graph below it. `#[serde(default)]` so older
    /// themes keep loading without regenerating the asset.
    #[serde(default = "default_chrome_fill")]
    pub chrome_fill: Color,

    // ── header badges ────────────────────────────────────────────
    // Indicator-chip accents in the node header. `#[serde(default)]`
    // so themes predating these fields still load (the embedded asset
    // doesn't need regenerating to pick up new chips).
    /// Subgraph (composite instance) chip — accent cyan.
    #[serde(default = "default_badge_subgraph")]
    pub badge_subgraph: Color,
    /// Terminal (sink) chip — error red.
    #[serde(default = "default_badge_terminal")]
    pub badge_terminal: Color,
    /// Cache (compute-once) chip — warning yellow.
    #[serde(default = "default_badge_cache")]
    pub badge_cache: Color,

    // ── execution-status glow ────────────────────────────────────
    // Color of the soft glow shadow behind a node, by the last run's
    // outcome (mirrors the deprecated editor's per-status shadows).
    // Palette swatches: `success`/`accent`/`syn_keyword`/`error`.
    // `#[serde(default)]` so themes predating these fields still load.
    /// Node computed this run — palette `success` (green).
    #[serde(default = "default_exec_executed")]
    pub exec_executed_glow: Color,
    /// Node reused its cached result — palette `accent` (cyan).
    #[serde(default = "default_exec_cached")]
    pub exec_cached_glow: Color,
    /// Node has unfilled required inputs — palette `syn_keyword` (orange).
    #[serde(default = "default_exec_missing")]
    pub exec_missing_glow: Color,
    /// Node errored — palette `error` (red).
    #[serde(default = "default_exec_errored")]
    pub exec_errored_glow: Color,

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

    /// Palantir-side widget theme. Pushed onto `Ui::theme` once at
    /// startup so every palantir widget (Button, TextEdit, MenuItem,
    /// Scroll, Tooltip…) reads a darkroom-tuned palette without each
    /// call site restyling per use. Last field so its TOML table
    /// follows all the scalar fields above (TOML `ValueAfterTable`).
    pub palantir_theme: palantir::Theme,
}

// `#[serde(default)]` fallbacks for fields added after the first themes
// shipped, so a user theme predating them still loads. They return the
// same consts `Theme::default` uses — one source of truth.
fn default_chrome_fill() -> Color {
    CHROME_FILL
}
fn default_badge_subgraph() -> Color {
    BADGE_SUBGRAPH
}
fn default_badge_terminal() -> Color {
    BADGE_TERMINAL
}
fn default_badge_cache() -> Color {
    BADGE_CACHE
}
fn default_exec_executed() -> Color {
    EXEC_EXECUTED_GLOW
}
fn default_exec_cached() -> Color {
    EXEC_CACHED_GLOW
}
fn default_exec_missing() -> Color {
    EXEC_MISSING_GLOW
}
fn default_exec_errored() -> Color {
    EXEC_ERRORED_GLOW
}

/// Palantir sub-theme for darkroom: palantir's own defaults, with the
/// menu bar + context-menu rows shrunk to [`MENU_FONT_SIZE`] and the
/// floating menu-bar triggers given a semi-transparent node-surface chip
/// (legible over busy nodes while still hinting through). Built in code so
/// it tracks palette changes; baked into the generated asset by the
/// in-sync test.
fn default_palantir_theme() -> palantir::Theme {
    use palantir::{Brush, WidgetLook};

    let mut theme = palantir::Theme::default();
    let base = theme.text;
    let shrink = |look: &mut WidgetLook| {
        look.text = Some(look.text.unwrap_or(base).with_font_size(MENU_FONT_SIZE));
    };
    let chip = Brush::Solid(NODE_FILL.with_alpha(MENU_CHIP_ALPHA));
    let mb = &mut theme.menu_button;
    shrink(&mut mb.normal);
    shrink(&mut mb.hovered);
    shrink(&mut mb.pressed);
    shrink(&mut mb.disabled);
    if let Some(bg) = mb.normal.background.as_mut() {
        bg.fill = chip.clone();
    }
    if let Some(bg) = mb.disabled.background.as_mut() {
        bg.fill = chip;
    }
    let item = &mut theme.context_menu.item;
    shrink(&mut item.normal);
    shrink(&mut item.hovered);
    shrink(&mut item.disabled);
    theme
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
    /// Built entirely from the module consts (the single source of truth
    /// for darkroom's look) plus [`default_palantir_theme`] — no file I/O,
    /// no deserialize. The checked-in `assets/ayu-graphite.toml` is
    /// regenerated *from* this by `tests::ayu_graphite_asset_in_sync`.
    fn default() -> Self {
        Self {
            canvas_bg: CANVAS_BG,
            selection_rect: SELECTION_RECT,
            canvas_dot: CANVAS_DOT,
            canvas_dot_spacing: CANVAS_DOT_SPACING,
            canvas_dot_radius: CANVAS_DOT_RADIUS,
            connection_broken: CONNECTION_BROKEN,
            connection_width: CONNECTION_WIDTH,
            breaker_stroke: BREAKER_STROKE,
            breaker_stroke_width: BREAKER_STROKE_WIDTH,
            node_fill: NODE_FILL,
            node_border: NODE_BORDER,
            node_border_width: NODE_BORDER_WIDTH,
            node_corner_radius: NODE_CORNER_RADIUS,
            node_min_width: NODE_MIN_WIDTH,
            node_min_height: NODE_MIN_HEIGHT,
            header_fill: HEADER_FILL,
            tab_corner_radius: TAB_CORNER_RADIUS,
            text_muted: TEXT_MUTED,
            chrome_fill: CHROME_FILL,
            badge_subgraph: BADGE_SUBGRAPH,
            badge_terminal: BADGE_TERMINAL,
            badge_cache: BADGE_CACHE,
            exec_executed_glow: EXEC_EXECUTED_GLOW,
            exec_cached_glow: EXEC_CACHED_GLOW,
            exec_missing_glow: EXEC_MISSING_GLOW,
            exec_errored_glow: EXEC_ERRORED_GLOW,
            input_port: INPUT_PORT,
            output_port: OUTPUT_PORT,
            input_port_hover: INPUT_PORT_HOVER,
            output_port_hover: OUTPUT_PORT_HOVER,
            port_size: PORT_SIZE,
            port_col_pad_top: PORT_COL_PAD_TOP,
            port_col_pad_x: PORT_COL_PAD_X,
            port_gap: PORT_GAP,
            port_cols_gap: PORT_COLS_GAP,
            value_editor_width: VALUE_EDITOR_WIDTH,
            new_node_popup_max_height: NEW_NODE_POPUP_MAX_HEIGHT,
            palantir_theme: default_palantir_theme(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::SerdeFormat;

    /// Keep the checked-in `assets/ayu-graphite.toml` in sync with the
    /// const-defined [`Theme::default`]: serialize the default and rewrite
    /// the file. The asset is a generated artifact (a reference theme
    /// users can copy / the Theme → Load-Export format), not a source of
    /// truth — running the suite regenerates it, so any change to the
    /// consts (or palantir's defaults) surfaces as an asset diff to commit.
    /// Writing is idempotent when already in sync, so it's a no-op on a
    /// clean tree.
    #[test]
    fn ayu_graphite_asset_in_sync() {
        let bytes = common::serialize(&Theme::default(), SerdeFormat::Toml);
        std::fs::write("assets/ayu-graphite.toml", bytes).expect("write toml asset");
    }

    /// The whole bundle — darkroom's own fields *and* the nested
    /// palantir palette — must survive a TOML round-trip; that's the
    /// on-disk format the Theme → Load / Export menu and the config
    /// rely on. Exercises the formerly-fragile case too: the tooltip's
    /// infinite max-size axis (handled by `Size`'s custom serde).
    #[test]
    fn theme_roundtrips_through_toml() {
        let mut theme = Theme {
            node_min_width: 137.5,
            text_muted: Color::hex(0x123456),
            ..Theme::default()
        };
        theme.palantir_theme.window_clear = Color::hex(0xabcdef);

        let bytes = common::serialize(&theme, SerdeFormat::Toml);
        let back: Theme = common::deserialize(&bytes, SerdeFormat::Toml)
            .expect("theme should deserialize from its own TOML output");

        assert_eq!(back.node_min_width, 137.5);
        assert_eq!(back.text_muted, Color::hex(0x123456));
        assert_eq!(back.canvas_bg, theme.canvas_bg);
        // Nested palantir palette round-trips too.
        assert_eq!(back.palantir_theme.window_clear, Color::hex(0xabcdef));
        // The infinite tooltip-height axis survives `Size`'s serde.
        assert!(back.palantir_theme.tooltip.max_size.h.is_infinite());
        assert_eq!(back.palantir_theme.tooltip.max_size.w, 280.0);
    }

    /// Pin a few const-defined default values (Ayu Mirage High Contrast:
    /// canvas = terminal_bg, ports = success-green / syn-keyword-orange)
    /// plus the non-trivial palantir tweak, so an accidental const edit or
    /// a regression in `default_palantir_theme` fails loudly.
    #[test]
    fn default_palette_and_menu_tweak() {
        let theme = Theme::default();
        assert_eq!(theme.canvas_bg, Color::hex(0x1a1a1a));
        assert_eq!(theme.input_port, Color::hex(0xdaff58));
        assert_eq!(theme.output_port, Color::hex(0xffa63d));
        assert_eq!(theme.node_min_width, 160.0);
        assert!(theme.palantir_theme.tooltip.max_size.h.is_infinite());
        // The menu-bar font was shrunk from palantir's default to ours.
        let menu_text = theme
            .palantir_theme
            .menu_button
            .normal
            .text
            .expect("menu button carries an explicit text style");
        assert_eq!(menu_text.font_size_px, MENU_FONT_SIZE);
    }
}
