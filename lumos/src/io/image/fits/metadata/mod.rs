pub(crate) mod domain_keywords;

use fits_well::header::Header;
use fits_well::image::SampleType;

use crate::io::image::calibration_state::CalibrationState;
use crate::io::image::cfa::{CfaImage, CfaType};
use crate::io::image::image_metadata::ImageMetadata;
use crate::io::image::image_provenance::RowOrder;
use crate::io::image::unverified_conditions::UnverifiedConditions;
use crate::io::raw::demosaic::bayer::CfaPattern;
use crate::io::raw::demosaic::xtrans::xtrans_pattern::XTransPattern;

/// A metadata field and every keyword it is read from: the standard or most common spelling first,
/// which is also the one written, then the aliases other writers use, as Siril reads them
/// (`fits_keywords.c`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MetadataField {
    Object,
    Instrument,
    Telescope,
    DateObs,
    DateLocal,
    ExposureTime,
    Iso,
    Filter,
    Gain,
    Egain,
    CcdTemp,
    CameraTemp,
    ImageType,
    XBinning,
    YBinning,
    SetTemp,
    Offset,
    FocalLength,
    Airmass,
    PixelSizeX,
    PixelSizeY,
    DataMax,
}

impl MetadataField {
    /// The keywords the field is read from, the first present one winning.
    pub(super) const fn keywords(self) -> &'static [&'static str] {
        match self {
            Self::Object => &["OBJECT"],
            Self::Instrument => &["INSTRUME"],
            Self::Telescope => &["TELESCOP"],
            Self::DateObs => &["DATE-OBS"],
            // N.I.N.A., MaxIm DL and SGP write the capture time by the local clock here.
            Self::DateLocal => &["DATE-LOC"],
            Self::ExposureTime => &["EXPTIME", "EXPOSURE"],
            Self::Iso => &["ISOSPEED"],
            Self::Filter => &["FILTER", "FILT-1"],
            Self::Gain => &["GAIN"],
            Self::Egain => &["EGAIN", "CVF"],
            Self::CcdTemp => &["CCD-TEMP", "CCD_TEMP", "CCDTEMP", "TEMPERAT", "CAMTCCD"],
            // lumos's own, as `LUMWB*` is: no convention names a camera body's temperature.
            Self::CameraTemp => &["LUMCTEMP"],
            Self::ImageType => &["IMAGETYP", "FRAMETYP", "FRAME"],
            Self::XBinning => &["XBINNING", "BINX"],
            Self::YBinning => &["YBINNING", "BINY"],
            Self::SetTemp => &["SET-TEMP"],
            Self::Offset => &["OFFSET", "BLKLEVEL"],
            Self::FocalLength => &["FOCALLEN", "FOCAL", "FLENGTH"],
            Self::Airmass => &["AIRMASS"],
            Self::PixelSizeX => &["XPIXSZ", "XPIXELSZ", "PIXSIZE1", "PIXSIZEX", "XPIXSIZE"],
            Self::PixelSizeY => &["YPIXSZ", "YPIXELSZ", "PIXSIZE2", "PIXSIZEY", "YPIXSIZE"],
            Self::DataMax => &["DATAMAX"],
        }
    }

    /// The keyword the field is written as.
    const fn keyword(self) -> &'static str {
        self.keywords()[0]
    }

    /// The field's value from `header`: the first of its keywords that `read` finds in a form it
    /// can have. One given in another form is skipped, with the reason logged, for the next.
    fn read<T>(
        self,
        header: &Header,
        read: impl Fn(&Header, &'static str) -> fits_well::Result<Option<T>>,
    ) -> Option<T> {
        self.keywords()
            .iter()
            .find_map(|&keyword| optional(keyword, read(header, keyword)))
    }
}

/// The observation keywords a FITS header gives, each one `None` when it is absent or given with
/// a type or value it cannot have: none of them changes a sample, so a writer's odd choice for
/// one never costs the frame.
pub(super) fn read_metadata(header: &Header, sample_type: SampleType) -> ImageMetadata {
    let text = |field: MetadataField| field.read(header, read_text);
    let real = |field: MetadataField| field.read(header, Header::get_real);
    ImageMetadata {
        object: text(MetadataField::Object),
        instrument: text(MetadataField::Instrument),
        telescope: text(MetadataField::Telescope),
        date_obs: text(MetadataField::DateObs),
        date_local: text(MetadataField::DateLocal),
        exposure_time: MetadataField::ExposureTime.read(header, read_exposure),
        iso: MetadataField::Iso.read(header, read_u32),
        sample_type: Some(sample_type),
        camera_white_balance: optional("LUMWB*", read_camera_white_balance(header)),
        filter: text(MetadataField::Filter),
        gain: real(MetadataField::Gain),
        egain: real(MetadataField::Egain),
        ccd_temp: MetadataField::CcdTemp.read(header, read_temperature),
        camera_temp: MetadataField::CameraTemp.read(header, read_temperature),
        image_type: text(MetadataField::ImageType),
        xbinning: MetadataField::XBinning.read(header, read_i32),
        ybinning: MetadataField::YBinning.read(header, read_i32),
        set_temp: real(MetadataField::SetTemp),
        offset: MetadataField::Offset.read(header, read_i32),
        focal_length: real(MetadataField::FocalLength),
        airmass: real(MetadataField::Airmass),
        ra_deg: read_ra_deg(header),
        dec_deg: read_dec_deg(header),
        pixel_size_x: real(MetadataField::PixelSizeX),
        pixel_size_y: real(MetadataField::PixelSizeY),
        // The decoder fills these: they depend on the decode plan as well as the header.
        data_max: None,
        provenance: None,
        domain: None,
        quantization_sigma: None,
        mosaic_noise: None,
        flat_gain: None,
        saturation_flagged: false,
        calibration: read_calibration(header),
        unverified_dark: read_unverified_dark(header),
    }
}

/// The logical keyword that records each part calibration removed: bias, dark signal, flat.
const CALIBRATION_KEYWORDS: [&str; 3] = ["LUMCALB", "LUMCALD", "LUMCALF"];

/// The logical keyword that records each capture condition a dark match did not compare:
/// exposure, temperature.
const UNVERIFIED_DARK_KEYWORDS: [&str; 2] = ["LUMUVEXP", "LUMUVTMP"];

fn read_calibration(header: &Header) -> CalibrationState {
    let [bias, thermal, flat] = read_flags(header, CALIBRATION_KEYWORDS);
    CalibrationState {
        bias,
        thermal,
        flat,
    }
}

fn read_unverified_dark(header: &Header) -> UnverifiedConditions {
    let [exposure, temperature] = read_flags(header, UNVERIFIED_DARK_KEYWORDS);
    UnverifiedConditions {
        exposure,
        temperature,
    }
}

/// Each logical keyword's value, `false` when it is absent: a record keyword is written only when
/// it is set.
fn read_flags<const N: usize>(header: &Header, keywords: [&str; N]) -> [bool; N] {
    keywords.map(|keyword| optional(keyword, header.get_logical(keyword)).unwrap_or(false))
}

/// The header's `DATAMAX`, in the file's sample units; `None`, with the reason logged, when it is
/// absent or given in a form it cannot have.
pub(super) fn read_data_max(header: &Header) -> Option<f64> {
    MetadataField::DataMax.read(header, Header::get_real)
}

/// An optional keyword's value, or `None` — with the reason logged — when the header gives it in
/// a form it cannot have.
fn optional<T>(keyword: &str, value: fits_well::Result<Option<T>>) -> Option<T> {
    value.unwrap_or_else(|error| {
        tracing::warn!(keyword, %error, "ignoring a FITS keyword given in a form it cannot have");
        None
    })
}

pub(super) fn write_image_metadata(
    header: &mut Header,
    metadata: &ImageMetadata,
    image_type: Option<&str>,
) -> fits_well::Result<()> {
    use MetadataField as Field;
    set_optional_text(header, Field::Object.keyword(), metadata.object.as_deref())?;
    set_optional_text(
        header,
        Field::Instrument.keyword(),
        metadata.instrument.as_deref(),
    )?;
    set_optional_text(
        header,
        Field::Telescope.keyword(),
        metadata.telescope.as_deref(),
    )?;
    set_optional_text(
        header,
        Field::DateObs.keyword(),
        metadata.date_obs.as_deref(),
    )?;
    set_optional_text(
        header,
        Field::DateLocal.keyword(),
        metadata.date_local.as_deref(),
    )?;
    set_optional_real(
        header,
        Field::ExposureTime.keyword(),
        metadata.exposure_time,
    )?;
    set_optional_integer(header, Field::Iso.keyword(), metadata.iso.map(i64::from))?;
    set_optional_text(header, Field::Filter.keyword(), metadata.filter.as_deref())?;
    set_optional_real(header, Field::Gain.keyword(), metadata.gain)?;
    set_optional_real(header, Field::Egain.keyword(), metadata.egain)?;
    set_optional_real(header, Field::CcdTemp.keyword(), metadata.ccd_temp)?;
    set_optional_real(header, Field::CameraTemp.keyword(), metadata.camera_temp)?;
    set_optional_text(
        header,
        Field::ImageType.keyword(),
        image_type.or(metadata.image_type.as_deref()),
    )?;
    set_optional_integer(
        header,
        Field::XBinning.keyword(),
        metadata.xbinning.map(i64::from),
    )?;
    set_optional_integer(
        header,
        Field::YBinning.keyword(),
        metadata.ybinning.map(i64::from),
    )?;
    set_optional_real(header, Field::SetTemp.keyword(), metadata.set_temp)?;
    set_optional_integer(
        header,
        Field::Offset.keyword(),
        metadata.offset.map(i64::from),
    )?;
    set_optional_real(header, Field::FocalLength.keyword(), metadata.focal_length)?;
    set_optional_real(header, Field::Airmass.keyword(), metadata.airmass)?;
    set_optional_real(header, "RA", metadata.ra_deg)?;
    set_optional_real(header, "DEC", metadata.dec_deg)?;
    set_optional_real(header, Field::PixelSizeX.keyword(), metadata.pixel_size_x)?;
    set_optional_real(header, Field::PixelSizeY.keyword(), metadata.pixel_size_y)?;
    set_optional_real(header, Field::DataMax.keyword(), metadata.data_max)?;
    let CalibrationState {
        bias,
        thermal,
        flat,
    } = metadata.calibration;
    let UnverifiedConditions {
        exposure,
        temperature,
    } = metadata.unverified_dark;
    for (keyword, set) in CALIBRATION_KEYWORDS
        .into_iter()
        .zip([bias, thermal, flat])
        .chain(
            UNVERIFIED_DARK_KEYWORDS
                .into_iter()
                .zip([exposure, temperature]),
        )
    {
        if set {
            header.set(keyword, true)?;
        }
    }
    domain_keywords::write(
        header,
        metadata.domain.as_ref(),
        metadata.quantization_sigma,
        metadata.saturation_flagged,
    )?;
    Ok(())
}

pub(super) fn write_cfa_metadata(header: &mut Header, cfa: &CfaImage) -> fits_well::Result<()> {
    // Two keywords, because they answer two questions that a bottom-up source pulls apart.
    //
    // `ROWORDER` says which rows the `BAYERPAT` below is expressed against, and that is always
    // `TOP-DOWN` here: the rows go out in the order this writer holds them and the pattern was
    // already converted into those terms on load, so declaring anything else would have a reload
    // apply the phase correction a second time. Written for every sensor type, not just the
    // mosaiced ones — a mono frame has no phase to correct but is as capable of being upside-down.
    //
    // `LUMROWO` says which way up the sky is, which is what survives from the source and what two
    // frames have to agree on. Omitted for a frame this crate synthesized, which has no source to
    // report and whose reader then falls back to `ROWORDER`.
    header.set("ROWORDER", RowOrder::TopDown.keyword())?;
    if let Some(row_order) = cfa.metadata.row_order() {
        header.set(SOURCE_ROW_ORDER, row_order.keyword())?;
    }
    match cfa.cfa_type {
        CfaType::Mono => {
            header.set("CFATYPE", "MONO")?;
        }
        CfaType::Bayer(pattern) => {
            header.set("CFATYPE", "BAYER")?;
            header.set("BAYERPAT", pattern.bayerpat())?;
        }
        CfaType::XTrans(pattern) => {
            header.set("CFATYPE", "XTRANS")?;
            for (row, values) in pattern.rows().iter().enumerate() {
                let keyword = format!("XTRNROW{row}");
                let value = values
                    .iter()
                    .map(|value| char::from(b'0' + *value))
                    .collect::<String>();
                header.set(&keyword, value)?;
            }
        }
    }

    if let Some([red, green_1, blue, green_2]) = cfa.metadata.camera_white_balance {
        header.set("LUMWBR", f64::from(red))?;
        header.set("LUMWBG1", f64::from(green_1))?;
        header.set("LUMWBB", f64::from(blue))?;
        header.set("LUMWBG2", f64::from(green_2))?;
    }
    Ok(())
}

fn set_optional_text(
    header: &mut Header,
    keyword: &str,
    value: Option<&str>,
) -> fits_well::Result<()> {
    if let Some(value) = value {
        header.set(keyword, value)?;
    }
    Ok(())
}

fn set_optional_real(
    header: &mut Header,
    keyword: &str,
    value: Option<f64>,
) -> fits_well::Result<()> {
    if let Some(value) = value {
        header.set(keyword, value)?;
    }
    Ok(())
}

fn set_optional_integer(
    header: &mut Header,
    keyword: &str,
    value: Option<i64>,
) -> fits_well::Result<()> {
    if let Some(value) = value {
        header.set(keyword, value)?;
    }
    Ok(())
}

/// The mosaic pattern a header declares, or `None` for an image that is no mosaic.
/// `unstated_bayer_pattern` stands in for a `BAYERPAT` of `'TRUE'`.
pub(super) fn read_cfa_from_headers(
    header: &Header,
    height: usize,
    unstated_bayer_pattern: Option<CfaPattern>,
) -> fits_well::Result<Option<CfaType>> {
    match header.get_text("CFATYPE")? {
        Some(value) if value.eq_ignore_ascii_case("MONO") => return Ok(Some(CfaType::Mono)),
        Some(value) if value.eq_ignore_ascii_case("BAYER") => {
            return read_bayer_cfa(header, height, true, unstated_bayer_pattern);
        }
        Some(value) if value.eq_ignore_ascii_case("XTRANS") => {
            return Ok(Some(CfaType::XTrans(read_xtrans_pattern(header)?)));
        }
        Some(_) => {
            return Err(fits_well::FitsError::TypeMismatch {
                name: "CFATYPE".to_string(),
                expected: "MONO, BAYER, or XTRANS",
            });
        }
        None => {}
    }
    read_bayer_cfa(header, height, false, unstated_bayer_pattern)
}

/// Where a Lumos-written file records the row order its *source* had.
///
/// `ROWORDER` cannot carry it. This writer emits the rows in the order it holds them and the Bayer
/// pattern already converted into those terms, so its `ROWORDER` has to say `TOP-DOWN` — "the
/// pattern as written applies to the rows as written" — or a reload would apply the phase
/// correction a second time. That leaves nowhere to say which way up the sky actually is, which is
/// the question two frames have to agree on, so it goes here.
const SOURCE_ROW_ORDER: &str = "LUMROWO";

/// Which way up the stored rows actually are.
///
/// [`SOURCE_ROW_ORDER`] where this writer left one, and `ROWORDER` otherwise — for a file from
/// anyone else the two are the same thing, and only a Lumos round-trip separates them. This is the
/// one the combine holds frames to agreeing on: it is about the sky, not about the pattern.
pub(super) fn read_row_order(header: &Header) -> fits_well::Result<RowOrder> {
    if let Some(recorded) = header.get_text(SOURCE_ROW_ORDER)? {
        return Ok(row_order_of(recorded));
    }
    read_declared_row_order(header)
}

/// What `ROWORDER` itself declares, which is the frame the Bayer pattern beside it is expressed in.
///
/// The FITS standard has no `ROWORDER`; it is a convention, and a file without it is read the way
/// every writer that omits it means — first row first.
pub(super) fn read_declared_row_order(header: &Header) -> fits_well::Result<RowOrder> {
    Ok(header
        .get_text("ROWORDER")?
        .map_or(RowOrder::TopDown, row_order_of))
}

/// Anything a keyword does not spell as `BOTTOM-UP` is top-down, the order a writer that says
/// nothing at all means.
fn row_order_of(value: &str) -> RowOrder {
    if value
        .trim()
        .eq_ignore_ascii_case(RowOrder::BottomUp.keyword())
    {
        RowOrder::BottomUp
    } else {
        RowOrder::TopDown
    }
}

/// The Bayer type `header` declares for an image of `height` decoded rows.
///
/// The height is the decoded image's, not `NAXIS2`: a tile-compressed HDU is a binary table whose
/// `NAXIS2` counts tile rows.
fn read_bayer_cfa(
    header: &Header,
    height: usize,
    required: bool,
    unstated_bayer_pattern: Option<CfaPattern>,
) -> fits_well::Result<Option<CfaType>> {
    let Some(bayerpat) = header.get_text("BAYERPAT")? else {
        if required {
            return Err(fits_well::FitsError::MissingKeyword { name: "BAYERPAT" });
        }
        return Ok(None);
    };
    let stated = CfaPattern::from_bayerpat(bayerpat);
    let unstated = bayerpat.trim().eq_ignore_ascii_case("TRUE");
    let mut pattern = match (stated, unstated, unstated_bayer_pattern) {
        (Some(pattern), _, _) | (None, true, Some(pattern)) => pattern,
        (None, true, None) => {
            return Err(fits_well::FitsError::TypeMismatch {
                name: "BAYERPAT".to_string(),
                expected: "a Bayer phase; 'TRUE' states none, so FitsLoadOptions::unstated_bayer_pattern must give it",
            });
        }
        (None, false, _) => {
            return Err(fits_well::FitsError::TypeMismatch {
                name: "BAYERPAT".to_string(),
                expected: "RGGB, BGGR, GRBG, or GBRG",
            });
        }
    };

    // `BAYERPAT` describes the top-down image and the rows are left in file order, so file row `f`
    // of a `BOTTOM-UP` frame carries the phase of displayed row `H - 1 - f`. That is the opposite
    // phase only when `H` is even: for odd `H`, `H - 1 - f ≡ f (mod 2)`, the declared pattern
    // already matches the rows as stored, and flipping would invert a correct one and mis-debayer
    // the entire frame. Odd visible heights are not hypothetical — LibRaw reports 4015 for the
    // EOS 1500D.
    // `ROWORDER` rather than the resolved order: the pattern is expressed against the rows as this
    // file stores them, and a Lumos-written file separates that from which way up the sky is.
    if read_declared_row_order(header)? == RowOrder::BottomUp && height.is_multiple_of(2) {
        pattern = pattern.flip_vertical();
    }

    // The offsets are in the stored image's own coordinates — where the pattern starts within the
    // data as written — so they compose onto the pattern *after* the row-order correction above has
    // put it in file-order terms. Applying them first would express them against a display
    // orientation the samples were never rearranged into.
    let xoff = header.get_integer("XBAYROFF")?.unwrap_or(0);
    let yoff = header.get_integer("YBAYROFF")?.unwrap_or(0);
    if yoff & 1 != 0 {
        pattern = pattern.flip_vertical();
    }
    if xoff & 1 != 0 {
        pattern = pattern.flip_horizontal();
    }

    Ok(Some(CfaType::Bayer(pattern)))
}

fn read_xtrans_pattern(header: &Header) -> fits_well::Result<XTransPattern> {
    let mut pattern = [[0u8; 6]; 6];
    for (row, values) in pattern.iter_mut().enumerate() {
        let keyword = format!("XTRNROW{row}");
        let value =
            header
                .get_text(&keyword)?
                .ok_or_else(|| fits_well::FitsError::TypeMismatch {
                    name: keyword.clone(),
                    expected: "six X-Trans color digits",
                })?;
        if value.len() != 6 {
            return Err(fits_well::FitsError::TypeMismatch {
                name: keyword,
                expected: "six X-Trans color digits",
            });
        }
        for (column, byte) in value.bytes().enumerate() {
            values[column] = match byte {
                b'0'..=b'2' => byte - b'0',
                _ => {
                    return Err(fits_well::FitsError::TypeMismatch {
                        name: keyword,
                        expected: "X-Trans digits in the range 0..=2",
                    });
                }
            };
        }
    }
    XTransPattern::new(pattern).map_err(|error| fits_well::FitsError::TypeMismatch {
        name: format!("XTRNROW0..XTRNROW5 ({error})"),
        expected: "X-Trans pattern",
    })
}

fn read_camera_white_balance(header: &Header) -> fits_well::Result<Option<[f32; 4]>> {
    let values = [
        header.get_real("LUMWBR")?,
        header.get_real("LUMWBG1")?,
        header.get_real("LUMWBB")?,
        header.get_real("LUMWBG2")?,
    ];
    match values {
        [None, None, None, None] => Ok(None),
        [Some(red), Some(green_1), Some(blue), Some(green_2)] => {
            let values = [red as f32, green_1 as f32, blue as f32, green_2 as f32];
            if values.iter().all(|value| value.is_finite() && *value > 0.0) {
                Ok(Some(values))
            } else {
                Err(fits_well::FitsError::KeywordOutOfRange { name: "LUMWB*" })
            }
        }
        _ => Err(fits_well::FitsError::TypeMismatch {
            name: "LUMWB*".to_string(),
            expected: "all four white-balance multipliers or none",
        }),
    }
}

/// The pointing right ascension: `RA` in degrees, else `OBJCTRA` in sexagesimal hours, else the
/// reference value of the WCS axis whose type is right ascension.
fn read_ra_deg(header: &Header) -> Option<f64> {
    optional("RA", header.get_real("RA"))
        .or_else(|| {
            optional("OBJCTRA", header.get_text("OBJCTRA"))
                .and_then(parse_sexagesimal)
                .map(|hours| hours * 15.0)
        })
        .or_else(|| celestial_reference(header, "RA--"))
}

/// The pointing declination: `DEC` in degrees, else `OBJCTDEC` in sexagesimal degrees, else the
/// reference value of the WCS axis whose type is declination.
fn read_dec_deg(header: &Header) -> Option<f64> {
    optional("DEC", header.get_real("DEC"))
        .or_else(|| optional("OBJCTDEC", header.get_text("OBJCTDEC")).and_then(parse_sexagesimal))
        .or_else(|| celestial_reference(header, "DEC-"))
}

/// `CRVALn` of the axis whose `CTYPEn` starts with `prefix`, the four-character equatorial
/// coordinate name of FITS WCS paper II. Galactic, ecliptic and non-celestial axes name another
/// coordinate, so a header that has only those gives no pointing, and an axis-swapped one gives
/// each value from its own axis.
fn celestial_reference(header: &Header, prefix: &str) -> Option<f64> {
    let axes = optional("NAXIS", header.get_integer("NAXIS"))?;
    (1..=axes).find_map(|axis| {
        let ctype = optional("CTYPE", header.get_text(&format!("CTYPE{axis}")))?;
        if !ctype.starts_with(prefix) {
            return None;
        }
        optional("CRVAL", header.get_real(&format!("CRVAL{axis}")))
    })
}

fn parse_sexagesimal(value: &str) -> Option<f64> {
    let parts: Vec<f64> = value
        .split([' ', ':'])
        .filter(|part| !part.is_empty())
        .map(|part| part.trim().parse().ok())
        .collect::<Option<Vec<_>>>()?;
    if parts.len() != 3 {
        return None;
    }
    let sign = if parts[0].is_sign_negative() {
        -1.0
    } else {
        1.0
    };
    Some(sign * (parts[0].abs() + parts[1] / 60.0 + parts[2] / 3600.0))
}

pub(super) fn read_text(header: &Header, key: &str) -> fits_well::Result<Option<String>> {
    Ok(header.get_text(key)?.map(str::to_owned))
}

fn read_u32(header: &Header, key: &'static str) -> fits_well::Result<Option<u32>> {
    header
        .get_integer(key)?
        .map(|value| {
            u32::try_from(value).map_err(|_| fits_well::FitsError::KeywordOutOfRange { name: key })
        })
        .transpose()
}

/// Seconds, never negative: a bias is exposed for 0 s.
fn read_exposure(header: &Header, key: &'static str) -> fits_well::Result<Option<f64>> {
    read_real_where(header, key, |seconds| seconds >= 0.0)
}

/// Degrees Celsius, above absolute zero.
fn read_temperature(header: &Header, key: &'static str) -> fits_well::Result<Option<f64>> {
    read_real_where(header, key, |celsius| celsius > -273.15)
}

fn read_real_where(
    header: &Header,
    key: &'static str,
    valid: impl Fn(f64) -> bool,
) -> fits_well::Result<Option<f64>> {
    header
        .get_real(key)?
        .map(|value| {
            if value.is_finite() && valid(value) {
                Ok(value)
            } else {
                Err(fits_well::FitsError::KeywordOutOfRange { name: key })
            }
        })
        .transpose()
}

fn read_i32(header: &Header, key: &'static str) -> fits_well::Result<Option<i32>> {
    header
        .get_integer(key)?
        .map(|value| {
            i32::try_from(value).map_err(|_| fits_well::FitsError::KeywordOutOfRange { name: key })
        })
        .transpose()
}

#[cfg(test)]
mod internals {
    use crate::io::image::fits::metadata::MetadataField;

    impl MetadataField {
        pub(crate) const ALL: [Self; 22] = [
            Self::Object,
            Self::Instrument,
            Self::Telescope,
            Self::DateObs,
            Self::DateLocal,
            Self::ExposureTime,
            Self::Iso,
            Self::Filter,
            Self::Gain,
            Self::Egain,
            Self::CcdTemp,
            Self::CameraTemp,
            Self::ImageType,
            Self::XBinning,
            Self::YBinning,
            Self::SetTemp,
            Self::Offset,
            Self::FocalLength,
            Self::Airmass,
            Self::PixelSizeX,
            Self::PixelSizeY,
            Self::DataMax,
        ];
    }
}

#[cfg(test)]
mod tests;
