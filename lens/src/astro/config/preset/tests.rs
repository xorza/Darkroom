use std::fmt;

use lumos::detection;
use lumos::{
    BackgroundMode, ColorMode, ExtractBackground, RegistrationConfig, Scnr, StackConfig, Stretch,
    StretchMethod,
};
use scenarium::{ConstValue, DataType, DynamicValue, TypeId};

use crate::astro::config::preset::Preset;
use crate::astro::config::processing::{ScnrMethodChoice, StretchKnobs, StretchMethodChoice};
use crate::astro::config::stacking::{CombineMethodChoice, DetectionPreset, RegistrationPreset};
use crate::config_node::{ConfigValue, config_data_type};

/// The stored strings and the labels a picker shows, pinned once per family:
/// a stored string is what a saved graph holds, so a rename is a format change.
#[test]
fn each_picker_offers_its_variants_under_their_labels() {
    fn offered<P: Preset>() -> Vec<(String, &'static str)> {
        let picker = P::picker("Pick");
        assert!(picker.const_only, "only a wired config overrides a pick");
        assert_eq!(
            picker.data_type,
            DataType::Enum(TypeId::literal(P::TYPE_ID))
        );
        assert_eq!(
            picker.default_value,
            Some(ConstValue::Enum(P::VARIANTS[0].to_owned()))
        );
        picker
            .value_variants
            .iter()
            .map(|variant| {
                let label = P::LABELS[P::VARIANTS.iter().position(|v| *v == variant.name).unwrap()];
                assert_eq!(variant.label(), label);
                (variant.name.clone(), label)
            })
            .collect()
    }
    let names = |offered: Vec<(String, &'static str)>| -> Vec<String> {
        offered.into_iter().map(|(name, _)| name).collect()
    };

    assert_eq!(
        offered::<StretchMethodChoice>(),
        [
            ("auto_asinh".to_owned(), "Auto Asinh"),
            ("auto_stf".to_owned(), "Auto STF")
        ]
    );
    assert_eq!(names(offered::<BackgroundMode>()), ["subtract", "divide"]);
    assert_eq!(
        names(offered::<ScnrMethodChoice>()),
        [
            "average_neutral",
            "additive_mask",
            "maximum_neutral",
            "maximum_mask"
        ]
    );
    assert_eq!(
        names(offered::<DetectionPreset>()),
        [
            "wide_field",
            "high_resolution",
            "crowded_field",
            "precise_ground"
        ]
    );
    assert_eq!(
        names(offered::<RegistrationPreset>()),
        ["default", "fast", "precise", "wide_field", "mosaic"]
    );
    assert_eq!(
        names(offered::<CombineMethodChoice>()),
        ["sigma_clipped", "winsorized", "median", "mean"]
    );
}

/// The config input overrides the picker it names and carries the knobs.
#[test]
fn the_config_input_overrides_the_picker() {
    let input = StretchMethodChoice::config_input("Config", 1);
    assert!(!input.required);
    assert_eq!(input.overrides, Some(1));
    assert_eq!(input.data_type, config_data_type::<StretchKnobs>());
}

/// Each pick builds exactly the lumos preset it names.
#[test]
fn each_pick_builds_the_preset_it_names() {
    let same = |a: &dyn fmt::Debug, b: &dyn fmt::Debug| {
        assert_eq!(format!("{a:?}"), format!("{b:?}"));
    };
    same(
        &StretchMethodChoice::AutoAsinh.config(),
        &Stretch::auto_asinh(),
    );
    same(&StretchMethodChoice::AutoStf.config(), &Stretch::auto_stf());
    same(
        &ScnrMethodChoice::AverageNeutral.config(),
        &Scnr::average_neutral(1.0),
    );
    same(
        &ScnrMethodChoice::MaximumNeutral.config(),
        &Scnr::maximum_neutral(1.0),
    );
    same(
        &ScnrMethodChoice::MaximumMask.config(),
        &Scnr::maximum_mask(1.0),
    );
    same(
        &ScnrMethodChoice::AdditiveMask.config(),
        &Scnr::additive_mask(1.0),
    );
    assert_eq!(BackgroundMode::Divide.config().mode, BackgroundMode::Divide);
    same(
        &BackgroundMode::Subtract.config(),
        &ExtractBackground {
            mode: BackgroundMode::Subtract,
            ..ExtractBackground::default()
        },
    );
    for (pick, preset) in [
        (DetectionPreset::WideField, detection::Config::wide_field()),
        (
            DetectionPreset::HighResolution,
            detection::Config::high_resolution(),
        ),
        (
            DetectionPreset::CrowdedField,
            detection::Config::crowded_field(),
        ),
        (
            DetectionPreset::PreciseGround,
            detection::Config::precise_ground(),
        ),
    ] {
        same(&pick.config(), &preset);
    }
    for (pick, preset) in [
        (RegistrationPreset::Default, RegistrationConfig::default()),
        (RegistrationPreset::Fast, RegistrationConfig::fast()),
        (RegistrationPreset::Precise, RegistrationConfig::precise()),
        (
            RegistrationPreset::WideField,
            RegistrationConfig::wide_field(),
        ),
        (RegistrationPreset::Mosaic, RegistrationConfig::mosaic()),
    ] {
        same(&pick.config(), &preset);
    }
    // Compared on `method` alone: a stack config's default cache directory is
    // unique per instance.
    for (pick, preset) in [
        (
            CombineMethodChoice::SigmaClipped,
            StackConfig::sigma_clipped(3.0),
        ),
        (
            CombineMethodChoice::Winsorized,
            StackConfig::winsorized(3.0),
        ),
        (CombineMethodChoice::Median, StackConfig::median()),
        (CombineMethodChoice::Mean, StackConfig::mean()),
    ] {
        assert_eq!(pick.config().method, preset.method, "{pick:?}");
    }
}

/// A wired config is what runs; without one, the pick is.
#[test]
fn a_wired_config_wins_over_the_pick() {
    let pick = DynamicValue::from(ConstValue::Enum("auto_stf".to_owned()));
    let picked = StretchMethodChoice::resolve(&pick, &DynamicValue::Unbound);
    assert!(matches!(picked.method, StretchMethod::AutoStf { .. }));

    let wired = DynamicValue::from_custom(ConfigValue(StretchKnobs::default()));
    let configured = StretchMethodChoice::resolve(&DynamicValue::Unbound, &wired);
    assert!(matches!(configured.method, StretchMethod::AutoAsinh { .. }));
    assert_eq!(configured.color, ColorMode::ColorPreserving);
}
