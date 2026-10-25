//! [`QualityFlags`]: the data-quality bits of one pixel, and the level that sets
//! [`QualityFlags::SATURATED`].

/// A sample at or past this fraction of its channel's span above black is flagged
/// [`QualityFlags::SATURATED`] where the source states no clip of its own, only a nominal white
/// level. Many sensors clip a little below it, which LibRaw's own `adjust_maximum` allows for down
/// to 75% of it; 95% catches those clips. The price is the samples between 95% and the true clip,
/// which are measurements and are flagged as lower bounds all the same.
pub(crate) const SATURATION_FRACTION: f32 = 0.95;

/// One pixel's data-quality bits.
///
/// Each bit is a fact a producer knew exactly when it set it; each consumer decides what the facts
/// mean for it. That is the HST and JWST `DQ` convention: a step records, it does not repair in
/// secret.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct QualityFlags(u8);

impl QualityFlags {
    /// The pixel holds no measurement: FITS NaN or `BLANK`, a LibRaw `zero_is_bad` zero, or a
    /// calibration master with no measurement there. The sample under it is a finite fill, not
    /// data.
    pub const NO_DATA: Self = Self(1);
    /// The raw value reached the sensor's linear limit, so the sample is a lower bound, not a
    /// measurement.
    pub const SATURATED: Self = Self(1 << 1);
    /// Hot or cold in the defect map.
    pub const DEFECT: Self = Self(1 << 2);
    /// Found by L.A.Cosmic.
    pub const COSMIC_RAY: Self = Self(1 << 3);
    /// The value is an interpolation from neighbours, not the photosite's own.
    pub const REPAIRED: Self = Self(1 << 4);
    /// The flat divisor was clamped at its floor, so the value is corrected by less than its
    /// vignetting asks.
    pub const FLAT_FLOOR: Self = Self(1 << 5);
    /// The flags under which a sample is no measurement: a fill, or a bound.
    pub(crate) const UNMEASURED: Self = Self(Self::NO_DATA.0 | Self::SATURATED.0);
    /// Every bit with a meaning; a stored byte holding another was written by something else.
    pub(crate) const KNOWN: Self = {
        let mut bits = 0;
        let mut i = 0;
        while i < Self::NAMED.len() {
            bits |= Self::NAMED[i].0.0;
            i += 1;
        }
        Self(bits)
    };
    /// The flags a resampler leaves a pixel out for: the value measures nothing of the photosite —
    /// a fill under a null, a hit, a defect or a neighbours' replacement. The space-telescope
    /// pipelines (DrizzlePac, the JWST resample step) give such a pixel no weight, so it never
    /// enters an output sample, and the output's coverage records its loss.
    pub(crate) const RESAMPLE_EXCLUDED: Self =
        Self(Self::NO_DATA.0 | Self::COSMIC_RAY.0 | Self::DEFECT.0 | Self::REPAIRED.0);
    /// The flags a resampler samples through and carries to its output: the value bounds the
    /// measurement — at saturation, or corrected short at the flat's floor. It is still the best
    /// figure for its pixel, and where every frame clips a star's core the only one, so the
    /// combine leaves such samples out only while enough clean ones remain.
    pub(crate) const RESAMPLE_CARRIED: Self = Self(Self::SATURATED.0 | Self::FLAT_FLOOR.0);
    /// Each flag and its name, by bit position, as the FITS extension documents them.
    pub(crate) const NAMED: [(Self, &'static str); 6] = [
        (Self::NO_DATA, "NO_DATA"),
        (Self::SATURATED, "SATURATED"),
        (Self::DEFECT, "DEFECT"),
        (Self::COSMIC_RAY, "COSMIC_RAY"),
        (Self::REPAIRED, "REPAIRED"),
        (Self::FLAT_FLOOR, "FLAT_FLOOR"),
    ];

    /// The flags a stored byte holds.
    #[inline]
    pub(crate) const fn from_byte(byte: u8) -> Self {
        Self(byte)
    }

    /// The flags as a byte: bit `i` is the flag `1 << i`, as the `LUMFLAGS` FITS extension
    /// stores them.
    #[inline]
    pub const fn byte(self) -> u8 {
        self.0
    }

    #[inline]
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// The flags `self` and `other` both hold.
    #[inline]
    #[must_use]
    pub(crate) const fn intersection(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    /// Whether `self` and `other` hold a flag in common.
    #[inline]
    pub const fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    /// # Panics
    ///
    /// If `self` is not a single flag.
    pub(crate) const fn idx(self) -> usize {
        assert!(self.0.is_power_of_two(), "a single flag");
        self.0.trailing_zeros() as usize
    }
}
