//! The persisted theme *preference* — the `system`/`dark`/`light` choice
//! the user makes, stored in `Preferences`. Frontend-agnostic (just a
//! serialized enum); the GUI's `theme` module resolves it to a concrete
//! palette via `ThemeChoice::resolve`.

/// The user's persisted theme preference, as offered in the Theme menu.
/// `System` follows the OS light/dark setting (re-resolved each launch by
/// the GUI's `theme` module); `Dark`/`Light` pin a palette regardless of
/// the OS. Serialized into `Preferences`.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ThemeChoice {
    /// Follow the OS light/dark preference, re-resolved on each launch.
    #[default]
    System,
    Dark,
    Light,
}

/// Which built-in palette built this [`Theme`](crate::gui::theme::Theme) — the concrete palette
/// a [`ThemeChoice`] resolves to. Carried on the theme itself and
/// round-tripped through TOML so a loaded theme file restores its
/// origin palette. `Default = Dark` so a hand-rolled `Theme` (e.g. the
/// deserialised round-trip used by tests) has a deterministic tag
/// without callers having to spell it out.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ThemePreset {
    #[default]
    Dark,
    Light,
}

impl ThemePreset {
    /// The OS's current light/dark preference, falling back to
    /// [`Dark`](Self::Dark) when the platform reports no preference or
    /// detection fails. Backs [`ThemeChoice::System`].
    pub(crate) fn from_system() -> Self {
        match dark_light::detect() {
            Ok(dark_light::Mode::Light) => Self::Light,
            Ok(dark_light::Mode::Dark | dark_light::Mode::Unspecified) | Err(_) => Self::Dark,
        }
    }
}

impl ThemeChoice {
    /// Resolve to the concrete built-in preset to load. `System` queries
    /// the OS (falling back to dark); `Dark` / `Light` map straight
    /// through.
    pub(crate) fn resolve(self) -> ThemePreset {
        match self {
            Self::System => ThemePreset::from_system(),
            Self::Dark => ThemePreset::Dark,
            Self::Light => ThemePreset::Light,
        }
    }
}
