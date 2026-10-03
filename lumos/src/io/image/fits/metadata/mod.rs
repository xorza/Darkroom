pub(crate) mod domain_keywords;

use fits_well::header::Header;
use fits_well::image::SampleType;

use crate::io::image::cfa::{CfaImage, CfaType};
use crate::io::image::image_metadata::ImageMetadata;
use crate::io::image::image_provenance::RowOrder;
use crate::io::raw::demosaic::bayer::CfaPattern;
use crate::io::raw::demosaic::xtrans::xtrans_pattern::XTransPattern;

/// The observation keywords a FITS header gives, each one `None` when it is absent or given with
/// a type or value it cannot have: none of them changes a sample, so a writer's odd choice for
/// one never costs the frame.
pub(super) fn read_metadata(
    header: &Header,
    header_dimensions: Vec<usize>,
    sample_type: SampleType,
) -> ImageMetadata {
    let text = |key| optional(key, read_text(header, key));
    let real = |key| optional(key, header.get_real(key));
    ImageMetadata {
        object: text("OBJECT"),
        instrument: text("INSTRUME"),
        telescope: text("TELESCOP"),
        date_obs: text("DATE-OBS"),
        exposure_time: real("EXPTIME"),
        iso: optional("ISOSPEED", read_u32(header, "ISOSPEED")),
        sample_type: Some(sample_type),
        header_dimensions,
        camera_white_balance: optional("LUMWB*", read_camera_white_balance(header)),
        filter: text("FILTER"),
        gain: real("GAIN"),
        egain: real("EGAIN"),
        ccd_temp: real("CCD-TEMP").or_else(|| real("CCDTEMP")),
        image_type: text("IMAGETYP").or_else(|| text("FRAME")),
        xbinning: optional("XBINNING", read_i32(header, "XBINNING")),
        ybinning: optional("YBINNING", read_i32(header, "YBINNING")),
        set_temp: real("SET-TEMP"),
        offset: optional("OFFSET", read_i32(header, "OFFSET")),
        focal_length: real("FOCALLEN"),
        airmass: real("AIRMASS"),
        ra_deg: read_ra_deg(header),
        dec_deg: read_dec_deg(header),
        pixel_size_x: real("XPIXSZ"),
        pixel_size_y: real("YPIXSZ"),
        data_max: real("DATAMAX"),
        provenance: None,
        // The decoder fills both: they depend on the decode plan as well as the header.
        domain: None,
        quantization_sigma: None,
        calibrated: optional("LUMCAL", header.get_logical("LUMCAL")).unwrap_or(false),
    }
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
    set_optional_text(header, "OBJECT", metadata.object.as_deref())?;
    set_optional_text(header, "INSTRUME", metadata.instrument.as_deref())?;
    set_optional_text(header, "TELESCOP", metadata.telescope.as_deref())?;
    set_optional_text(header, "DATE-OBS", metadata.date_obs.as_deref())?;
    set_optional_real(header, "EXPTIME", metadata.exposure_time)?;
    set_optional_integer(header, "ISOSPEED", metadata.iso.map(i64::from))?;
    set_optional_text(header, "FILTER", metadata.filter.as_deref())?;
    set_optional_real(header, "GAIN", metadata.gain)?;
    set_optional_real(header, "EGAIN", metadata.egain)?;
    set_optional_real(header, "CCD-TEMP", metadata.ccd_temp)?;
    set_optional_text(
        header,
        "IMAGETYP",
        image_type.or(metadata.image_type.as_deref()),
    )?;
    set_optional_integer(header, "XBINNING", metadata.xbinning.map(i64::from))?;
    set_optional_integer(header, "YBINNING", metadata.ybinning.map(i64::from))?;
    set_optional_real(header, "SET-TEMP", metadata.set_temp)?;
    set_optional_integer(header, "OFFSET", metadata.offset.map(i64::from))?;
    set_optional_real(header, "FOCALLEN", metadata.focal_length)?;
    set_optional_real(header, "AIRMASS", metadata.airmass)?;
    set_optional_real(header, "RA", metadata.ra_deg)?;
    set_optional_real(header, "DEC", metadata.dec_deg)?;
    set_optional_real(header, "XPIXSZ", metadata.pixel_size_x)?;
    set_optional_real(header, "YPIXSZ", metadata.pixel_size_y)?;
    set_optional_real(header, "DATAMAX", metadata.data_max)?;
    if metadata.calibrated {
        header.set("LUMCAL", true)?;
    }
    domain_keywords::write(
        header,
        metadata.domain.as_ref(),
        metadata.quantization_sigma,
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

fn read_i32(header: &Header, key: &'static str) -> fits_well::Result<Option<i32>> {
    header
        .get_integer(key)?
        .map(|value| {
            i32::try_from(value).map_err(|_| fits_well::FitsError::KeywordOutOfRange { name: key })
        })
        .transpose()
}

#[cfg(test)]
mod tests;
