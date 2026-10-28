//! [`InterfaceSize`]: the whole UI's size, as the Preferences tab offers it.

use palantir::UserScale;
use serde::{Deserialize, Deserializer, Serialize};

/// The whole UI's size as a percent of the platform's scale: one of palantir's [`UserScale`]
/// rungs in [`Self::OFFERED`], as VS Code's zoom and Blender's
/// Resolution Scale size a whole UI. A rung, not a free number, as each distinct factor
/// re-rasterizes every glyph.
///
/// Persisted as its percent. A saved percent that is no offered rung loads as the nearest one,
/// so a hand-edited file or a rung palantir drops still opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub(crate) struct InterfaceSize {
    percent: u16,
}

impl InterfaceSize {
    /// The platform's own size.
    pub(crate) const DEFAULT: Self = Self { percent: 100 };

    /// Every size the Preferences tab offers, smallest first: palantir's rungs from 80 to
    /// 150 %, which a test holds to [`UserScale::LADDER`].
    pub(crate) const OFFERED: [Self; 6] = [
        Self { percent: 80 },
        Self { percent: 90 },
        Self { percent: 100 },
        Self { percent: 110 },
        Self { percent: 125 },
        Self { percent: 150 },
    ];

    /// [`Self::OFFERED`]'s labels, in its order.
    const LABELS: [&str; 6] = ["80%", "90%", "100%", "110%", "125%", "150%"];

    /// The offered size nearest `percent`; a tie takes the smaller.
    pub(crate) fn nearest(percent: u16) -> Self {
        Self::OFFERED
            .into_iter()
            .min_by_key(|size| size.percent.abs_diff(percent))
            .expect("one size is offered")
    }

    /// Where the size sits in [`Self::OFFERED`]; every size is an offered one.
    pub(crate) fn offered_idx(self) -> usize {
        Self::OFFERED
            .iter()
            .position(|size| *size == self)
            .expect("a size is an offered one")
    }

    /// The size as the Preferences tab lists it.
    pub(crate) fn label(self) -> &'static str {
        Self::LABELS[self.offered_idx()]
    }

    /// The factor palantir multiplies onto the platform's.
    pub(crate) fn user_scale(self) -> UserScale {
        UserScale::new(f32::from(self.percent) / 100.0).expect("a percent is finite")
    }
}

impl Default for InterfaceSize {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl<'de> Deserialize<'de> for InterfaceSize {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        u16::deserialize(deserializer).map(Self::nearest)
    }
}

#[cfg(test)]
mod tests {
    use common::SerdeFormat;
    use palantir::UserScale;

    use crate::core::io::preferences::interface_size::InterfaceSize;

    /// The offered sizes are palantir's rungs from 80 to 150 %, labelled by their percent; a
    /// saved percent loads as the nearest of them, a far one clamped to an end; a size
    /// round-trips and maps to its factor.
    #[test]
    fn a_size_is_a_rung_and_loads_as_the_nearest() {
        let rungs: Vec<u16> = UserScale::LADDER
            .into_iter()
            .filter_map(UserScale::new)
            .map(|rung| u16::try_from(rung.percent()).unwrap())
            .filter(|percent| (80..=150).contains(percent))
            .collect();
        let offered: Vec<u16> = InterfaceSize::OFFERED.map(|size| size.percent).into();
        assert_eq!(offered, rungs);
        for size in InterfaceSize::OFFERED {
            assert_eq!(size.label(), format!("{}%", size.percent));
        }
        for (saved, loaded) in [(100, 100), (117, 110), (118, 125), (40, 80), (400, 150)] {
            assert_eq!(InterfaceSize::nearest(saved).percent, loaded, "{saved} %");
        }
        let size = InterfaceSize::nearest(125);
        let ron = SerdeFormat::Ron.serialize(&size).unwrap();
        assert_eq!(ron, b"125");
        assert_eq!(
            SerdeFormat::Ron
                .deserialize::<InterfaceSize>(b"123")
                .unwrap(),
            size
        );
        assert_eq!(size.user_scale().get(), 1.25);
    }
}
