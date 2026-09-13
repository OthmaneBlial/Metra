use std::path::Path;

use metra_core::{FileFormat, FileInfo, MetraError, ParseLimits, Result};

/// A format-independent edit for the currently supported narrow rewrite
/// surface.
///
/// Keys use the same stable namespace form emitted by [`metra_core::Tag`],
/// for example `JPEG:Comment`, `PNG:Text:Comment`, or `ID3:Title`. The
/// operation is deliberately string-based until a typed write IR can cover
/// numeric, rational, array, and binary metadata safely.
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
    let format = crate::read_path_with_limits(path, limits)?.file_info.format;
    match format {
        FileFormat::Jpeg => crate::rewrite_jpeg_path(path, limits, &collect_jpeg(edits, format)?),
        FileFormat::Tiff => crate::rewrite_tiff_path(path, limits, &collect_tiff(edits, format)?),
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
    let detected = crate::read_reader_with_limits(
        &mut std::io::Cursor::new(bytes),
        file_info.clone(),
        limits,
    )?
    .file_info
    .format;
    let file_info = FileInfo::new(file_info.path, bytes.len() as u64, detected);
    match detected {
        FileFormat::Jpeg => {
            crate::rewrite_jpeg_to_vec(bytes, file_info, limits, &collect_jpeg(edits, detected)?)
        }
        FileFormat::Tiff => {
            crate::rewrite_tiff_to_vec(bytes, file_info, limits, &collect_tiff(edits, detected)?)
        }
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
        _ => Err(unsupported_format(detected)),
    }
}

/// Copy one supported string metadata value from a source path to a target
/// path through the same read-first and atomic rewrite pipeline.
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

fn source_lookup_key(key: &str) -> &str {
    if let Some(key) = key.strip_prefix("TIFF:") {
        key
    } else if jpeg_xmp_key(key) || png_xmp_key(key) || webp_xmp_key(key) || psd_xmp_key(key) {
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
            MetadataEdit::Set { key, value } => tiff_ascii_key(key)
                .map(|key| crate::TiffEdit::SetAscii {
                    key: key.to_owned(),
                    value: value.clone(),
                })
                .ok_or_else(|| unsupported_edit(format, key)),
            MetadataEdit::Delete { key } => Err(unsupported_edit(format, key)),
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
            MetadataEdit::Set { key, value } if isobmff_text_key(key) => {
                Ok(crate::IsobmffEdit::SetText {
                    key: key.clone(),
                    value: value.clone(),
                })
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
            MetadataEdit::Delete { key } => Err(unsupported_edit(format, key)),
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
            _ => Err(unsupported_edit(format, edit.key())),
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
            MetadataEdit::Delete { key } => Err(unsupported_edit(format, key)),
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
            MetadataEdit::Set { key, value } => matroska_tag_name(key)
                .map(|name| crate::MatroskaEdit::SetTag {
                    name: name.to_owned(),
                    value: value.clone(),
                })
                .ok_or_else(|| unsupported_edit(format, key)),
            MetadataEdit::Delete { key } => Err(unsupported_edit(format, key)),
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
            MetadataEdit::Set { key, value } => wav_info_name(key)
                .map(|name| crate::WavEdit::SetInfo {
                    name: name.to_owned(),
                    value: value.clone(),
                })
                .ok_or_else(|| unsupported_edit(format, key)),
            MetadataEdit::Delete { key } => wav_info_name(key)
                .map(|name| crate::WavEdit::DeleteInfo {
                    name: name.to_owned(),
                })
                .ok_or_else(|| unsupported_edit(format, key)),
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
    matches!(key, "PSD:XMP" | "PSD:ImageResources:XMP" | "XMP:Packet")
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

        let error = collect_tiff(&[MetadataEdit::delete("TIFF:EXIF:Make")], FileFormat::Tiff)
            .expect_err("TIFF deletion is not supported by the in-place writer");
        assert!(error.to_string().contains("TIFF:EXIF:Make"));
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
}
