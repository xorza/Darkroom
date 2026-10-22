//! Which per-frame processing configs the editor can build, and the projections
//! for the ones whose shape it cannot edit directly.
//!
//! Most of these are simply the lumos config: it derives
//! [`Introspect`] itself, so the builder node's ports *are*
//! its fields and adding one in lumos adds a port here with no edit on this
//! side. Their wire identity — a `TYPE_ID` that ships in saved documents and so
//! is fixed for the life of the type, and a `DISPLAY_NAME` that is only what the
//! editor labels that wire — is declared by their own derive in lumos.
//!
//! A config the field model can't express — one whose enum variants carry
//! data, which [`IntrospectEnum`] does not describe —
//! gets a projection instead: a flat struct of the knobs the editor offers,
//! plus the one-way conversion that expands them back into the real config. A
//! projection is deliberately narrower than the type it builds, so it does
//! *not* track that type field-for-field.

use common::{Introspect, IntrospectEnum};
use lumos::{BgMode, ColorMode, ExtractBg, Scnr, ScnrProtection, Stretch, StretchMethod};

use crate::astro::config::preset::Preset;

/// The pick is the extraction mode; every other field keeps its default.
impl Preset for BgMode {
    type Knobs = ExtractBg;
    type Config = ExtractBg;

    fn config(self) -> ExtractBg {
        ExtractBg {
            mode: self,
            ..Default::default()
        }
    }
}

/// The pick is the protection; the strength keeps its default, full.
impl Preset for ScnrProtection {
    type Knobs = Scnr;
    type Config = Scnr;

    fn config(self) -> Scnr {
        Scnr {
            protection: self,
            ..Default::default()
        }
    }
}

/// Which stretch curve [`StretchKnobs`] builds — the two automatic methods.
/// [`StretchMethod`]'s explicit curves (`Asinh`, `Ghs`) are not offered: each
/// carries its own parameter set, which one flat knob list cannot present
/// without showing every other method's parameters alongside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, IntrospectEnum)]
#[config(type_id = "722f7047-a6fc-4538-abd7-8af5fd1ee0ff")]
pub(crate) enum StretchMethodChoice {
    AutoAsinh,
    #[config(label = "Auto STF")]
    AutoStf,
}

impl Preset for StretchMethodChoice {
    type Knobs = StretchKnobs;
    type Config = Stretch;

    fn config(self) -> Stretch {
        StretchKnobs {
            method: self,
            ..Default::default()
        }
        .into()
    }
}

/// The editable knobs behind a [`Stretch`]. Both methods take a
/// `target_bg` and a `shadow_sigmas`, which sets their black point.
#[derive(Debug, Clone, Introspect)]
#[config(type_id = "b08bb9a1-db12-43d4-aa57-fe3e3732e917", name = "Stretch")]
pub(crate) struct StretchKnobs {
    method: StretchMethodChoice,
    target_bg: f32,
    shadow_sigmas: f32,
    color: ColorMode,
}

impl Default for StretchKnobs {
    /// Lumos's automatic presets: [`Stretch::default`]'s auto-asinh, with the
    /// black point and target both automatic methods share.
    fn default() -> Self {
        Self {
            method: StretchMethodChoice::AutoAsinh,
            target_bg: StretchMethod::AUTO_TARGET_BG,
            shadow_sigmas: StretchMethod::AUTO_SHADOW_SIGMAS,
            color: Stretch::default().color,
        }
    }
}

impl From<StretchKnobs> for Stretch {
    fn from(knobs: StretchKnobs) -> Self {
        let method = match knobs.method {
            StretchMethodChoice::AutoAsinh => StretchMethod::AutoAsinh {
                shadow_sigmas: knobs.shadow_sigmas,
                target_bg: knobs.target_bg,
            },
            StretchMethodChoice::AutoStf => StretchMethod::AutoStf {
                shadow_sigmas: knobs.shadow_sigmas,
                target_bg: knobs.target_bg,
            },
        };
        Self {
            method,
            color: knobs.color,
        }
    }
}

#[cfg(test)]
mod tests {
    use common::{Introspect, IntrospectEnum};
    use lumos::{
        BgMode, ColorMode, Denoise, ExtractBg, Hdr, LocalContrast, Stretch, StretchMethod,
        Threshold,
    };

    use crate::astro::config::processing::{StretchKnobs, StretchMethodChoice};

    fn field_names<T: Introspect>() -> Vec<&'static str> {
        T::fields().into_iter().map(|field| field.name).collect()
    }

    /// A builder node's ports are its config's fields, in declaration order,
    /// and a saved graph binds them by position — so reordering or renaming a
    /// field in lumos silently rewires every document that used the node.
    /// Pinned here because the config types live in another crate: this is
    /// what makes the coupling visible from the side that depends on it.
    #[test]
    fn builder_ports_follow_the_lumos_field_order() {
        assert_eq!(
            field_names::<ExtractBg>(),
            [
                "tile_size",
                "degree",
                "mode",
                "rejection_sigma",
                "iterations",
                "divide_floor"
            ]
        );
        assert_eq!(
            field_names::<Denoise>(),
            ["scales", "k", "threshold", "strength"]
        );
        assert_eq!(field_names::<Hdr>(), ["scales", "amount"]);
        assert_eq!(
            field_names::<LocalContrast>(),
            ["tiles", "clip_limit", "strength"]
        );
        assert_eq!(
            field_names::<StretchKnobs>(),
            ["method", "target_bg", "shadow_sigmas", "color"]
        );
    }

    /// The variant strings are what a saved graph stores for an enum port, and
    /// the derive renders them from the variant names — so a rename in lumos
    /// would change what is already on disk.
    #[test]
    fn enum_ports_keep_their_stored_variant_names() {
        assert_eq!(BgMode::VARIANTS, ["subtract", "divide"]);
        assert_eq!(Threshold::VARIANTS, ["hard", "soft"]);
        assert_eq!(ColorMode::VARIANTS, ["color_preserving", "per_channel"]);
        assert_eq!(StretchMethodChoice::VARIANTS, ["auto_asinh", "auto_stf"]);
    }

    #[test]
    fn stretch_default_and_supported_methods_convert_exactly() {
        let default = StretchKnobs::default();
        assert_eq!(default.method, StretchMethodChoice::AutoAsinh);
        assert_eq!(default.color, ColorMode::ColorPreserving);

        let stretch: Stretch = StretchKnobs {
            method: StretchMethodChoice::AutoStf,
            target_bg: 0.25,
            shadow_sigmas: 2.0,
            color: ColorMode::PerChannel,
        }
        .into();
        let StretchMethod::AutoStf {
            shadow_sigmas,
            target_bg,
        } = stretch.method
        else {
            panic!("expected auto-STF");
        };
        assert_eq!(shadow_sigmas, 2.0);
        assert_eq!(target_bg, 0.25);
        assert_eq!(stretch.color, ColorMode::PerChannel);

        let stretch: Stretch = StretchKnobs {
            method: StretchMethodChoice::AutoAsinh,
            target_bg: 0.3,
            shadow_sigmas: 1.5,
            color: ColorMode::ColorPreserving,
        }
        .into();
        let StretchMethod::AutoAsinh {
            shadow_sigmas,
            target_bg,
        } = stretch.method
        else {
            panic!("expected auto-asinh");
        };
        assert_eq!((shadow_sigmas, target_bg), (1.5, 0.3));
    }
}
