use std::str::FromStr;

use lumos::{BackgroundMode, ExtractBackground, RegistrationConfig, Scnr, Stretch, StretchMethod};
use scenarium::{ConstValue, DynamicValue, EnumVariants};
use strum::IntoEnumIterator;

use crate::astro::config::preset::resolve;
use crate::astro::config::processing::{BackgroundModeKind, ScnrKind, StretchKnobs, StretchPreset};
use crate::astro::config::stacking::{CombinePreset, DetectionPreset, RegistrationPreset};
use crate::config_node::ConfigValue;

#[test]
fn detection_labels_round_trip_through_from_str() {
    assert_eq!(
        DetectionPreset::variant_names(),
        [
            "wide_field",
            "high_resolution",
            "crowded_field",
            "precise_ground"
        ]
    );
    for v in DetectionPreset::iter() {
        assert_eq!(DetectionPreset::from_str(v.label()).unwrap(), v);
    }
    assert!(DetectionPreset::from_str("nope").is_err());
}

#[test]
fn combine_preset_lists_its_variants() {
    assert_eq!(
        CombinePreset::variant_names(),
        ["sigma_clipped", "winsorized", "median", "mean"]
    );
}

#[test]
fn registration_default_label_maps_to_default_config() {
    // The `Default` variant resolves and produces a config (smoke test
    // that the macro wired the trait `default()` ctor correctly).
    assert_eq!(
        RegistrationPreset::from_str("default").unwrap(),
        RegistrationPreset::Default
    );
    let _cfg: RegistrationConfig = RegistrationPreset::Default.config();
}

#[test]
fn stretch_preset_round_trips_and_maps_to_config() {
    assert_eq!(StretchPreset::variant_names(), ["auto_asinh", "auto_stf"]);
    for v in StretchPreset::iter() {
        assert_eq!(StretchPreset::from_str(v.label()).unwrap(), v);
    }
    let _cfg: Stretch = StretchPreset::AutoStf.config();
}

#[test]
fn processing_enums_have_expected_variants() {
    assert_eq!(BackgroundModeKind::variant_names(), ["subtract", "divide"]);
    assert_eq!(
        ScnrKind::variant_names(),
        ["average_neutral", "additive_mask"]
    );
    let background: ExtractBackground = BackgroundModeKind::Divide.config();
    assert_eq!(background.mode, BackgroundMode::Divide);
    let _scnr: Scnr = ScnrKind::AdditiveMask.config();
}

#[test]
fn resolver_accepts_presets_and_wired_configs() {
    let preset = resolve::<StretchKnobs, StretchPreset>(&DynamicValue::from(ConstValue::Enum(
        "auto_stf".to_string(),
    )));
    assert!(matches!(preset.method, StretchMethod::AutoStf { .. }));

    let configured = resolve::<StretchKnobs, StretchPreset>(&DynamicValue::from_custom(
        ConfigValue(StretchKnobs::default()),
    ));
    assert!(matches!(configured.method, StretchMethod::AutoAsinh { .. }));
}

#[test]
#[should_panic(expected = "config input type is validated")]
fn resolver_rejects_incompatible_values() {
    resolve::<StretchKnobs, StretchPreset>(&DynamicValue::from(1.0));
}
