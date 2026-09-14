use std::path::Path;

use metra_core::{FileFormat, FileInfo, MetraError, ParseLimits, Result, TagValue};

/// A format-independent edit for the currently supported narrow rewrite
/// surface.
///
/// Keys use the same stable namespace form emitted by [`metra_core::Tag`],
/// for example `JPEG:Comment`, `PNG:Text:Comment`, or `ID3:Title`. String
/// writes remain the broadest operation; [`MetadataEdit::set_value`] adds a
/// bounded typed constructor for values that have an unambiguous text encoding
/// in the current writers. The bounded `GPS:*` deletion is the one supported
/// namespace wildcard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MetadataEdit {
    Set { key: String, value: String },
    Delete { key: String },
}

impl MetadataEdit {
    /// Construct a bounded string replacement operation.
    pub fn set(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self::Set {
            key: key.into(),
            value: value.into(),
        }
    }

    /// Construct a bounded typed replacement. Only values with a validated
    /// canonical text representation for the selected writer are accepted.
    pub fn set_value(key: impl Into<String>, value: TagValue) -> Result<Self> {
        let key = key.into();
        Ok(Self::Set {
            value: typed_value_text(&key, &value)?,
            key,
        })
    }

    /// Construct a deletion operation for a supported writable key.
    pub fn delete(key: impl Into<String>) -> Self {
        Self::Delete { key: key.into() }
    }

    fn key(&self) -> &str {
        match self {
            Self::Set { key, .. } | Self::Delete { key } => key,
        }
    }
}

fn typed_value_text(key: &str, value: &TagValue) -> Result<String> {
    let invalid = || MetraError::InvalidTag {
        context: "metadata edit".to_owned(),
        message: format!("typed value is not supported for {key}"),
    };
    match value {
        TagValue::String(value) => Ok(value.clone()),
        TagValue::Unsigned(value) => Ok(value.to_string()),
        TagValue::Signed(value) => Ok(value.to_string()),
        TagValue::Float(value) if value.is_finite() => Ok(value.to_string()),
        TagValue::Date { year, month, day } if gps_date_key(key).is_some() => {
            Ok(format!("{year:04}:{month:02}:{day:02}"))
        }
        TagValue::Time {
            hour,
            minute,
            second,
            nanosecond,
        } if gps_time_key(key).is_some()
            && *hour < 24
            && *minute < 60
            && *second < 60
            && *nanosecond < 1_000_000_000 =>
        {
            let seconds = f64::from(*hour) * 3_600.0
                + f64::from(*minute) * 60.0
                + f64::from(*second)
                + f64::from(*nanosecond) / 1_000_000_000.0;
            Ok(seconds.to_string())
        }
        TagValue::DateTime {
            year,
            month,
            day,
            hour,
            minute,
            second,
            nanosecond: 0,
            offset_minutes: None,
        } if key == "WAV:DateTimeOriginal" => Ok(format!(
            "{year:04}:{month:02}:{day:02} {hour:02}:{minute:02}:{second:02}"
        )),
        TagValue::Bytes(value) if key == "XMP:Packet" => {
            String::from_utf8(value.clone()).map_err(|_| invalid())
        }
        _ => Err(invalid()),
    }
}

/// Apply canonical metadata edits to a path through the format-specific
/// validated, atomic writer.
pub fn rewrite_metadata_path(
    path: impl AsRef<Path>,
    limits: ParseLimits,
    edits: &[MetadataEdit],
) -> Result<()> {
    if edits.is_empty() {
        return Err(MetraError::InvalidTag {
            context: "metadata edit".to_owned(),
            message: "at least one edit is required".to_owned(),
        });
    }
    let path = path.as_ref();
    let metadata = crate::read_path_with_limits(path, limits)?;
    let format = metadata.file_info.format;
    match format {
        FileFormat::Jpeg => crate::rewrite_jpeg_path(path, limits, &collect_jpeg(edits, format)?),
        FileFormat::Tiff => crate::rewrite_tiff_path(path, limits, &collect_tiff(edits, format)?),
        FileFormat::Raw if is_cr3_metadata(&metadata) => {
            crate::rewrite_raw_cr3_path(path, limits, &collect_isobmff(edits, format)?)
        }
        FileFormat::Raw => {
            crate::rewrite_raw_tiff_path(path, limits, &collect_tiff(edits, format)?)
        }
        FileFormat::Png => crate::rewrite_png_path(path, limits, &collect_png(edits, format)?),
        FileFormat::Webp => crate::rewrite_webp_path(path, limits, &collect_webp(edits, format)?),
        FileFormat::Heif
        | FileFormat::Avif
        | FileFormat::Mp4
        | FileFormat::Mov
        | FileFormat::M4a => {
            crate::rewrite_isobmff_path(path, limits, &collect_isobmff(edits, format)?)
        }
        FileFormat::Gif => crate::rewrite_gif_path(path, limits, &collect_gif(edits, format)?),
        FileFormat::Mp3 => crate::rewrite_mp3_path(path, limits, &collect_mp3(edits, format)?),
        FileFormat::Flac => crate::rewrite_flac_path(path, limits, &collect_flac(edits, format)?),
        FileFormat::Ogg => crate::rewrite_ogg_path(path, limits, &collect_ogg(edits, format)?),
        FileFormat::Wav => crate::rewrite_wav_path(path, limits, &collect_wav(edits, format)?),
        FileFormat::Svg => crate::rewrite_svg_path(path, limits, &collect_svg(edits, format)?),
        FileFormat::Pdf => crate::rewrite_pdf_path(path, limits, &collect_pdf(edits, format)?),
        FileFormat::Psd => crate::rewrite_psd_path(path, limits, &collect_psd(edits, format)?),
        FileFormat::Avi => crate::rewrite_avi_path(path, limits, &collect_avi(edits, format)?),
        FileFormat::Mkv | FileFormat::Webm => {
            crate::rewrite_matroska_path(path, limits, &collect_matroska(edits, format)?)
        }
        FileFormat::Icc => crate::rewrite_icc_path(path, limits, &collect_icc(edits, format)?),
        FileFormat::Xmp => crate::rewrite_xmp_path(path, limits, &collect_xmp(edits, format)?),
        _ => Err(unsupported_format(format)),
    }
}

/// Apply canonical metadata edits to bytes and validate the rewritten result.
pub fn rewrite_metadata_to_vec(
    bytes: &[u8],
    file_info: FileInfo,
    limits: ParseLimits,
    edits: &[MetadataEdit],
) -> Result<Vec<u8>> {
    if edits.is_empty() {
        return Err(MetraError::InvalidTag {
            context: "metadata edit".to_owned(),
            message: "at least one edit is required".to_owned(),
        });
    }
    let detected_metadata = crate::read_reader_with_limits(
        &mut std::io::Cursor::new(bytes),
        file_info.clone(),
        limits,
    )?;
    let detected = detected_metadata.file_info.format;
    let file_info = FileInfo::new(file_info.path, bytes.len() as u64, detected);
    match detected {
        FileFormat::Jpeg => {
            crate::rewrite_jpeg_to_vec(bytes, file_info, limits, &collect_jpeg(edits, detected)?)
        }
        FileFormat::Tiff => {
            crate::rewrite_tiff_to_vec(bytes, file_info, limits, &collect_tiff(edits, detected)?)
        }
        FileFormat::Raw if is_cr3_metadata(&detected_metadata) => crate::rewrite_raw_cr3_to_vec(
            bytes,
            file_info,
            limits,
            &collect_isobmff(edits, detected)?,
        ),
        FileFormat::Raw => crate::rewrite_raw_tiff_to_vec(
            bytes,
            file_info,
            limits,
            &collect_tiff(edits, detected)?,
        ),
        FileFormat::Png => {
            crate::rewrite_png_to_vec(bytes, file_info, limits, &collect_png(edits, detected)?)
        }
        FileFormat::Webp => {
            crate::rewrite_webp_to_vec(bytes, file_info, limits, &collect_webp(edits, detected)?)
        }
        FileFormat::Heif
        | FileFormat::Avif
        | FileFormat::Mp4
        | FileFormat::Mov
        | FileFormat::M4a => crate::rewrite_isobmff_to_vec(
            bytes,
            file_info,
            limits,
            &collect_isobmff(edits, detected)?,
        ),
        FileFormat::Gif => {
            crate::rewrite_gif_to_vec(bytes, file_info, limits, &collect_gif(edits, detected)?)
        }
        FileFormat::Mp3 => {
            crate::rewrite_mp3_to_vec(bytes, file_info, limits, &collect_mp3(edits, detected)?)
        }
        FileFormat::Flac => {
            crate::rewrite_flac_to_vec(bytes, file_info, limits, &collect_flac(edits, detected)?)
        }
        FileFormat::Ogg => {
            crate::rewrite_ogg_to_vec(bytes, file_info, limits, &collect_ogg(edits, detected)?)
        }
        FileFormat::Wav => {
            crate::rewrite_wav_to_vec(bytes, file_info, limits, &collect_wav(edits, detected)?)
        }
        FileFormat::Svg => {
            crate::rewrite_svg_to_vec(bytes, file_info, limits, &collect_svg(edits, detected)?)
        }
        FileFormat::Pdf => {
            crate::rewrite_pdf_to_vec(bytes, file_info, limits, &collect_pdf(edits, detected)?)
        }
        FileFormat::Psd => {
            crate::rewrite_psd_to_vec(bytes, file_info, limits, &collect_psd(edits, detected)?)
        }
        FileFormat::Avi => {
            crate::rewrite_avi_to_vec(bytes, file_info, limits, &collect_avi(edits, detected)?)
        }
        FileFormat::Mkv | FileFormat::Webm => crate::rewrite_matroska_to_vec(
            bytes,
            file_info,
            limits,
            &collect_matroska(edits, detected)?,
        ),
        FileFormat::Icc => {
            crate::rewrite_icc_to_vec(bytes, file_info, limits, &collect_icc(edits, detected)?)
        }
        FileFormat::Xmp => {
            crate::rewrite_xmp_to_vec(bytes, file_info, limits, &collect_xmp(edits, detected)?)
        }
        _ => Err(unsupported_format(detected)),
    }
}

/// Copy one supported metadata value from a source path to a target path
/// through the same read-first and atomic rewrite pipeline. In addition to
/// strings, bounded derived GPS scalar values and Broadcast Wave scalar
/// fields are copied as canonical text.
pub fn copy_metadata_path(
    source: impl AsRef<Path>,
    target: impl AsRef<Path>,
    limits: ParseLimits,
    key: impl AsRef<str>,
) -> Result<()> {
    let source = source.as_ref();
    let target = target.as_ref();
    let key = key.as_ref();
    let source_metadata = crate::read_path_with_limits(source, limits)?;
    let lookup_key = source_lookup_key(key);
    let tag = source_metadata
        .find(lookup_key)
        .ok_or_else(|| MetraError::InvalidTag {
            context: "metadata copy".to_owned(),
            message: format!("source does not contain {key}"),
        })?;
    let value = match (&tag.value, lookup_key == "XMP:Packet") {
        (metra_core::TagValue::String(value), _) => value.clone(),
        (metra_core::TagValue::Bytes(value), true) => {
            String::from_utf8(value.clone()).map_err(|_| MetraError::InvalidTag {
                context: "metadata copy".to_owned(),
                message: format!("source value {key} is not valid UTF-8"),
            })?
        }
        (metra_core::TagValue::Float(value), false)
            if matches!(
                lookup_key,
                "GPS:LatitudeDecimal"
                    | "GPS:LongitudeDecimal"
                    | "GPS:AltitudeMeters"
                    | "GPS:ImageDirectionDegrees"
                    | "GPS:SpeedMetersPerSecond"
                    | "GPS:TimeOfDaySeconds"
            ) =>
        {
            value.to_string()
        }
        (metra_core::TagValue::Date { year, month, day }, false)
            if lookup_key == "GPS:GPSDateStamp" =>
        {
            format!("{year:04}:{month:02}:{day:02}")
        }
        (
            metra_core::TagValue::DateTime {
                year,
                month,
                day,
                hour,
                minute,
                second,
                nanosecond: 0,
                offset_minutes: None,
            },
            false,
        ) if lookup_key == "WAV:DateTimeOriginal" => {
            format!("{year:04}:{month:02}:{day:02} {hour:02}:{minute:02}:{second:02}")
        }
        (metra_core::TagValue::Unsigned(value), false)
            if matches!(lookup_key, "WAV:TimeReference" | "WAV:BWFVersion") =>
        {
            value.to_string()
        }
        _ => {
            return Err(MetraError::InvalidTag {
                context: "metadata copy".to_owned(),
                message: format!("source value {key} is not a single string"),
            });
        }
    };
    rewrite_metadata_path(target, limits, &[MetadataEdit::set(key, value)])
}

fn unsupported_format(format: FileFormat) -> MetraError {
    MetraError::UnsupportedFormat {
        description: format!("generic metadata edits are not implemented for {format}"),
    }
}

fn is_cr3_metadata(metadata: &metra_core::Metadata) -> bool {
    metadata.find("RAW:Variant").is_some_and(
        |tag| matches!(&tag.value, metra_core::TagValue::String(variant) if variant == "CR3"),
    )
}

fn source_lookup_key(key: &str) -> &str {
    if let Some(key) = gps_decimal_key(key) {
        match key {
            "GPS:GPSLatitude" => "GPS:LatitudeDecimal",
            "GPS:GPSLongitude" => "GPS:LongitudeDecimal",
            _ => unreachable!("GPS decimal aliases are normalized above"),
        }
    } else if let Some(key) = gps_scalar_key(key) {
        match key {
            "GPS:GPSAltitude" => "GPS:AltitudeMeters",
            "GPS:GPSImgDirection" => "GPS:ImageDirectionDegrees",
            "GPS:GPSSpeed" => "GPS:SpeedMetersPerSecond",
            _ => unreachable!("GPS scalar aliases are normalized above"),
        }
    } else if gps_time_key(key).is_some() {
        "GPS:TimeOfDaySeconds"
    } else if gps_date_key(key).is_some() {
        "GPS:GPSDateStamp"
    } else if let Some(key) = key.strip_prefix("TIFF:") {
        key
    } else if jpeg_xmp_key(key)
        || png_xmp_key(key)
        || webp_xmp_key(key)
        || psd_xmp_key(key)
        || xmp_packet_key(key)
    {
        "XMP:Packet"
    } else if let Some(key) = jpeg_exif_ascii_key(key) {
        key
    } else {
        key
    }
}

fn unsupported_edit(format: FileFormat, key: &str) -> MetraError {
    MetraError::InvalidTag {
        context: format!("{format} metadata edit"),
        message: format!("unsupported writable key {key}"),
    }
}

pub(crate) fn collect_jpeg(
    edits: &[MetadataEdit],
    format: FileFormat,
) -> Result<Vec<crate::JpegEdit>> {
    edits
        .iter()
        .map(|edit| match edit {
            MetadataEdit::Set { key, value } if key == "JPEG:Comment" => {
                Ok(crate::JpegEdit::SetComment(value.clone()))
            }
            MetadataEdit::Delete { key } if key == "JPEG:Comment" => {
                Ok(crate::JpegEdit::DeleteComments)
            }
            MetadataEdit::Set { key, value } if jpeg_xmp_key(key) => {
                Ok(crate::JpegEdit::SetXmp(value.clone()))
            }
            MetadataEdit::Delete { key } if jpeg_xmp_key(key) => Ok(crate::JpegEdit::DeleteXmp),
            MetadataEdit::Set { key, value } => {
                if let Some(exif_key) = jpeg_exif_ascii_key(key) {
                    Ok(crate::JpegEdit::SetExifAscii {
                        key: exif_key.to_owned(),
                        value: value.clone(),
                    })
                } else {
                    iptc_name(key)
                        .map(|name| crate::JpegEdit::SetIptc {
                            name: name.to_owned(),
                            value: value.clone(),
                        })
                        .ok_or_else(|| unsupported_edit(format, key))
                }
            }
            MetadataEdit::Delete { key } => iptc_name(key)
                .map(|name| crate::JpegEdit::DeleteIptc {
                    name: name.to_owned(),
                })
                .ok_or_else(|| unsupported_edit(format, key)),
        })
        .collect()
}

pub(crate) fn collect_tiff(
    edits: &[MetadataEdit],
    format: FileFormat,
) -> Result<Vec<crate::TiffEdit>> {
    edits
        .iter()
        .map(|edit| match edit {
            MetadataEdit::Set { key, value } => {
                if let Some(key) = gps_decimal_key(key) {
                    Ok(crate::TiffEdit::SetGpsDecimal {
                        key: key.to_owned(),
                        value: value.clone(),
                    })
                } else if let Some(key) = gps_scalar_key(key) {
                    Ok(crate::TiffEdit::SetGpsScalar {
                        key: key.to_owned(),
                        value: value.clone(),
                    })
                } else if let Some(key) = gps_time_key(key) {
                    Ok(crate::TiffEdit::SetGpsTime {
                        key: key.to_owned(),
                        value: value.clone(),
                    })
                } else if let Some(key) = gps_date_key(key) {
                    Ok(crate::TiffEdit::SetAscii {
                        key: key.to_owned(),
                        value: value.clone(),
                    })
                } else {
                    tiff_ascii_key(key)
                        .map(|key| crate::TiffEdit::SetAscii {
                            key: key.to_owned(),
                            value: value.clone(),
                        })
                        .ok_or_else(|| unsupported_edit(format, key))
                }
            }
            MetadataEdit::Delete { key } if key == "GPS:*" => Ok(crate::TiffEdit::DeleteGpsAll),
            MetadataEdit::Delete { key } => {
                if let Some(key) = gps_decimal_key(key) {
                    Ok(crate::TiffEdit::DeleteGpsDecimal {
                        key: key.to_owned(),
                    })
                } else if let Some(key) = gps_scalar_key(key) {
                    Ok(crate::TiffEdit::DeleteGpsScalar {
                        key: key.to_owned(),
                    })
                } else if let Some(key) = gps_time_key(key) {
                    Ok(crate::TiffEdit::DeleteGpsTime {
                        key: key.to_owned(),
                    })
                } else if let Some(key) = gps_date_key(key) {
                    Ok(crate::TiffEdit::DeleteAscii {
                        key: key.to_owned(),
                    })
                } else {
                    tiff_ascii_key(key)
                        .map(|key| crate::TiffEdit::DeleteAscii {
                            key: key.to_owned(),
                        })
                        .ok_or_else(|| unsupported_edit(format, key))
                }
            }
        })
        .collect()
}

pub(crate) fn collect_png(
    edits: &[MetadataEdit],
    format: FileFormat,
) -> Result<Vec<crate::PngEdit>> {
    edits
        .iter()
        .map(|edit| match edit {
            MetadataEdit::Set { key, value } if png_xmp_key(key) => {
                Ok(crate::PngEdit::SetXmp(value.clone()))
            }
            MetadataEdit::Delete { key } if png_xmp_key(key) => Ok(crate::PngEdit::DeleteXmp),
            MetadataEdit::Set { key, value } => png_text_keyword(key)
                .map(|keyword| crate::PngEdit::SetText {
                    keyword: keyword.to_owned(),
                    value: value.clone(),
                })
                .ok_or_else(|| unsupported_edit(format, key)),
            MetadataEdit::Delete { key } => png_text_keyword(key)
                .map(|keyword| crate::PngEdit::DeleteText {
                    keyword: keyword.to_owned(),
                })
                .ok_or_else(|| unsupported_edit(format, key)),
        })
        .collect()
}

pub(crate) fn collect_webp(
    edits: &[MetadataEdit],
    format: FileFormat,
) -> Result<Vec<crate::WebpEdit>> {
    edits
        .iter()
        .map(|edit| match edit {
            MetadataEdit::Set { key, value } if webp_xmp_key(key) => {
                Ok(crate::WebpEdit::SetXmp(value.clone()))
            }
            MetadataEdit::Delete { key } if webp_xmp_key(key) => Ok(crate::WebpEdit::DeleteXmp),
            _ => Err(unsupported_edit(format, edit.key())),
        })
        .collect()
}

pub(crate) fn collect_isobmff(
    edits: &[MetadataEdit],
    format: FileFormat,
) -> Result<Vec<crate::IsobmffEdit>> {
    edits
        .iter()
        .map(|edit| match edit {
            MetadataEdit::Set { key, value } if isobmff_xmp_key(key) => {
                Ok(crate::IsobmffEdit::SetXmp {
                    value: value.clone(),
                })
            }
            MetadataEdit::Delete { key } if isobmff_xmp_key(key) => {
                Ok(crate::IsobmffEdit::DeleteXmp)
            }
            MetadataEdit::Set { key, value } if isobmff_text_key(key) => {
                Ok(crate::IsobmffEdit::SetText {
                    key: key.clone(),
                    value: value.clone(),
                })
            }
            MetadataEdit::Delete { key } if isobmff_text_key(key) => {
                Ok(crate::IsobmffEdit::DeleteText { key: key.clone() })
            }
            _ => Err(unsupported_edit(format, edit.key())),
        })
        .collect()
}

pub(crate) fn collect_gif(
    edits: &[MetadataEdit],
    format: FileFormat,
) -> Result<Vec<crate::GifEdit>> {
    edits
        .iter()
        .map(|edit| match edit {
            MetadataEdit::Set { key, value } if key == "GIF:Comment" => {
                Ok(crate::GifEdit::SetComment(value.clone()))
            }
            MetadataEdit::Delete { key } if key == "GIF:Comment" => {
                Ok(crate::GifEdit::DeleteComments)
            }
            _ => Err(unsupported_edit(format, edit.key())),
        })
        .collect()
}

pub(crate) fn collect_mp3(
    edits: &[MetadataEdit],
    format: FileFormat,
) -> Result<Vec<crate::Mp3Edit>> {
    edits
        .iter()
        .map(|edit| match edit {
            MetadataEdit::Set { key, value } if key == "ID3:Comment" => {
                Ok(crate::Mp3Edit::SetComment(value.clone()))
            }
            MetadataEdit::Delete { key } if key == "ID3:Comment" => {
                Ok(crate::Mp3Edit::DeleteComments)
            }
            MetadataEdit::Set { key, value } => mp3_text_name(key)
                .map(|name| crate::Mp3Edit::SetText {
                    name: name.to_owned(),
                    value: value.clone(),
                })
                .ok_or_else(|| unsupported_edit(format, key)),
            MetadataEdit::Delete { key } => mp3_text_name(key)
                .map(|name| crate::Mp3Edit::DeleteText {
                    name: name.to_owned(),
                })
                .ok_or_else(|| unsupported_edit(format, key)),
        })
        .collect()
}

pub(crate) fn collect_flac(
    edits: &[MetadataEdit],
    format: FileFormat,
) -> Result<Vec<crate::FlacEdit>> {
    edits
        .iter()
        .map(|edit| match edit {
            MetadataEdit::Set { key, value } => flac_comment_name(key)
                .map(|key| crate::FlacEdit::SetComment {
                    key: key.to_owned(),
                    value: value.clone(),
                })
                .ok_or_else(|| unsupported_edit(format, key)),
            MetadataEdit::Delete { key } => flac_comment_name(key)
                .map(|key| crate::FlacEdit::DeleteComment {
                    key: key.to_owned(),
                })
                .ok_or_else(|| unsupported_edit(format, key)),
        })
        .collect()
}

pub(crate) fn collect_ogg(
    edits: &[MetadataEdit],
    format: FileFormat,
) -> Result<Vec<crate::OggEdit>> {
    edits
        .iter()
        .map(|edit| match edit {
            MetadataEdit::Set { key, value } => ogg_comment_name(key)
                .map(|key| crate::OggEdit::SetComment {
                    key: key.to_owned(),
                    value: value.clone(),
                })
                .ok_or_else(|| unsupported_edit(format, key)),
            MetadataEdit::Delete { key } => ogg_comment_name(key)
                .map(|key| crate::OggEdit::DeleteComment {
                    key: key.to_owned(),
                })
                .ok_or_else(|| unsupported_edit(format, key)),
        })
        .collect()
}

pub(crate) fn collect_pdf(
    edits: &[MetadataEdit],
    format: FileFormat,
) -> Result<Vec<crate::PdfEdit>> {
    edits
        .iter()
        .map(|edit| match edit {
            MetadataEdit::Set { key, value } => pdf_info_name(key)
                .map(|name| crate::PdfEdit::SetInfo {
                    name: name.to_owned(),
                    value: value.clone(),
                })
                .ok_or_else(|| unsupported_edit(format, key)),
            MetadataEdit::Delete { key } => pdf_info_name(key)
                .map(|name| crate::PdfEdit::DeleteInfo {
                    name: name.to_owned(),
                })
                .ok_or_else(|| unsupported_edit(format, key)),
        })
        .collect()
}

pub(crate) fn collect_psd(
    edits: &[MetadataEdit],
    format: FileFormat,
) -> Result<Vec<crate::PsdEdit>> {
    edits
        .iter()
        .map(|edit| match edit {
            MetadataEdit::Set { key, value } if psd_xmp_key(key) => {
                Ok(crate::PsdEdit::SetXmp(value.clone()))
            }
            MetadataEdit::Delete { key } if psd_xmp_key(key) => Ok(crate::PsdEdit::DeleteXmp),
            _ => Err(unsupported_edit(format, edit.key())),
        })
        .collect()
}

pub(crate) fn collect_xmp(
    edits: &[MetadataEdit],
    format: FileFormat,
) -> Result<Vec<crate::XmpEdit>> {
    edits
        .iter()
        .map(|edit| match edit {
            MetadataEdit::Set { key, value } if xmp_packet_key(key) => {
                Ok(crate::XmpEdit::SetPacket(value.clone()))
            }
            MetadataEdit::Delete { key } if xmp_packet_key(key) => Ok(crate::XmpEdit::DeletePacket),
            _ => Err(unsupported_edit(format, edit.key())),
        })
        .collect()
}

pub(crate) fn collect_icc(
    edits: &[MetadataEdit],
    format: FileFormat,
) -> Result<Vec<crate::IccEdit>> {
    edits
        .iter()
        .map(|edit| match edit {
            MetadataEdit::Set { key, value } => icc_text_name(key)
                .map(|name| crate::IccEdit::SetText {
                    name: name.to_owned(),
                    value: value.clone(),
                })
                .ok_or_else(|| unsupported_edit(format, key)),
            MetadataEdit::Delete { key } => icc_text_name(key)
                .map(|name| crate::IccEdit::DeleteText {
                    name: name.to_owned(),
                })
                .ok_or_else(|| unsupported_edit(format, key)),
        })
        .collect()
}

pub(crate) fn collect_avi(
    edits: &[MetadataEdit],
    format: FileFormat,
) -> Result<Vec<crate::AviEdit>> {
    edits
        .iter()
        .map(|edit| match edit {
            MetadataEdit::Set { key, value } => avi_info_name(key)
                .map(|name| crate::AviEdit::SetInfo {
                    name: name.to_owned(),
                    value: value.clone(),
                })
                .ok_or_else(|| unsupported_edit(format, key)),
            MetadataEdit::Delete { key } => avi_info_name(key)
                .map(|name| crate::AviEdit::DeleteInfo {
                    name: name.to_owned(),
                })
                .ok_or_else(|| unsupported_edit(format, key)),
        })
        .collect()
}

pub(crate) fn collect_matroska(
    edits: &[MetadataEdit],
    format: FileFormat,
) -> Result<Vec<crate::MatroskaEdit>> {
    edits
        .iter()
        .map(|edit| match edit {
            MetadataEdit::Set { key, value } => {
                if let Some(name) = matroska_tag_name(key) {
                    Ok(crate::MatroskaEdit::SetTag {
                        name: name.to_owned(),
                        value: value.clone(),
                    })
                } else if matroska_info_key(key) {
                    Ok(crate::MatroskaEdit::SetString {
                        key: key.clone(),
                        value: value.clone(),
                    })
                } else {
                    Err(unsupported_edit(format, key))
                }
            }
            MetadataEdit::Delete { key } => {
                if let Some(name) = matroska_tag_name(key) {
                    Ok(crate::MatroskaEdit::DeleteTag {
                        name: name.to_owned(),
                    })
                } else if matroska_info_key(key) {
                    Ok(crate::MatroskaEdit::DeleteString { key: key.clone() })
                } else {
                    Err(unsupported_edit(format, key))
                }
            }
        })
        .collect()
}

pub(crate) fn collect_wav(
    edits: &[MetadataEdit],
    format: FileFormat,
) -> Result<Vec<crate::WavEdit>> {
    edits
        .iter()
        .map(|edit| match edit {
            MetadataEdit::Set { key, value } => {
                if let Some(name) = wav_info_name(key) {
                    Ok(crate::WavEdit::SetInfo {
                        name: name.to_owned(),
                        value: value.clone(),
                    })
                } else if let Some(name) = wav_bext_name(key) {
                    Ok(crate::WavEdit::SetBext {
                        name: name.to_owned(),
                        value: value.clone(),
                    })
                } else {
                    Err(unsupported_edit(format, key))
                }
            }
            MetadataEdit::Delete { key } => {
                if let Some(name) = wav_info_name(key) {
                    Ok(crate::WavEdit::DeleteInfo {
                        name: name.to_owned(),
                    })
                } else if let Some(name) = wav_bext_name(key) {
                    Ok(crate::WavEdit::DeleteBext {
                        name: name.to_owned(),
                    })
                } else {
                    Err(unsupported_edit(format, key))
                }
            }
        })
        .collect()
}

pub(crate) fn collect_svg(
    edits: &[MetadataEdit],
    format: FileFormat,
) -> Result<Vec<crate::SvgEdit>> {
    edits
        .iter()
        .map(|edit| match edit {
            MetadataEdit::Set { key, value } => match key.as_str() {
                "SVG:Title" => Ok(crate::SvgEdit::SetTitle(value.clone())),
                "SVG:Description" => Ok(crate::SvgEdit::SetDescription(value.clone())),
                "SVG:Comment" => Ok(crate::SvgEdit::SetComment(value.clone())),
                _ => Err(unsupported_edit(format, key)),
            },
            MetadataEdit::Delete { key } => match key.as_str() {
                "SVG:Title" => Ok(crate::SvgEdit::DeleteTitles),
                "SVG:Description" => Ok(crate::SvgEdit::DeleteDescriptions),
                "SVG:Comment" => Ok(crate::SvgEdit::DeleteComments),
                _ => Err(unsupported_edit(format, key)),
            },
        })
        .collect()
}

fn jpeg_xmp_key(key: &str) -> bool {
    matches!(key, "JPEG:XMP" | "JPEG:APP1:XMP")
}

fn jpeg_exif_ascii_key(key: &str) -> Option<&str> {
    let key = key.strip_prefix("JPEG:").unwrap_or(key);
    key.starts_with("EXIF:").then_some(key)
}

fn tiff_ascii_key(key: &str) -> Option<&str> {
    let key = key.strip_prefix("TIFF:")?;
    ["EXIF:", "GPS:", "Interop:", "DNG:"]
        .iter()
        .any(|prefix| key.starts_with(prefix))
        .then_some(key)
}

fn gps_decimal_key(key: &str) -> Option<&'static str> {
    let key = key.strip_prefix("TIFF:").unwrap_or(key);
    match key {
        "GPS:Latitude" | "GPS:LatitudeDecimal" | "GPS:GPSLatitude" => Some("GPS:GPSLatitude"),
        "GPS:Longitude" | "GPS:LongitudeDecimal" | "GPS:GPSLongitude" => Some("GPS:GPSLongitude"),
        _ => None,
    }
}

fn gps_scalar_key(key: &str) -> Option<&'static str> {
    let key = key.strip_prefix("TIFF:").unwrap_or(key);
    match key {
        "GPS:Altitude" | "GPS:AltitudeMeters" | "GPS:GPSAltitude" => Some("GPS:GPSAltitude"),
        "GPS:ImageDirection" | "GPS:ImageDirectionDegrees" | "GPS:GPSImgDirection" => {
            Some("GPS:GPSImgDirection")
        }
        "GPS:Speed" | "GPS:SpeedMetersPerSecond" | "GPS:GPSSpeed" => Some("GPS:GPSSpeed"),
        _ => None,
    }
}

fn gps_time_key(key: &str) -> Option<&'static str> {
    let key = key.strip_prefix("TIFF:").unwrap_or(key);
    matches!(key, "GPS:TimeOfDaySeconds" | "GPS:GPSTimeStamp").then_some("GPS:GPSTimeStamp")
}

fn gps_date_key(key: &str) -> Option<&'static str> {
    let key = key.strip_prefix("TIFF:").unwrap_or(key);
    matches!(key, "GPS:Date" | "GPS:GPSDateStamp").then_some("GPS:GPSDateStamp")
}

fn png_xmp_key(key: &str) -> bool {
    matches!(key, "PNG:XMP" | "PNG:iTXt:XMP")
}

fn png_text_keyword(key: &str) -> Option<&str> {
    let keyword = key.strip_prefix("PNG:Text:")?;
    (!keyword.is_empty()).then_some(keyword)
}

fn webp_xmp_key(key: &str) -> bool {
    matches!(key, "WebP:XMP" | "WEBP:XMP")
}

fn psd_xmp_key(key: &str) -> bool {
    matches!(key, "PSD:XMP" | "PSD:ImageResources:XMP")
}

fn xmp_packet_key(key: &str) -> bool {
    key == "XMP:Packet"
}

fn icc_text_name(key: &str) -> Option<&str> {
    let name = key.strip_prefix("ICC:")?;
    matches!(
        name,
        "Description" | "Copyright" | "ManufacturerDescription" | "ModelDescription"
    )
    .then_some(name)
}

fn avi_info_name(key: &str) -> Option<&str> {
    let name = key.strip_prefix("AVI:")?;
    matches!(
        name,
        "Title"
            | "Artist"
            | "Comment"
            | "Copyright"
            | "Software"
            | "Genre"
            | "Product"
            | "Keywords"
            | "DateTime"
    )
    .then_some(name)
}

fn matroska_tag_name(key: &str) -> Option<&str> {
    let name = key.strip_prefix("Matroska:Tag:")?;
    (!name.is_empty()).then_some(name)
}

fn matroska_info_key(key: &str) -> bool {
    matches!(
        key,
        "Matroska:Title" | "Matroska:MuxingApp" | "Matroska:WritingApp"
    )
}

fn pdf_info_name(key: &str) -> Option<&str> {
    let name = key.strip_prefix("PDF:")?;
    matches!(
        name,
        "Title"
            | "Author"
            | "Subject"
            | "Keywords"
            | "Creator"
            | "Producer"
            | "CreationDate"
            | "ModifyDate"
    )
    .then_some(name)
}

fn isobmff_text_key(key: &str) -> bool {
    matches!(
        key,
        "ISOBMFF:Title"
            | "ISOBMFF:Artist"
            | "ISOBMFF:Album"
            | "ISOBMFF:Year"
            | "ISOBMFF:Comment"
            | "ISOBMFF:AlbumArtist"
            | "ISOBMFF:Description"
            | "ISOBMFF:PurchaseDate"
            | "ISOBMFF:Encoder"
    )
}

fn isobmff_xmp_key(key: &str) -> bool {
    matches!(key, "ISOBMFF:XMP" | "ISOBMFF:UUID:XMP")
}

fn iptc_name(key: &str) -> Option<&str> {
    let name = key.strip_prefix("IPTC:")?;
    matches!(
        name,
        "ObjectName"
            | "EditStatus"
            | "Urgency"
            | "Category"
            | "SupplementalCategories"
            | "Keywords"
            | "DateCreated"
            | "TimeCreated"
            | "Byline"
            | "BylineTitle"
            | "City"
            | "SubLocation"
            | "ProvinceState"
            | "CountryCode"
            | "Country"
            | "Headline"
            | "Credit"
            | "Source"
            | "CopyrightNotice"
            | "CaptionAbstract"
            | "WriterEditor"
    )
    .then_some(name)
}

fn wav_info_name(key: &str) -> Option<&str> {
    let name = key.strip_prefix("WAV:")?;
    matches!(
        name,
        "Title"
            | "Artist"
            | "Product"
            | "Comment"
            | "CreationDate"
            | "Genre"
            | "Engineer"
            | "Software"
            | "Copyright"
            | "Technician"
            | "Subject"
            | "Source"
    )
    .then_some(name)
}

fn wav_bext_name(key: &str) -> Option<&str> {
    let name = key.strip_prefix("WAV:")?;
    matches!(
        name,
        "Description"
            | "Originator"
            | "OriginatorReference"
            | "DateTimeOriginal"
            | "TimeReference"
            | "BWFVersion"
            | "BWF_UMID"
            | "CodingHistory"
    )
    .then_some(name)
}

fn flac_comment_name(key: &str) -> Option<&str> {
    let name = key.strip_prefix("FLAC:")?;
    matches!(
        name,
        "Title"
            | "Artist"
            | "Album"
            | "AlbumArtist"
            | "Date"
            | "Genre"
            | "TrackNumber"
            | "DiscNumber"
            | "Comment"
            | "Composer"
            | "Copyright"
            | "Description"
            | "Encoder"
            | "License"
            | "Organization"
            | "ISRC"
    )
    .then_some(name)
}

fn ogg_comment_name(key: &str) -> Option<&str> {
    let name = key.strip_prefix("Ogg:")?;
    let name = name.strip_prefix("Comment:").unwrap_or(name);
    matches!(
        name,
        "Title"
            | "Artist"
            | "Album"
            | "AlbumArtist"
            | "Date"
            | "Genre"
            | "TrackNumber"
            | "DiscNumber"
            | "Comment"
            | "Composer"
            | "Copyright"
            | "Description"
            | "Encoder"
            | "License"
            | "Organization"
            | "ISRC"
    )
    .then_some(name)
}

fn mp3_text_name(key: &str) -> Option<&str> {
    let name = key.strip_prefix("ID3:")?;
    matches!(
        name,
        "Title"
            | "Artist"
            | "AlbumArtist"
            | "Album"
            | "RecordingDate"
            | "Genre"
            | "TrackNumber"
            | "DiscNumber"
            | "Composer"
            | "BPM"
            | "DurationMilliseconds"
            | "Copyright"
            | "Publisher"
            | "EncodedBy"
            | "EncoderSettings"
            | "AlbumSortOrder"
            | "ArtistSortOrder"
            | "TitleSortOrder"
            | "OriginalReleaseDate"
            | "ReleaseDate"
            | "InitialKey"
            | "Language"
            | "ContentGroup"
            | "Subtitle"
            | "FileType"
            | "MediaType"
    )
    .then_some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructors_preserve_canonical_keys() {
        assert_eq!(
            MetadataEdit::set("JPEG:Comment", "reviewed"),
            MetadataEdit::Set {
                key: "JPEG:Comment".to_owned(),
                value: "reviewed".to_owned(),
            }
        );
        assert_eq!(MetadataEdit::delete("PNG:XMP").key(), "PNG:XMP");
    }

    #[test]
    fn collectors_reject_cross_format_or_unsupported_keys() {
        let error = collect_png(
            &[MetadataEdit::set("JPEG:Comment", "wrong format")],
            FileFormat::Png,
        )
        .expect_err("a PNG collector must reject JPEG keys");
        assert!(error.to_string().contains("JPEG:Comment"));

        let error = collect_tiff(&[MetadataEdit::delete("JPEG:Comment")], FileFormat::Tiff)
            .expect_err("cross-format TIFF deletion must remain unsupported");
        assert!(error.to_string().contains("JPEG:Comment"));
    }

    #[test]
    fn jpeg_collector_accepts_canonical_exif_ascii_keys() {
        assert_eq!(
            collect_jpeg(&[MetadataEdit::set("EXIF:Make", "Sony")], FileFormat::Jpeg,).unwrap(),
            vec![crate::JpegEdit::SetExifAscii {
                key: "EXIF:Make".to_owned(),
                value: "Sony".to_owned(),
            }]
        );
        assert_eq!(
            collect_jpeg(
                &[MetadataEdit::set("JPEG:EXIF:Make", "Sony")],
                FileFormat::Jpeg,
            )
            .unwrap(),
            vec![crate::JpegEdit::SetExifAscii {
                key: "EXIF:Make".to_owned(),
                value: "Sony".to_owned(),
            }]
        );
    }

    #[test]
    fn isobmff_collector_accepts_canonical_text_deletion() {
        assert_eq!(
            collect_isobmff(&[MetadataEdit::delete("ISOBMFF:Title")], FileFormat::Mp4,).unwrap(),
            vec![crate::IsobmffEdit::DeleteText {
                key: "ISOBMFF:Title".to_owned(),
            }]
        );
    }

    #[test]
    fn isobmff_collector_accepts_embedded_xmp_aliases() {
        assert_eq!(
            collect_isobmff(
                &[MetadataEdit::set("ISOBMFF:XMP", "<x:xmpmeta/>")],
                FileFormat::Heif,
            )
            .unwrap(),
            vec![crate::IsobmffEdit::SetXmp {
                value: "<x:xmpmeta/>".to_owned(),
            }]
        );
        assert_eq!(
            collect_isobmff(
                &[MetadataEdit::delete("ISOBMFF:UUID:XMP")],
                FileFormat::Avif,
            )
            .unwrap(),
            vec![crate::IsobmffEdit::DeleteXmp]
        );
    }

    #[test]
    fn tiff_collector_accepts_gps_decimal_aliases() {
        assert_eq!(
            collect_tiff(
                &[MetadataEdit::set("GPS:Latitude", "48.8566")],
                FileFormat::Tiff,
            )
            .unwrap(),
            vec![crate::TiffEdit::SetGpsDecimal {
                key: "GPS:GPSLatitude".to_owned(),
                value: "48.8566".to_owned(),
            }]
        );
        assert_eq!(
            collect_tiff(
                &[MetadataEdit::delete("TIFF:GPS:LongitudeDecimal")],
                FileFormat::Raw,
            )
            .unwrap(),
            vec![crate::TiffEdit::DeleteGpsDecimal {
                key: "GPS:GPSLongitude".to_owned(),
            }]
        );
        assert_eq!(
            collect_tiff(
                &[MetadataEdit::set("GPS:AltitudeMeters", "-125.5")],
                FileFormat::Tiff,
            )
            .unwrap(),
            vec![crate::TiffEdit::SetGpsScalar {
                key: "GPS:GPSAltitude".to_owned(),
                value: "-125.5".to_owned(),
            }]
        );
        assert_eq!(
            collect_tiff(
                &[MetadataEdit::delete("TIFF:GPS:SpeedMetersPerSecond")],
                FileFormat::Raw,
            )
            .unwrap(),
            vec![crate::TiffEdit::DeleteGpsScalar {
                key: "GPS:GPSSpeed".to_owned(),
            }]
        );
        assert_eq!(
            collect_tiff(
                &[MetadataEdit::set("GPS:TimeOfDaySeconds", "45296.125")],
                FileFormat::Tiff,
            )
            .unwrap(),
            vec![crate::TiffEdit::SetGpsTime {
                key: "GPS:GPSTimeStamp".to_owned(),
                value: "45296.125".to_owned(),
            }]
        );
    }

    #[test]
    fn typed_values_use_writer_specific_canonical_text() {
        assert_eq!(
            collect_tiff(
                &[MetadataEdit::set_value(
                    "GPS:Date",
                    TagValue::Date {
                        year: 2026,
                        month: 9,
                        day: 14,
                    },
                )
                .unwrap()],
                FileFormat::Tiff,
            )
            .unwrap(),
            vec![crate::TiffEdit::SetAscii {
                key: "GPS:GPSDateStamp".to_owned(),
                value: "2026:09:14".to_owned(),
            }]
        );
        assert_eq!(
            collect_tiff(
                &[MetadataEdit::set_value(
                    "GPS:TimeOfDaySeconds",
                    TagValue::Time {
                        hour: 12,
                        minute: 34,
                        second: 56,
                        nanosecond: 125_000_000,
                    },
                )
                .unwrap()],
                FileFormat::Tiff,
            )
            .unwrap(),
            vec![crate::TiffEdit::SetGpsTime {
                key: "GPS:GPSTimeStamp".to_owned(),
                value: "45296.125".to_owned(),
            }]
        );
        assert_eq!(
            collect_wav(
                &[MetadataEdit::set_value(
                    "WAV:DateTimeOriginal",
                    TagValue::DateTime {
                        year: 2026,
                        month: 9,
                        day: 14,
                        hour: 1,
                        minute: 2,
                        second: 3,
                        nanosecond: 0,
                        offset_minutes: None,
                    },
                )
                .unwrap()],
                FileFormat::Wav,
            )
            .unwrap(),
            vec![crate::WavEdit::SetBext {
                name: "DateTimeOriginal".to_owned(),
                value: "2026:09:14 01:02:03".to_owned(),
            }]
        );
    }

    #[test]
    fn typed_values_reject_ambiguous_structures_and_non_finite_floats() {
        let error = MetadataEdit::set_value(
            "PNG:Text:Author",
            TagValue::Array(vec![TagValue::String("not a scalar".to_owned())]),
        )
        .expect_err("structured PNG text must be rejected");
        assert!(error.to_string().contains("typed value is not supported"));

        let error = MetadataEdit::set_value("GPS:Latitude", TagValue::Float(f64::NAN))
            .expect_err("non-finite typed GPS values must be rejected");
        assert!(error.to_string().contains("typed value is not supported"));
    }

    #[test]
    fn matroska_collector_accepts_canonical_text_deletion() {
        assert_eq!(
            collect_matroska(
                &[MetadataEdit::delete("Matroska:Tag:TITLE")],
                FileFormat::Webm,
            )
            .unwrap(),
            vec![crate::MatroskaEdit::DeleteTag {
                name: "TITLE".to_owned(),
            }]
        );
        assert_eq!(
            collect_matroska(&[MetadataEdit::delete("Matroska:Title")], FileFormat::Webm,).unwrap(),
            vec![crate::MatroskaEdit::DeleteString {
                key: "Matroska:Title".to_owned(),
            }]
        );
    }

    #[test]
    fn icc_collector_accepts_canonical_text_edits() {
        assert_eq!(
            collect_icc(
                &[MetadataEdit::set("ICC:Description", "new")],
                FileFormat::Icc,
            )
            .unwrap(),
            vec![crate::IccEdit::SetText {
                name: "Description".to_owned(),
                value: "new".to_owned(),
            }]
        );
        assert_eq!(
            collect_icc(&[MetadataEdit::delete("ICC:Copyright")], FileFormat::Icc,).unwrap(),
            vec![crate::IccEdit::DeleteText {
                name: "Copyright".to_owned(),
            }]
        );
    }

    #[test]
    fn pdf_collector_accepts_canonical_info_deletion() {
        assert_eq!(
            collect_pdf(&[MetadataEdit::delete("PDF:Title")], FileFormat::Pdf).unwrap(),
            vec![crate::PdfEdit::DeleteInfo {
                name: "Title".to_owned(),
            }]
        );
    }

    #[test]
    fn psd_collector_accepts_canonical_xmp_deletion() {
        assert_eq!(
            collect_psd(&[MetadataEdit::delete("PSD:XMP")], FileFormat::Psd).unwrap(),
            vec![crate::PsdEdit::DeleteXmp]
        );
    }
}
