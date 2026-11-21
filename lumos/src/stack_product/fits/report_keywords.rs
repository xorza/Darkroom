//! A stack product's [`RunReport`] as keywords of its primary header, so the product read back
//! states what the run decided, as the one written did.

use fits_well::header::{Header, Keyword};

use crate::image::flags::flag_counts::FlagCounts;
use crate::io::fits::error::{FitsLoadError, FitsRefusal};
use crate::io::fits::metadata::dark_comparison_keywords::DarkComparisonKeywords;
use crate::stack_product::run_report::{
    CalibrationReport, CombineReport, DrizzleReport, MemoryReport, RunReport,
};

/// A field of a report group, by its keyword.
type Field<R, T> = (Keyword, fn(&mut R) -> &mut T);

/// The combine's counts, by their keywords.
const COMBINE_COUNTS: [Field<CombineReport, u64>; 1] = [(Keyword::new("LUMSMLN"), |report| {
    &mut report.small_n_fallback_pixels
})];

/// The combine's yes or no, by its keyword.
const COMBINE_DECISIONS: [Field<CombineReport, bool>; 1] = [(Keyword::new("LUMVBKG"), |report| {
    &mut report.variance_bg_only
})];

/// The prefix of each of the combine's flag counts' keywords, the flag's bit after it.
const COMBINE_FLAG_COUNTS: [Field<CombineReport, FlagCounts>; 2] = [
    (Keyword::new("LUMEXC"), |report| {
        &mut report.excluded_samples
    }),
    (Keyword::new("LUMKEP"), |report| {
        &mut report.kept_flagged_samples
    }),
];

/// The memory group's counts, by their keywords.
const MEMORY_COUNTS: [Field<MemoryReport, u64>; 3] = [
    (Keyword::new("LUMSPIL"), |report| &mut report.spilled_frames),
    (Keyword::new("LUMPARK"), |report| &mut report.parked_lights),
    (Keyword::new("LUMOVCB"), |report| {
        &mut report.overcommit_bytes
    }),
];

/// The peak the memory plan expected, by its keyword, absent for a run with no plan.
const PLANNED_PEAK: Keyword = Keyword::new("LUMPLNB");

/// The calibration group's counts, by their keywords: all present for a run that calibrated, none
/// for one that did not.
const CALIBRATION_COUNTS: [Field<CalibrationReport, u64>; 9] = [
    (Keyword::new("LUMUDEX"), |report| {
        &mut report.unverified_dark_exposures
    }),
    (Keyword::new("LUMUDTE"), |report| {
        &mut report.unverified_dark_temperatures
    }),
    (Keyword::new("LUMCTDK"), |report| {
        &mut report.camera_temperature_darks
    }),
    (Keyword::new("LUMDTDF"), |report| {
        &mut report.dark_temperature_differences
    }),
    (Keyword::new("LUMSCDK"), |report| &mut report.scaled_darks),
    (Keyword::new("LUMFTDK"), |report| {
        &mut report.fitted_dark_scales
    }),
    (Keyword::new("LUMFFLP"), |report| {
        &mut report.floored_flat_pixels
    }),
    (Keyword::new("LUMUSET"), |report| {
        &mut report.unverified_settings
    }),
    (Keyword::new("LUMFILD"), |report| {
        &mut report.filter_differences
    }),
];

/// The drizzle group's counts, by their keywords: present for a drizzle alone.
const DRIZZLE_COUNTS: [Field<DrizzleReport, u64>; 1] = [(Keyword::new("LUMUNCP"), |report| {
    &mut report.unconverged_points
})];

/// Write `report` into `header`, every group the run had.
pub(super) fn write(header: &mut Header, report: &RunReport) -> fits_well::Result<()> {
    let RunReport {
        mut combine,
        mut memory,
        calibration,
        drizzle,
    } = *report;
    write_counts(header, &mut combine, &COMBINE_COUNTS)?;
    for (keyword, field) in COMBINE_DECISIONS {
        header.set(keyword, *field(&mut combine))?;
    }
    for (prefix, field) in COMBINE_FLAG_COUNTS {
        for (bit, &count) in field(&mut combine).per_bit().iter().enumerate() {
            header.set(prefix.indexed(bit)?, integer(count))?;
        }
    }
    write_counts(header, &mut memory, &MEMORY_COUNTS)?;
    if let Some(peak) = memory.planned_peak_bytes {
        header.set(PLANNED_PEAK, integer(peak))?;
    }
    if let Some(mut calibration) = calibration {
        write_counts(header, &mut calibration, &CALIBRATION_COUNTS)?;
        DarkComparisonKeywords::FLAT_DARK.write(header, calibration.flat_dark)?;
    }
    if let Some(mut drizzle) = drizzle {
        write_counts(header, &mut drizzle, &DRIZZLE_COUNTS)?;
    }
    Ok(())
}

/// The report `header` states, refused when a field of a group it holds is missing, or a count is
/// negative.
pub(super) fn read(header: &Header) -> Result<RunReport, FitsLoadError> {
    let mut combine =
        read_counts(header, &COMBINE_COUNTS)?.ok_or_else(|| missing(COMBINE_COUNTS[0].0))?;
    for (keyword, field) in COMBINE_DECISIONS {
        *field(&mut combine) = header
            .get_logical(keyword)?
            .ok_or_else(|| missing(keyword))?;
    }
    for (prefix, field) in COMBINE_FLAG_COUNTS {
        let mut per_bit = [0; 8];
        for (bit, count_of_bit) in per_bit.iter_mut().enumerate() {
            *count_of_bit = count(header, prefix.indexed(bit)?)?;
        }
        *field(&mut combine) = FlagCounts::from_per_bit(per_bit);
    }
    let mut memory: MemoryReport =
        read_counts(header, &MEMORY_COUNTS)?.ok_or_else(|| missing(MEMORY_COUNTS[0].0))?;
    memory.planned_peak_bytes = header
        .get_integer(PLANNED_PEAK)?
        .map(|value| non_negative(PLANNED_PEAK, value))
        .transpose()?;
    let calibration =
        read_counts(header, &CALIBRATION_COUNTS)?.map(|calibration| CalibrationReport {
            flat_dark: DarkComparisonKeywords::FLAT_DARK.read(header),
            ..calibration
        });
    Ok(RunReport {
        combine,
        memory,
        calibration,
        drizzle: read_counts(header, &DRIZZLE_COUNTS)?,
    })
}

/// Write each count of `group` that `table` names.
fn write_counts<R>(
    header: &mut Header,
    group: &mut R,
    table: &[Field<R, u64>],
) -> fits_well::Result<()> {
    for &(keyword, field) in table {
        header.set(keyword, integer(*field(group)))?;
    }
    Ok(())
}

/// The group whose counts `table` names: `None` when `header` states none of them, refused when it
/// states some alone.
fn read_counts<R: Default>(
    header: &Header,
    table: &[Field<R, u64>],
) -> Result<Option<R>, FitsLoadError> {
    let mut group = R::default();
    let mut stated = false;
    let mut first_missing = None;
    for &(keyword, field) in table {
        match header.get_integer(keyword)? {
            Some(value) => {
                *field(&mut group) = non_negative(keyword, value)?;
                stated = true;
            }
            None => first_missing = first_missing.or(Some(keyword)),
        }
    }
    match (stated, first_missing) {
        (false, _) => Ok(None),
        (true, Some(keyword)) => Err(missing(keyword)),
        (true, None) => Ok(Some(group)),
    }
}

/// A count as FITS states an integer: a count of samples, pixels, frames or bytes stays far below
/// `i64::MAX`.
fn integer(count: u64) -> i64 {
    i64::try_from(count).expect("a run's count fits i64")
}

fn count(header: &Header, keyword: Keyword) -> Result<u64, FitsLoadError> {
    let value = header
        .get_integer(keyword)?
        .ok_or_else(|| missing(keyword))?;
    non_negative(keyword, value)
}

/// `value`, the integer `keyword` states, as a count: refused when negative.
fn non_negative(keyword: Keyword, value: i64) -> Result<u64, FitsLoadError> {
    u64::try_from(value).map_err(|_| {
        FitsLoadError::from(FitsRefusal::KeywordOutOfRange {
            keyword: keyword.to_string(),
        })
    })
}

fn missing(keyword: Keyword) -> FitsLoadError {
    FitsLoadError::from(FitsRefusal::MissingKeyword {
        keyword: keyword.to_string(),
    })
}
