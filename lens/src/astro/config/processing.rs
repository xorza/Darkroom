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
use lumos::{BackgroundMode, ColorMode, ExtractBackground, Scnr, Stretch, StretchMethod};

use crate::astro::config::preset::Preset;

/// The pick is the extraction mode; every other field keeps its default.
impl Preset for BackgroundMode {
    type Knobs = ExtractBackground;
    type Config = ExtractBackground;

    fn config(self) -> ExtractBackground {
        ExtractBackground {
            mode: self,
            ..Default::default()
        }
    }
}

/// Which green-removal protection [`ScnrKnobs`] builds; the amount is its own
/// field, which every protection reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, IntrospectEnum)]
#[config(type_id = "662e2432-b685-4b5b-bf05-0041814dc908")]
pub(crate) enum ScnrMethodChoice {
    AverageNeutral,
    AdditiveMask,
    MaximumNeutral,
    MaximumMask,
}

impl Preset for ScnrMethodChoice {
    type Knobs = ScnrKnobs;
    type Config = Scnr;

    fn config(self) -> Scnr {
        ScnrKnobs {
            method: self,
            ..Default::default()
        }
        .into()
    }
}

/// The editable knobs behind a [`Scnr`]: the protection, and the blend toward
/// its full strength.
#[derive(Debug, Clone, Introspect)]
#[config(type_id = "cb80e688-a5ed-42fd-9087-6a9639a8b056", name = "ScnrConfig")]
pub(crate) struct ScnrKnobs {
    method: ScnrMethodChoice,
    amount: f32,
}

impl Default for ScnrKnobs {
    /// Average-neutral at full strength, as [`Scnr::default`].
    fn default() -> Self {
        Self {
            method: ScnrMethodChoice::AverageNeutral,
            amount: 1.0,
        }
    }
}

impl From<ScnrKnobs> for Scnr {
    fn from(knobs: ScnrKnobs) -> Self {
        match knobs.method {
            ScnrMethodChoice::AverageNeutral => Scnr::average_neutral(knobs.amount),
            ScnrMethodChoice::AdditiveMask => Scnr::additive_mask(knobs.amount),
            ScnrMethodChoice::MaximumNeutral => Scnr::maximum_neutral(knobs.amount),
            ScnrMethodChoice::MaximumMask => Scnr::maximum_mask(knobs.amount),
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
/// `target_background` and a `shadow_sigmas`, which sets their black point.
#[derive(Debug, Clone, Introspect)]
#[config(type_id = "b08bb9a1-db12-43d4-aa57-fe3e3732e917", name = "Stretch")]
pub(crate) struct StretchKnobs {
    method: StretchMethodChoice,
    target_background: f32,
    shadow_sigmas: f32,
    color: ColorMode,
}

impl Default for StretchKnobs {
    /// Lumos's automatic presets: [`Stretch::default`]'s auto-asinh, with the
    /// black point and target both automatic methods share.
    fn default() -> Self {
        Self {
            method: StretchMethodChoice::AutoAsinh,
            target_background: StretchMethod::AUTO_TARGET_BACKGROUND,
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
                target_background: knobs.target_background,
            },
            StretchMethodChoice::AutoStf => StretchMethod::AutoStf {
                shadow_sigmas: knobs.shadow_sigmas,
                target_background: knobs.target_background,
            },
        };
        Stretch {
            method,
            color: knobs.color,
        }
    }
}

#[cfg(test)]
mod tests {
    use common::{Introspect, IntrospectEnum};
    use lumos::{
        BackgroundMode, ColorMode, Denoise, ExtractBackground, Hdr, LocalContrast, Stretch,
        StretchMethod, Threshold,
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
            field_names::<ExtractBackground>(),
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
            ["method", "target_background", "shadow_sigmas", "color"]
        );
    }

    /// The variant strings are what a saved graph stores for an enum port, and
    /// the derive renders them from the variant names — so a rename in lumos
    /// would change what is already on disk.
    #[test]
    fn enum_ports_keep_their_stored_variant_names() {
        assert_eq!(BackgroundMode::VARIANTS, ["subtract", "divide"]);
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
            target_background: 0.25,
            shadow_sigmas: 2.0,
            color: ColorMode::PerChannel,
        }
        .into();
        let StretchMethod::AutoStf {
            shadow_sigmas,
            target_background,
        } = stretch.method
        else {
            panic!("expected auto-STF");
        };
        assert_eq!(shadow_sigmas, 2.0);
        assert_eq!(target_background, 0.25);
        assert_eq!(stretch.color, ColorMode::PerChannel);

        let stretch: Stretch = StretchKnobs {
            method: StretchMethodChoice::AutoAsinh,
            target_background: 0.3,
            shadow_sigmas: 1.5,
            color: ColorMode::ColorPreserving,
        }
        .into();
        let StretchMethod::AutoAsinh {
            shadow_sigmas,
            target_background,
        } = stretch.method
        else {
            panic!("expected auto-asinh");
        };
        assert_eq!((shadow_sigmas, target_background), (1.5, 0.3));
    }
}
