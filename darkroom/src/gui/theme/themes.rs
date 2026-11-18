//! [`Themes`]: darkroom's theme in each scheme, and the one a frame shows.

use palantir::{ColorScheme, Ui};

use crate::gui::theme::Theme;

/// Ayu Graphite in both schemes, built once at launch, so a turn of the scheme rebuilds nothing:
/// palantir holds the pair's widget halves and resolves the scheme each frame shows, from the
/// person's pick and the system's, and darkroom's own widgets read the half that scheme names.
#[derive(Debug)]
pub(crate) struct Themes {
    light: Theme,
    dark: Theme,
}

impl Themes {
    pub(crate) fn new() -> Self {
        Self {
            light: Theme::new(ColorScheme::Light),
            dark: Theme::new(ColorScheme::Dark),
        }
    }

    /// The theme of `scheme`.
    pub(crate) const fn get(&self, scheme: ColorScheme) -> &Theme {
        match scheme {
            ColorScheme::Light => &self.light,
            ColorScheme::Dark => &self.dark,
        }
    }

    /// The theme of the scheme `ui` shows this frame, the one palantir's own widgets paint in.
    pub(crate) const fn shown(&self, ui: &Ui) -> &Theme {
        self.get(ui.color_scheme())
    }

    /// Hand palantir each scheme's widget theme, so its own widgets paint in the scheme it shows.
    pub(crate) fn install(&self, ui: &mut Ui) {
        for scheme in ColorScheme::ALL {
            ui.set_scheme_theme(scheme, self.get(scheme).palantir.clone())
                .expect("darkroom's theme names no press effect");
        }
    }
}
