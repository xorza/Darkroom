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
