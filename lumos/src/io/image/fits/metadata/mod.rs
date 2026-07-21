use fits_well::header::Header;

use crate::io::image::cfa::{CfaImage, CfaType};
use crate::io::image::fits::provenance::FitsTransferProvenance;
use crate::io::image::{BitPix, ImageMetadata, ImageProvenance, TransferProvenance};
use crate::io::raw::demosaic::bayer::CfaPattern;
use crate::io::raw::demosaic::xtrans::XTransPattern;

pub(crate) fn read_metadata(
    header: &Header,
    header_dimensions: Vec<usize>,
    bitpix: BitPix,
) -> fits_well::Result<ImageMetadata> {
    Ok(ImageMetadata {
        object: read_text(header, "OBJECT")?,
        instrument: read_text(header, "INSTRUME")?,
        telescope: read_text(header, "TELESCOP")?,
        date_obs: read_text(header, "DATE-OBS")?,
        exposure_time: header.get_real("EXPTIME")?,
        iso: read_u32(header, "ISOSPEED")?,
        bitpix,
        header_dimensions,
        cfa_type: read_cfa_from_headers(header)?,
        camera_white_balance: read_camera_white_balance(header)?,
        filter: read_text(header, "FILTER")?,
        gain: header.get_real("GAIN")?,
        egain: header.get_real("EGAIN")?,
        ccd_temp: first_real(header, "CCD-TEMP", "CCDTEMP")?,
        image_type: first_text(header, "IMAGETYP", "FRAME")?,
        xbinning: read_i32(header, "XBINNING")?,
        ybinning: read_i32(header, "YBINNING")?,
        set_temp: header.get_real("SET-TEMP")?,
        offset: read_i32(header, "OFFSET")?,
        focal_length: header.get_real("FOCALLEN")?,
        airmass: header.get_real("AIRMASS")?,
        ra_deg: read_ra_deg(header)?,
        dec_deg: read_dec_deg(header)?,
        pixel_size_x: header.get_real("XPIXSZ")?,
        pixel_size_y: header.get_real("YPIXSZ")?,
        data_max: header.get_real("DATAMAX")?,
        provenance: None,
        calibrated: header.get_logical("LUMCAL")?.unwrap_or(false),
    })
}

pub(crate) fn write_image_metadata(
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
    if let Some(ImageProvenance {
        transfer:
            TransferProvenance::FitsPhysical(FitsTransferProvenance {
                unit: Some(unit), ..
            }),
        ..
    }) = &metadata.provenance
    {
        header.set("BUNIT", unit.as_str())?;
    }
    Ok(())
}

pub(crate) fn write_cfa_metadata(header: &mut Header, cfa: &CfaImage) -> fits_well::Result<()> {
    match cfa.metadata.cfa_type.as_ref() {
        Some(CfaType::Mono) => {
            header.set("CFATYPE", "MONO")?;
        }
        Some(CfaType::Bayer(pattern)) => {
            header.set("CFATYPE", "BAYER")?;
            header.set("BAYERPAT", bayerpat(*pattern))?;
            header.set("ROWORDER", "TOP-DOWN")?;
        }
        Some(CfaType::XTrans(pattern)) => {
            XTransPattern::new(*pattern).map_err(|_| fits_well::FitsError::TypeMismatch {
                name: "CFATYPE".to_string(),
                expected: "valid X-Trans pattern",
            })?;
            header.set("CFATYPE", "XTRANS")?;
            header.set("ROWORDER", "TOP-DOWN")?;
            for (row, values) in pattern.iter().enumerate() {
                let keyword = format!("XTRNROW{row}");
                let value = values
                    .iter()
                    .map(|value| char::from(b'0' + *value))
                    .collect::<String>();
                header.set(&keyword, value)?;
            }
        }
        None => {
            return Err(fits_well::FitsError::TypeMismatch {
                name: "CFATYPE".to_string(),
                expected: "declared CFA sensor type",
            });
        }
    }

    if let Some([red, green_1, blue, green_2]) = cfa.metadata.camera_white_balance {
        header.set("LUMWBR", f64::from(red))?;
        header.set("LUMWBG1", f64::from(green_1))?;
        header.set("LUMWBB", f64::from(blue))?;
        header.set("LUMWBG2", f64::from(green_2))?;
    }
    if let Some(sigma) = cfa.quantization_sigma {
        if !sigma.is_finite() || sigma < 0.0 {
            return Err(fits_well::FitsError::KeywordOutOfRange { name: "QNTZSIG" });
        }
        header.set("QNTZSIG", f64::from(sigma))?;
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

fn bayerpat(pattern: CfaPattern) -> &'static str {
    match pattern {
        CfaPattern::Rggb => "RGGB",
        CfaPattern::Bggr => "BGGR",
        CfaPattern::Grbg => "GRBG",
        CfaPattern::Gbrg => "GBRG",
    }
}

pub(crate) fn read_cfa_from_headers(header: &Header) -> fits_well::Result<Option<CfaType>> {
    match header.get_text("CFATYPE")? {
        Some(value) if value.eq_ignore_ascii_case("MONO") => return Ok(Some(CfaType::Mono)),
        Some(value) if value.eq_ignore_ascii_case("BAYER") => {
            return read_bayer_cfa(header, true);
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
    read_bayer_cfa(header, false)
}

fn read_bayer_cfa(header: &Header, required: bool) -> fits_well::Result<Option<CfaType>> {
    let Some(bayerpat) = header.get_text("BAYERPAT")? else {
        if required {
            return Err(fits_well::FitsError::MissingKeyword { name: "BAYERPAT" });
        }
        return Ok(None);
    };
    let Some(mut pattern) = CfaPattern::from_bayerpat(bayerpat) else {
        return Err(fits_well::FitsError::TypeMismatch {
            name: "BAYERPAT".to_string(),
            expected: "RGGB, BGGR, GRBG, or GBRG",
        });
    };

    if let Some(roworder) = header.get_text("ROWORDER")?
        && roworder.trim().eq_ignore_ascii_case("BOTTOM-UP")
    {
        pattern = pattern.flip_vertical();
    }

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

fn read_xtrans_pattern(header: &Header) -> fits_well::Result<[[u8; 6]; 6]> {
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
    XTransPattern::new(pattern).map_err(|_| fits_well::FitsError::TypeMismatch {
        name: "CFATYPE".to_string(),
        expected: "valid X-Trans pattern",
    })?;
    Ok(pattern)
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

pub(crate) fn read_quantization_sigma(header: &Header) -> fits_well::Result<Option<f32>> {
    header
        .get_real("QNTZSIG")?
        .map(|value| {
            let value = value as f32;
            if value.is_finite() && value >= 0.0 {
                Ok(value)
            } else {
                Err(fits_well::FitsError::KeywordOutOfRange { name: "QNTZSIG" })
            }
        })
        .transpose()
}

fn read_ra_deg(header: &Header) -> fits_well::Result<Option<f64>> {
    if let Some(ra) = header.get_real("RA")? {
        return Ok(Some(ra));
    }
    if let Some(value) = header.get_text("OBJCTRA")? {
        return Ok(parse_sexagesimal(value).map(|hours| hours * 15.0));
    }
    header.get_real("CRVAL1")
}

fn read_dec_deg(header: &Header) -> fits_well::Result<Option<f64>> {
    if let Some(dec) = header.get_real("DEC")? {
        return Ok(Some(dec));
    }
    if let Some(value) = header.get_text("OBJCTDEC")? {
        return Ok(parse_sexagesimal(value));
    }
    header.get_real("CRVAL2")
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

pub(crate) fn read_text(header: &Header, key: &str) -> fits_well::Result<Option<String>> {
    Ok(header.get_text(key)?.map(str::to_owned))
}

fn first_text(header: &Header, first: &str, second: &str) -> fits_well::Result<Option<String>> {
    match read_text(header, first)? {
        Some(value) => Ok(Some(value)),
        None => read_text(header, second),
    }
}

fn first_real(header: &Header, first: &str, second: &str) -> fits_well::Result<Option<f64>> {
    match header.get_real(first)? {
        Some(value) => Ok(Some(value)),
        None => header.get_real(second),
    }
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
mod tests {
    use crate::io::image::fits::metadata::parse_sexagesimal;

    #[test]
    fn sexagesimal_hms_converts_to_ra_degrees() {
        let expected = (5.0 + 35.0 / 60.0 + 17.3 / 3600.0) * 15.0;
        for sample in ["05 35 17.3", "05:35:17.3"] {
            let degrees = parse_sexagesimal(sample).unwrap() * 15.0;
            assert!(
                (degrees - expected).abs() < 1e-10,
                "{sample}: got {degrees}, expected {expected}"
            );
        }
        assert!((parse_sexagesimal("00 00 00.0").unwrap() * 15.0).abs() < 1e-10);
    }

    #[test]
    fn sexagesimal_dms_preserves_sign() {
        let negative = parse_sexagesimal("-05 23 28.0").unwrap();
        assert!((negative - -(5.0 + 23.0 / 60.0 + 28.0 / 3600.0)).abs() < 1e-10);
        let positive = parse_sexagesimal("+45:30:15.5").unwrap();
        assert!((positive - (45.0 + 30.0 / 60.0 + 15.5 / 3600.0)).abs() < 1e-10);
        assert!((parse_sexagesimal("-00 30 00.0").unwrap() - -0.5).abs() < 1e-10);
    }

    #[test]
    fn invalid_sexagesimal_values_are_rejected() {
        assert!(parse_sexagesimal("05 35").is_none());
        assert!(parse_sexagesimal("").is_none());
        assert!(parse_sexagesimal("abc def ghi").is_none());
    }
}
