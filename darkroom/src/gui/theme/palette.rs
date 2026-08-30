//! [`Palette`]: darkroom's colour roster, read from the baked-in
//! `assets/ayu-graphite.ron`.

use palantir::Color;

use crate::gui::theme::type_colors::TypeColors;

/// The generated palette file, compiled in. Written by `darkroom/build.py`
/// in the ayu-graphite repo, which resolves each role below against that
/// palette's semantic layer and copies the result here.
const AYU_GRAPHITE: &str = include_str!("../../../assets/ayu-graphite.ron");

/// Every colour darkroom paints with, one field per role, as loaded from
/// `assets/ayu-graphite.ron`. This is the theme's whole colour vocabulary:
/// [`Theme::build`](crate::gui::theme::Theme::build) fills each
/// roster from it, so a call site reads a named role instead of a hex
/// literal and restyling is an edit to the palette rather than to the app.
///
/// Every entry is a *resting* colour. A port lifts under the pointer by
/// blending toward white (see
/// [`port_color`](crate::gui::pane::graph::node::port_color)), because the
/// palette's brightest tint has nothing above it to lift into.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct Palette {
    /// Ground fill behind the whole graph.
    pub(crate) canvas_bg: Color,
    /// Backdrop grid dot colour.
    pub(crate) canvas_dot: Color,
    /// Top-chrome fill behind the menu bar and tab strip.
    pub(crate) chrome_fill: Color,
    /// Inactive tab chip.
    pub(crate) tab_inactive: Color,
    /// Node body fill, and the surface tier palantir gives its own popups.
    pub(crate) node_fill: Color,
    /// Resting card outline. Transparent — the shadow carries the edge.
    pub(crate) node_border: Color,
    /// Header band, a step off the body so the band reads against it.
    pub(crate) header_fill: Color,
    /// Ambient elevation shadow. A near-black ground takes a lot of alpha
    /// before a shadow registers at all.
    pub(crate) node_ambient_shadow: Color,
    /// Primary ink.
    pub(crate) text: Color,
    /// De-emphasized chrome ink.
    pub(crate) text_muted: Color,
    /// Disabled ink.
    pub(crate) text_disabled: Color,
    /// Control fill one step up from the chrome behind it.
    pub(crate) elem_mid: Color,
    /// Control fill two steps up — the emphasis tier.
    pub(crate) elem_strong: Color,
    /// Focus ring.
    pub(crate) border_focused: Color,
    /// Rubber-band sweep and committed selection halo.
    pub(crate) selection_rect: Color,
    /// A wire whose endpoint no longer resolves.
    pub(crate) connection_broken: Color,
    /// The scribble that cuts wires.
    pub(crate) breaker_stroke: Color,
    /// Positional colour for an untyped input port.
    pub(crate) input_port: Color,
    /// Positional colour for an untyped output port.
    pub(crate) output_port: Color,
    /// Event emitters, subscription pins and event wires — the sink
    /// marker's red, so the trigger machinery reads as one family.
    pub(crate) event_port: Color,
    /// Port and event label ink.
    pub(crate) port_label: Color,
    /// Inspect chip and pinned-inspector outline.
    pub(crate) badge_graph: Color,
    /// Sink chip.
    pub(crate) badge_sink: Color,
    /// Persist-to-disk cache chip.
    pub(crate) badge_cache: Color,
    /// Impure marker. Shares the palette's one bright magenta with
    /// [`TypeColors::image`] — a glyph on a node's head never meets a wire.
    pub(crate) badge_impure: Color,
    /// It worked / it ran.
    pub(crate) status_success: Color,
    /// It was reused from cache.
    pub(crate) status_info: Color,
    /// It is happening right now.
    pub(crate) status_busy: Color,
    /// It is incomplete but not broken.
    pub(crate) status_warning: Color,
    /// It failed.
    pub(crate) status_error: Color,
    /// Data-type hues for wires and typed port circles.
    pub(crate) type_colors: TypeColors,
}

impl Palette {
    /// Parse the baked-in palette.
    ///
    /// The file is compiled into the binary, so a parse failure means the
    /// checked-in asset and this struct disagree — a build-time mistake,
    /// not a runtime condition, which is why it panics rather than
    /// returning a `Result` no caller could act on.
    pub(super) fn load() -> Self {
        ron::from_str(AYU_GRAPHITE).expect("assets/ayu-graphite.ron matches Palette")
    }
}
