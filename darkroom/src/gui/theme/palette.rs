//! [`Palette`]: darkroom's color roster, one per scheme, read from the baked-in
//! `assets/ayu-graphite-dark.ron` and `assets/ayu-graphite-light.ron`.

use palantir::{ColorScheme, RgbaF32};

use crate::gui::theme::type_colors::TypeColors;

/// The generated palette files, compiled in, one per scheme. Written by `darkroom/build.py` in the
/// ayu-graphite repo, which resolves each role below against that scheme's semantic layer and
/// copies the result here.
const DARK: &str = include_str!("../../../assets/ayu-graphite-dark.ron");
const LIGHT: &str = include_str!("../../../assets/ayu-graphite-light.ron");

/// Every color darkroom paints with in one scheme, one field per role, as loaded from that
/// scheme's file: the theme's whole color vocabulary, which
/// [`Theme::build`](crate::gui::theme::Theme::build) fills each roster from.
///
/// Every entry is a *resting* color. A port lifts under the pointer by blending toward
/// [`Self::lift`] (see [`PortTheme::paint`](crate::gui::theme::port_theme::PortTheme::paint)),
/// because the palette's far tint has nothing past it to lift into.
///
/// **Roles may share a hue, never a form.** The file resolves every role against one upstream
/// semantic layer, so a badge, a status and a wire type routinely land on the same value; each
/// then shows in a form of its own, as `darkroom/AGENTS.md`, "A status and a type share a hue,
/// never a form", records. Two badges meet on one head, which is why
/// `theme::tests::badges_are_told_apart_from_each_other` pins them pairwise.
///
/// A role the file holds and this struct does not fails the load, as a missing one does.
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Palette {
    /// Ground fill behind the whole graph.
    pub(crate) canvas_bg: RgbaF32,
    /// Backdrop grid dot color.
    pub(crate) canvas_dot: RgbaF32,
    /// Surround fill behind the menu bar and tab strip.
    pub(crate) surround_bg: RgbaF32,
    /// Node body fill, and the surface tier palantir gives its own popups and its modal.
    pub(crate) node_bg: RgbaF32,
    /// Resting card outline. Transparent — the shadow carries the edge.
    pub(crate) node_border: RgbaF32,
    /// Ambient elevation shadow. A near-black ground takes a lot of alpha
    /// before a shadow registers at all.
    pub(crate) shadow: RgbaF32,
    /// Primary ink.
    pub(crate) text: RgbaF32,
    /// De-emphasized ink.
    pub(crate) text_muted: RgbaF32,
    /// Disabled ink.
    pub(crate) text_disabled: RgbaF32,
    /// Control fill one step up from the surface behind it — the hover tier.
    pub(crate) element_mid: RgbaF32,
    /// Control fill two steps up — the emphasis tier a press reaches for.
    pub(crate) element_strong: RgbaF32,
    /// The accent: the selection's rubber band and halo, focus, and the buttons that open a
    /// node's inspector.
    pub(crate) accent: RgbaF32,
    /// A wire whose endpoint no longer resolves.
    pub(crate) wire_broken: RgbaF32,
    /// The scribble that cuts wires.
    pub(crate) breaker_stroke: RgbaF32,
    /// The neutral a hover blends a port and a link toward, past every ink: white on a dark
    /// ground, black on a light one.
    pub(crate) lift: RgbaF32,
    /// The light square of the checker behind a transparent image, a transparency reference the
    /// same in every theme.
    pub(crate) checker_light: RgbaF32,
    /// The dark square of that checker.
    pub(crate) checker_dark: RgbaF32,
    /// Color for an untyped (`Any`) input port.
    pub(crate) input_port: RgbaF32,
    /// Events, triggers and event wires: one white, as Unreal draws its execution pins, so the
    /// event machinery reads as one family.
    pub(crate) event_port: RgbaF32,
    /// Port and event label ink.
    pub(crate) port_label: RgbaF32,
    /// A text link.
    pub(crate) link: RgbaF32,
    /// Persist-to-disk cache chip.
    pub(crate) badge_cache: RgbaF32,
    /// The kind mark: a sink, or a node recomputed every run.
    pub(crate) badge_kind: RgbaF32,
    /// It worked / it ran.
    pub(crate) status_success: RgbaF32,
    /// It was reused from cache.
    pub(crate) status_info: RgbaF32,
    /// It is happening right now.
    pub(crate) status_busy: RgbaF32,
    /// It is incomplete but not broken.
    pub(crate) status_warning: RgbaF32,
    /// It failed.
    pub(crate) status_error: RgbaF32,
    /// Data-type hues for wires and typed port circles.
    pub(crate) type_colors: TypeColors,
}

impl Palette {
    /// Parse the baked-in palette of `scheme`.
    ///
    /// Panics on a parse failure: the file is compiled in, so that is a mismatch between the
    /// checked-in asset and this struct, not a runtime condition.
    pub(super) fn load(scheme: ColorScheme) -> Self {
        let file = match scheme {
            ColorScheme::Dark => DARK,
            ColorScheme::Light => LIGHT,
        };
        ron::from_str(file)
            .unwrap_or_else(|error| panic!("the {scheme:?} palette matches Palette: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use common::internals::with_unknown_field::{UNKNOWN_FIELD, WithUnknownField};

    use super::*;

    /// The file and the struct must name the same roles: one the generator
    /// still emits but darkroom stopped reading fails the load, in the palette and in its type
    /// roster alike, like one it dropped. Both edits start from a real asset, and both assets
    /// load.
    #[test]
    fn a_role_the_struct_does_not_read_fails_the_load() {
        for file in [DARK, LIGHT] {
            ron::from_str::<Palette>(file).expect("the asset loads");
        }

        let extras = WithUnknownField::each_struct(DARK);
        assert_eq!(extras.len(), 2, "the palette and its `type_colors`");
        for extra in extras {
            let error = ron::from_str::<Palette>(&extra.text).expect_err("an extra role fails");
            assert_eq!(error.span.start.line, extra.line, "{error}");
            assert!(
                matches!(&error.code, ron::Error::NoSuchStructField { found, .. } if found == UNKNOWN_FIELD),
                "{error}"
            );
        }

        let missing = DARK
            .lines()
            .filter(|line| !line.trim_start().starts_with("wire_broken:"))
            .collect::<Vec<_>>()
            .join("\n");
        assert_ne!(missing, DARK, "the role came out");
        let error = ron::from_str::<Palette>(&missing).expect_err("a missing role fails");
        assert!(error.to_string().contains("wire_broken"), "{error}");
    }
}
