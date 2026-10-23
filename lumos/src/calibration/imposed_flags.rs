//! [`of`]: the flags a calibration pass imposes on what it calibrates, from its masters.

use arrayvec::ArrayVec;

use crate::calibration::prepared_flat::PreparedFlat;
use crate::image::cfa::cfa_image::CfaImage;
use crate::image::pixel_flags::PixelFlags;
use crate::image::quality_flags::QualityFlags;

/// What a pass imposes from the `masters` it applies and the `flat` it divides by: `NO_DATA`
/// wherever one of them holds no measurement — a fill, or a saturated bound — as the value it
/// applies there is not one either, and `FLAT_FLOOR` wherever the flat floored its gain. `None`
/// when it imposes nothing.
pub(crate) fn of<'a>(
    masters: impl IntoIterator<Item = &'a CfaImage>,
    flat: Option<&PreparedFlat>,
) -> Option<PixelFlags> {
    let unmeasured: ArrayVec<&PixelFlags, 3> = masters
        .into_iter()
        .filter_map(CfaImage::flags)
        .filter(|flags| flags.contains(QualityFlags::UNMEASURED))
        .collect();
    let floored = flat.filter(|flat| flat.floored() > 0);
    let size = unmeasured
        .first()
        .map(|flags| flags.size())
        .or_else(|| floored.map(|flat| flat.divisor().size()))?;
    PixelFlags::from_fn(size, |idx| {
        let mut flags = QualityFlags::default();
        if unmeasured
            .iter()
            .any(|master| master.at(idx).intersects(QualityFlags::UNMEASURED))
        {
            flags = flags.union(QualityFlags::NO_DATA);
        }
        if floored.is_some_and(|flat| flat.is_floored(idx)) {
            flags = flags.union(QualityFlags::FLAT_FLOOR);
        }
        flags
    })
}
