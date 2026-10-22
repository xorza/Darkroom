//! A stack product's [`RunReport`] as keywords of its primary header, so the product read back
//! states what the run decided, as the one written did.

use fits_well::header::Header;

use crate::image::flag_counts::FlagCounts;
use crate::io::fits::error::{FitsLoadError, FitsRefusal};
use crate::io::fits::metadata::dark_comparison_keywords::DarkComparisonKeywords;
use crate::stack_product::run_report::RunReport;

/// A field of the report, by its keyword.
type Field<T> = (&'static str, fn(&mut RunReport) -> &mut T);

/// Each count the report holds beside its flag counts, by its keyword.
const COUNTS: [Field<u64>; 12] = [
    ("LUMUDEX", |report| &mut report.unverified_dark_exposures),
    ("LUMUDTE", |report| &mut report.unverified_dark_temperatures),
    ("LUMCTDK", |report| &mut report.camera_temperature_darks),
    ("LUMDTDF", |report| &mut report.dark_temperature_differences),
    ("LUMSCDK", |report| &mut report.scaled_darks),
    ("LUMFTDK", |report| &mut report.fitted_dark_scales),
    ("LUMFFLP", |report| &mut report.floored_flat_pixels),
    ("LUMSPIL", |report| &mut report.spilled_frames),
    ("LUMSMLN", |report| &mut report.small_n_fallback_pixels),
    ("LUMOVCB", |report| &mut report.overcommit_bytes),
    ("LUMPARK", |report| &mut report.parked_lights),
    ("LUMUNCP", |report| &mut report.unconverged_points),
];

/// Each yes or no the report holds, by its keyword.
const DECISIONS: [Field<bool>; 1] = [("LUMVBKG", |report| &mut report.variance_bg_only)];

/// The prefix of each flag count's keyword, the flag's bit after it.
const FLAG_COUNTS: [Field<FlagCounts>; 2] = [
    ("LUMEXC", |report| &mut report.excluded_samples),
    ("LUMKEP", |report| &mut report.kept_flagged_samples),
];

/// Write `report` into `header`, every field of it.
pub(super) fn write(header: &mut Header, report: &RunReport) -> fits_well::Result<()> {
    let mut report = *report;
    for (keyword, field) in COUNTS {
        header.set(keyword, integer(*field(&mut report)))?;
    }
    for (keyword, field) in DECISIONS {
        header.set(keyword, *field(&mut report))?;
    }
    for (prefix, field) in FLAG_COUNTS {
        for (bit, &count) in field(&mut report).per_bit().iter().enumerate() {
            header.set(&format!("{prefix}{bit}"), integer(count))?;
        }
    }
    DarkComparisonKeywords::FLAT_DARK.write(header, report.flat_dark)
}

/// The report `header` states, refused when a field is missing or a count is negative.
pub(super) fn read(header: &Header) -> Result<RunReport, FitsLoadError> {
    let mut report = RunReport::default();
    for (keyword, field) in COUNTS {
        *field(&mut report) = count(header, keyword)?;
    }
    for (keyword, field) in DECISIONS {
        *field(&mut report) = header
            .get_logical(keyword)?
            .ok_or_else(|| missing(keyword))?;
    }
    for (prefix, field) in FLAG_COUNTS {
        let mut per_bit = [0; 8];
        for (bit, count_of_bit) in per_bit.iter_mut().enumerate() {
            *count_of_bit = count(header, &format!("{prefix}{bit}"))?;
        }
        *field(&mut report) = FlagCounts::from_per_bit(per_bit);
    }
    report.flat_dark = DarkComparisonKeywords::FLAT_DARK.read(header);
    Ok(report)
}

/// A count as FITS states an integer: a count of samples, pixels, frames or bytes stays far below
/// `i64::MAX`.
fn integer(count: u64) -> i64 {
    i64::try_from(count).expect("a run's count fits i64")
}

fn count(header: &Header, keyword: &str) -> Result<u64, FitsLoadError> {
    let value = header
        .get_integer(keyword)?
        .ok_or_else(|| missing(keyword))?;
    u64::try_from(value).map_err(|_| {
        FitsLoadError::from(FitsRefusal::KeywordOutOfRange {
            keyword: keyword.to_owned(),
        })
    })
}

fn missing(keyword: &str) -> FitsLoadError {
    FitsLoadError::from(FitsRefusal::MissingKeyword {
        keyword: keyword.to_owned(),
    })
}
