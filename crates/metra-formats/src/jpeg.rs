use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use metra_core::{
    FileFormat, FileInfo, Metadata, MetraError, ParseLimits, Result, Source, Tag, TagValue,
    ValueType, Warning,
};

use crate::icc::parse_icc_profile;
use crate::iptc::parse_photoshop_resources;
use crate::tiff::parse_tiff_from_reader;
use crate::xmp::parse_xmp;

/// The first rewrite surface is intentionally narrow: JPEG COM segments are
/// self-contained, bounded, and can be changed without re-encoding pixels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JpegEdit {
    SetComment(String),
    DeleteComments,
}

pub fn read_jpeg<R: Read + Seek>(
    reader: &mut R,
    file_info: FileInfo,
    limits: ParseLimits,
) -> Result<Metadata> {
    let path = file_info.path.clone();
    let mut metadata = Metadata::new(file_info);
    let mut soi = [0_u8; 2];
    read_exact(reader, &mut soi, &path)?;
    if soi != [0xFF, 0xD8] {
        return Err(MetraError::InvalidHeader {
            context: "JPEG".to_owned(),
            message: "missing SOI marker".to_owned(),
        });
    }

    let mut offset = 2_u64;
    let mut segments = 0_usize;
    let mut consumed_metadata = 0_usize;
    loop {
        if segments >= limits.max_jpeg_segments {
            metadata.add_warning(
                Warning::new(
                    "jpeg-segment-limit",
                    format!("stopped after {} JPEG segments", limits.max_jpeg_segments),
                )
                .at(offset),
            );
            break;
        }
        let marker = match read_marker(reader, &mut offset, &path)? {
            Some(marker) => marker,
            None => {
                metadata.add_warning(Warning::new(
                    "truncated-jpeg",
                    "JPEG ended before an EOI or SOS marker",
                ));
                break;
            }
        };
        if marker == 0xD9 || marker == 0xDA {
            break;
        }
        if (0xD0..=0xD7).contains(&marker) || marker == 0x01 {
            segments += 1;
            continue;
        }

        let mut length_bytes = [0_u8; 2];
        read_exact(reader, &mut length_bytes, &path)?;
        offset = offset.checked_add(2).ok_or(MetraError::InvalidOffset {
            context: "JPEG segment length".to_owned(),
            offset,
        })?;
        let segment_length = usize::from(u16::from_be_bytes(length_bytes));
        if segment_length < 2 {
            return Err(MetraError::InvalidTag {
                context: format!("JPEG marker 0xFF{marker:02X}"),
                message: "segment length must include its two length bytes".to_owned(),
            });
        }
        let data_length = segment_length - 2;
        let data_offset = offset;
        let remaining_budget = limits.max_metadata_bytes.saturating_sub(consumed_metadata);
        if data_length > remaining_budget {
            metadata.add_warning(
                Warning::new(
                    "jpeg-metadata-limit",
                    format!("skipped {data_length}-byte segment after metadata budget was reached"),
                )
                .at(data_offset),
            );
            seek_forward(reader, data_length, &path)?;
            offset = offset
                .checked_add(u64::try_from(data_length).unwrap_or(u64::MAX))
                .ok_or(MetraError::InvalidOffset {
                    context: "JPEG segment end".to_owned(),
                    offset: data_offset,
                })?;
            segments += 1;
            continue;
        }

        let mut data = vec![0_u8; data_length];
        read_exact(reader, &mut data, &path)?;
        consumed_metadata = consumed_metadata.saturating_add(data_length);
        offset = offset
            .checked_add(
                u64::try_from(data_length).map_err(|_| MetraError::InvalidOffset {
                    context: "JPEG segment length".to_owned(),
                    offset: data_length as u64,
                })?,
            )
            .ok_or(MetraError::InvalidOffset {
                context: "JPEG segment end".to_owned(),
                offset: data_offset,
            })?;
        process_segment(marker, &data, data_offset, &mut metadata, limits)?;
        segments += 1;
    }
    metadata.sort_tags();
    Ok(metadata)
}

/// Validate the source, stream a JPEG rewrite to the destination, and keep
/// every segment that is not explicitly targeted by `edits` byte-for-byte.
pub fn rewrite_jpeg<R: Read + Seek, W: Write>(
    reader: &mut R,
    writer: &mut W,
    file_info: FileInfo,
    limits: ParseLimits,
    edits: &[JpegEdit],
) -> Result<()> {
    read_jpeg(reader, file_info.clone(), limits)?;
    reader
        .seek(SeekFrom::Start(0))
        .map_err(|source| io_error(&file_info.path, source))?;
    rewrite_jpeg_stream(reader, writer, &file_info.path, limits, edits)
}

/// Rewrite a JPEG in memory and validate the resulting container before
/// returning it. This is useful for callers that want to preview or stage a
/// change without touching the source file.
pub fn rewrite_jpeg_to_vec(
    bytes: &[u8],
    file_info: FileInfo,
    limits: ParseLimits,
    edits: &[JpegEdit],
) -> Result<Vec<u8>> {
    let mut reader = std::io::Cursor::new(bytes);
    let mut output = Vec::new();
    rewrite_jpeg(&mut reader, &mut output, file_info.clone(), limits, edits)?;
    let mut validation_reader = std::io::Cursor::new(output.as_slice());
    read_jpeg(&mut validation_reader, file_info, limits)?;
    Ok(output)
}

/// Atomically replace a JPEG after validating both the source and rewritten
/// output. The temporary file is created beside the source so the final rename
/// stays on the same filesystem.
pub fn rewrite_jpeg_path(
    path: impl AsRef<Path>,
    limits: ParseLimits,
    edits: &[JpegEdit],
) -> Result<()> {
    let path = path.as_ref().to_path_buf();
    let source_metadata = fs::metadata(&path).map_err(|source| MetraError::Io {
        path: path.clone(),
        source,
    })?;
    let file_info = FileInfo::new(path.clone(), source_metadata.len(), FileFormat::Jpeg);
    let temp_path = temporary_path(&path)?;
    let result = (|| {
        let mut input = File::open(&path).map_err(|source| MetraError::Io {
            path: path.clone(),
            source,
        })?;
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
            .map_err(|source| MetraError::WriteFailure {
                message: format!("cannot create {}: {source}", temp_path.display()),
            })?;
        rewrite_jpeg(&mut input, &mut output, file_info.clone(), limits, edits)?;
        output
            .sync_all()
            .map_err(|source| MetraError::WriteFailure {
                message: format!("cannot sync {}: {source}", temp_path.display()),
            })?;
        drop(output);

        let mut validation = File::open(&temp_path).map_err(|source| MetraError::Io {
            path: temp_path.clone(),
            source,
        })?;
        let written_size = validation
            .metadata()
            .map_err(|source| MetraError::Io {
                path: temp_path.clone(),
                source,
            })?
            .len();
        let written_info = FileInfo::new(temp_path.clone(), written_size, FileFormat::Jpeg);
        read_jpeg(&mut validation, written_info, limits)?;
        fs::set_permissions(&temp_path, source_metadata.permissions()).map_err(|source| {
            MetraError::WriteFailure {
                message: format!(
                    "cannot preserve permissions on {}: {source}",
                    temp_path.display()
                ),
            }
        })?;
        fs::rename(&temp_path, &path).map_err(|source| MetraError::WriteFailure {
            message: format!("cannot atomically replace {}: {source}", path.display()),
        })?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result
}

#[derive(Debug)]
enum CommentAction {
    Set(Vec<u8>),
    Delete,
}

fn rewrite_jpeg_stream<R: Read, W: Write>(
    reader: &mut R,
    writer: &mut W,
    path: &Path,
    limits: ParseLimits,
    edits: &[JpegEdit],
) -> Result<()> {
    let action = comment_action(edits)?;
    let mut soi = [0_u8; 2];
    read_exact(reader, &mut soi, path)?;
    if soi != [0xFF, 0xD8] {
        return Err(MetraError::InvalidHeader {
            context: "JPEG".to_owned(),
            message: "missing SOI marker".to_owned(),
        });
    }
    write_all(writer, &soi)?;

    let mut segments = 0_usize;
    let mut metadata_bytes = 0_usize;
    let mut inserted = false;
    loop {
        if segments >= limits.max_jpeg_segments {
            return Err(MetraError::ResourceLimitExceeded {
                resource: "JPEG segments during rewrite".to_owned(),
                limit: limits.max_jpeg_segments,
            });
        }
        let Some(marker_bytes) = read_marker_bytes(reader, path)? else {
            return Err(MetraError::CorruptMetadata {
                message: "JPEG ended before SOS or EOI during rewrite".to_owned(),
            });
        };
        let marker = *marker_bytes.last().unwrap_or(&0);
        if marker == 0xD9 || marker == 0xDA {
            if let Some(CommentAction::Set(comment)) = action.as_ref()
                && !inserted
            {
                write_comment(writer, comment)?;
            }
            write_all(writer, &marker_bytes)?;
            if marker == 0xDA {
                copy_remainder(reader, writer, path)?;
            }
            return Ok(());
        }
        if (0xD0..=0xD7).contains(&marker) || marker == 0x01 {
            write_all(writer, &marker_bytes)?;
            segments += 1;
            continue;
        }

        let mut length_bytes = [0_u8; 2];
        read_exact(reader, &mut length_bytes, path)?;
        let segment_length = usize::from(u16::from_be_bytes(length_bytes));
        if segment_length < 2 {
            return Err(MetraError::InvalidTag {
                context: format!("JPEG marker 0xFF{marker:02X}"),
                message: "segment length must include its two length bytes".to_owned(),
            });
        }
        let data_length = segment_length - 2;
        metadata_bytes = metadata_bytes.saturating_add(data_length);
        if metadata_bytes > limits.max_metadata_bytes {
            return Err(MetraError::ResourceLimitExceeded {
                resource: "JPEG metadata during rewrite".to_owned(),
                limit: limits.max_metadata_bytes,
            });
        }
        let mut data = vec![0_u8; data_length];
        read_exact(reader, &mut data, path)?;
        if marker == 0xFE {
            match action.as_ref() {
                Some(CommentAction::Set(comment)) if !inserted => {
                    write_comment(writer, comment)?;
                    inserted = true;
                }
                Some(CommentAction::Set(_)) | Some(CommentAction::Delete) => {}
                None => {
                    write_all(writer, &marker_bytes)?;
                    write_all(writer, &length_bytes)?;
                    write_all(writer, &data)?;
                }
            }
        } else {
            write_all(writer, &marker_bytes)?;
            write_all(writer, &length_bytes)?;
            write_all(writer, &data)?;
        }
        segments += 1;
    }
}

fn comment_action(edits: &[JpegEdit]) -> Result<Option<CommentAction>> {
    let mut action = None;
    for edit in edits {
        match edit {
            JpegEdit::SetComment(comment) => {
                let bytes = comment.as_bytes().to_vec();
                let length =
                    bytes
                        .len()
                        .checked_add(2)
                        .ok_or_else(|| MetraError::WriteFailure {
                            message: "JPEG comment length overflowed".to_owned(),
                        })?;
                if u16::try_from(length).is_err() {
                    return Err(MetraError::WriteFailure {
                        message: "JPEG comment exceeds the 65533-byte segment limit".to_owned(),
                    });
                }
                action = Some(CommentAction::Set(bytes));
            }
            JpegEdit::DeleteComments => action = Some(CommentAction::Delete),
        }
    }
    Ok(action)
}

fn write_comment<W: Write>(writer: &mut W, comment: &[u8]) -> Result<()> {
    let length = u16::try_from(comment.len() + 2).map_err(|_| MetraError::WriteFailure {
        message: "JPEG comment exceeds the 65533-byte segment limit".to_owned(),
    })?;
    write_all(writer, &[0xFF, 0xFE])?;
    write_all(writer, &length.to_be_bytes())?;
    write_all(writer, comment)
}

fn read_marker_bytes<R: Read>(reader: &mut R, path: &Path) -> Result<Option<Vec<u8>>> {
    let mut first = [0_u8; 1];
    match reader.read(&mut first) {
        Ok(0) => return Ok(None),
        Ok(1) => {}
        Ok(_) => unreachable!("a one-byte buffer cannot read more than one byte"),
        Err(error) => return Err(io_error(path, error)),
    }
    if first[0] != 0xFF {
        return Err(MetraError::CorruptMetadata {
            message: "expected JPEG marker during rewrite".to_owned(),
        });
    }
    let mut marker = vec![first[0]];
    loop {
        let mut byte = [0_u8; 1];
        read_exact(reader, &mut byte, path)?;
        marker.push(byte[0]);
        if byte[0] != 0xFF {
            break;
        }
    }
    if marker.last() == Some(&0x00) {
        return Err(MetraError::CorruptMetadata {
            message: "entropy-coded data appeared before SOS during rewrite".to_owned(),
        });
    }
    Ok(Some(marker))
}

fn copy_remainder<R: Read, W: Write>(reader: &mut R, writer: &mut W, path: &Path) -> Result<()> {
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|source| io_error(path, source))?;
        if read == 0 {
            return Ok(());
        }
        write_all(writer, &buffer[..read])?;
    }
}

fn write_all<W: Write>(writer: &mut W, bytes: &[u8]) -> Result<()> {
    writer
        .write_all(bytes)
        .map_err(|source| MetraError::WriteFailure {
            message: source.to_string(),
        })
}

fn temporary_path(path: &Path) -> Result<PathBuf> {
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("metadata.jpg");
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| MetraError::WriteFailure {
            message: format!("cannot create temporary name: {error}"),
        })?
        .as_nanos();
    Ok(path.with_file_name(format!(
        ".{filename}.metra-{}-{timestamp}.tmp",
        std::process::id()
    )))
}

fn read_marker<R: Read>(reader: &mut R, offset: &mut u64, path: &Path) -> Result<Option<u8>> {
    let mut byte = [0_u8; 1];
    match reader.read_exact(&mut byte) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => {
            return Ok(None);
        }
        Err(error) => return Err(io_error(path, error)),
    }
    *offset = offset.saturating_add(1);
    if byte[0] != 0xFF {
        return Err(MetraError::CorruptMetadata {
            message: format!(
                "expected JPEG marker at offset {}",
                offset.saturating_sub(1)
            ),
        });
    }
    loop {
        read_exact(reader, &mut byte, path)?;
        *offset = offset.saturating_add(1);
        if byte[0] != 0xFF {
            break;
        }
    }
    if byte[0] == 0x00 {
        return Err(MetraError::CorruptMetadata {
            message: "entropy-coded data appeared before SOS".to_owned(),
        });
    }
    Ok(Some(byte[0]))
}

fn process_segment(
    marker: u8,
    data: &[u8],
    data_offset: u64,
    metadata: &mut Metadata,
    limits: ParseLimits,
) -> Result<()> {
    match marker {
        0xE1 if data.starts_with(b"Exif\0\0") => {
            if data.len() <= 6 {
                metadata.add_warning(Warning::new(
                    "empty-exif",
                    "JPEG APP1 segment has an EXIF header but no TIFF payload",
                ));
                return Ok(());
            }
            let tiff_data = &data[6..];
            let mut cursor = std::io::Cursor::new(tiff_data);
            if let Err(error) = parse_tiff_from_reader(
                &mut cursor,
                0,
                tiff_data.len() as u64,
                data_offset.saturating_add(6),
                metadata,
                limits,
            ) {
                metadata.add_warning(
                    Warning::new("invalid-exif", error.to_string()).at(data_offset + 6),
                );
            }
        }
        0xE1 if data.starts_with(b"http://ns.adobe.com/xap/1.0/\0") => {
            let prefix_len = b"http://ns.adobe.com/xap/1.0/\0".len();
            if let Err(error) = parse_xmp(
                &data[prefix_len..],
                data_offset.saturating_add(prefix_len as u64),
                "JPEG/APP1-XMP",
                metadata,
                limits,
            ) {
                metadata
                    .add_warning(Warning::new("invalid-xmp", error.to_string()).at(data_offset));
            }
        }
        0xE2 if data.starts_with(b"ICC_PROFILE\0") => {
            let prefix_len = b"ICC_PROFILE\0".len();
            if data.len() < prefix_len + 2 {
                metadata.add_warning(
                    Warning::new("truncated-icc", "ICC APP2 header is truncated").at(data_offset),
                );
            } else {
                let sequence = data[prefix_len];
                let total = data[prefix_len + 1];
                if total != 1 || sequence != 1 {
                    metadata.add_warning(
                        Warning::new(
                            "icc-fragment",
                            format!(
                                "ICC profile fragment {sequence} of {total} is not reassembled yet"
                            ),
                        )
                        .at(data_offset),
                    );
                } else if let Err(error) = parse_icc_profile(
                    &data[prefix_len + 2..],
                    data_offset + u64::try_from(prefix_len + 2).unwrap_or(u64::MAX),
                    metadata,
                    limits,
                ) {
                    metadata.add_warning(
                        Warning::new("invalid-icc", error.to_string()).at(data_offset),
                    );
                }
            }
        }
        0xED if data.starts_with(b"Photoshop 3.0\0") => {
            if let Err(error) = parse_photoshop_resources(data, data_offset, metadata, limits) {
                metadata.add_warning(
                    Warning::new("invalid-photoshop", error.to_string()).at(data_offset),
                );
            }
        }
        0xE0 if data.starts_with(b"JFIF\0") => parse_jfif(data, data_offset, metadata),
        0xFE => {
            let value = String::from_utf8_lossy(data).into_owned();
            metadata.add_tag(Tag {
                namespace: "JPEG".to_owned(),
                group: "COM".to_owned(),
                id: None,
                name: "Comment".to_owned(),
                description: Some("JPEG comment".to_owned()),
                raw_value: Some(data.to_vec()),
                value: TagValue::String(value),
                value_type: ValueType::String,
                source: Source::new("JPEG/COM", Some(data_offset), Some(data.len() as u64)),
                writable: false,
            });
        }
        _ => {}
    }
    Ok(())
}

fn parse_jfif(data: &[u8], data_offset: u64, metadata: &mut Metadata) {
    if data.len() < 14 {
        metadata.add_warning(
            Warning::new(
                "truncated-jfif",
                "JFIF segment is shorter than its fixed header",
            )
            .at(data_offset),
        );
        return;
    }
    let version = format!("{}.{}", data[5], data[6]);
    let units = match data[7] {
        0 => "None",
        1 => "inches",
        2 => "cm",
        other => {
            metadata.add_warning(
                Warning::new(
                    "invalid-jfif-units",
                    format!("unknown JFIF density unit {other}"),
                )
                .at(data_offset.saturating_add(7)),
            );
            "Unknown"
        }
    };
    add_jpeg_tag(
        metadata,
        "JFIFVersion",
        TagValue::String(version),
        ValueType::String,
        data_offset.saturating_add(5),
        2,
    );
    add_jpeg_tag(
        metadata,
        "ResolutionUnit",
        TagValue::String(units.to_owned()),
        ValueType::String,
        data_offset.saturating_add(7),
        1,
    );
    add_jpeg_tag(
        metadata,
        "XResolution",
        TagValue::Unsigned(u64::from(u16::from_be_bytes([data[8], data[9]]))),
        ValueType::UnsignedInteger,
        data_offset.saturating_add(8),
        2,
    );
    add_jpeg_tag(
        metadata,
        "YResolution",
        TagValue::Unsigned(u64::from(u16::from_be_bytes([data[10], data[11]]))),
        ValueType::UnsignedInteger,
        data_offset.saturating_add(10),
        2,
    );
}

fn add_jpeg_tag(
    metadata: &mut Metadata,
    name: &str,
    value: TagValue,
    value_type: ValueType,
    offset: u64,
    length: u64,
) {
    metadata.add_tag(Tag {
        namespace: "JFIF".to_owned(),
        group: "APP0".to_owned(),
        id: None,
        name: name.to_owned(),
        description: Some("JFIF container property".to_owned()),
        raw_value: None,
        value,
        value_type,
        source: Source::new("JPEG/APP0", Some(offset), Some(length)),
        writable: false,
    });
}

fn seek_forward<R: Seek>(reader: &mut R, length: usize, path: &Path) -> Result<()> {
    let distance = i64::try_from(length).map_err(|_| MetraError::InvalidOffset {
        context: "JPEG segment skip".to_owned(),
        offset: length as u64,
    })?;
    reader
        .seek(SeekFrom::Current(distance))
        .map(|_| ())
        .map_err(|source| io_error(path, source))
}

fn read_exact<R: Read>(reader: &mut R, buffer: &mut [u8], path: &Path) -> Result<()> {
    reader
        .read_exact(buffer)
        .map_err(|source| io_error(path, source))
}

fn io_error(path: &Path, source: std::io::Error) -> MetraError {
    if source.kind() == std::io::ErrorKind::UnexpectedEof {
        MetraError::UnexpectedEof {
            context: path.display().to_string(),
        }
    } else {
        MetraError::Io {
            path: path.to_path_buf(),
            source,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;
    use metra_core::FileFormat;

    fn jpeg_with_comment() -> Vec<u8> {
        let comment = b"hello from Metra";
        let length = u16::try_from(comment.len() + 2).unwrap();
        let mut bytes = vec![0xFF, 0xD8, 0xFF, 0xFE];
        bytes.extend_from_slice(&length.to_be_bytes());
        bytes.extend_from_slice(comment);
        bytes.extend_from_slice(&[0xFF, 0xD9]);
        bytes
    }

    fn jpeg_with_unknown_segment_and_comment() -> Vec<u8> {
        let comment = b"before";
        let comment_length = u16::try_from(comment.len() + 2).unwrap();
        let mut bytes = vec![0xFF, 0xD8, 0xFF, 0xE2, 0, 4, 1, 2];
        bytes.extend_from_slice(&[0xFF, 0xFE]);
        bytes.extend_from_slice(&comment_length.to_be_bytes());
        bytes.extend_from_slice(comment);
        bytes.extend_from_slice(&[0xFF, 0xD9]);
        bytes
    }

    #[test]
    fn reads_jpeg_comment_without_decoding_pixels() {
        let bytes = jpeg_with_comment();
        let info = FileInfo::new("comment.jpg".into(), bytes.len() as u64, FileFormat::Jpeg);
        let metadata = read_jpeg(&mut Cursor::new(bytes), info, ParseLimits::default()).unwrap();
        assert_eq!(
            metadata.find("JPEG:Comment").unwrap().display_value(),
            "hello from Metra"
        );
    }

    #[test]
    fn reads_structured_xmp_from_app1() {
        let packet = br#"<x:xmpmeta><rdf:RDF><rdf:Description dc:format="image/jpeg" xmlns:dc="urn:dc"/></rdf:RDF></x:xmpmeta>"#;
        let mut data = b"http://ns.adobe.com/xap/1.0/\0".to_vec();
        data.extend_from_slice(packet);
        let length = u16::try_from(data.len() + 2).unwrap();
        let mut bytes = vec![0xFF, 0xD8, 0xFF, 0xE1];
        bytes.extend_from_slice(&length.to_be_bytes());
        bytes.extend_from_slice(&data);
        bytes.extend_from_slice(&[0xFF, 0xD9]);
        let info = FileInfo::new("xmp.jpg".into(), bytes.len() as u64, FileFormat::Jpeg);
        let metadata = read_jpeg(&mut Cursor::new(bytes), info, ParseLimits::default()).unwrap();
        assert_eq!(
            metadata.find("XMP:dc:format").unwrap().display_value(),
            "image/jpeg"
        );
    }

    #[test]
    fn malformed_segment_length_is_rejected() {
        let bytes = vec![0xFF, 0xD8, 0xFF, 0xE1, 0, 1];
        let info = FileInfo::new("bad.jpg".into(), bytes.len() as u64, FileFormat::Jpeg);
        let result = read_jpeg(&mut Cursor::new(bytes), info, ParseLimits::default());
        assert!(matches!(result, Err(MetraError::InvalidTag { .. })));
    }

    #[test]
    fn rewrites_comment_and_preserves_unknown_segments() {
        let bytes = jpeg_with_unknown_segment_and_comment();
        let info = FileInfo::new("rewrite.jpg".into(), bytes.len() as u64, FileFormat::Jpeg);
        let output = rewrite_jpeg_to_vec(
            &bytes,
            info.clone(),
            ParseLimits::default(),
            &[JpegEdit::SetComment("after".to_owned())],
        )
        .unwrap();
        assert!(
            output
                .windows(6)
                .any(|window| window == [0xFF, 0xE2, 0, 4, 1, 2])
        );
        let metadata = read_jpeg(
            &mut Cursor::new(output),
            FileInfo::new("rewrite.jpg".into(), 0, FileFormat::Jpeg),
            ParseLimits::default(),
        )
        .unwrap();
        assert_eq!(
            metadata.find("JPEG:Comment").unwrap().display_value(),
            "after"
        );
    }

    #[test]
    fn deletes_all_comments_and_can_insert_a_new_one() {
        let bytes = jpeg_with_unknown_segment_and_comment();
        let info = FileInfo::new("rewrite.jpg".into(), bytes.len() as u64, FileFormat::Jpeg);
        let deleted = rewrite_jpeg_to_vec(
            &bytes,
            info.clone(),
            ParseLimits::default(),
            &[JpegEdit::DeleteComments],
        )
        .unwrap();
        let deleted_metadata = read_jpeg(
            &mut Cursor::new(deleted),
            FileInfo::new("rewrite.jpg".into(), 0, FileFormat::Jpeg),
            ParseLimits::default(),
        )
        .unwrap();
        assert!(deleted_metadata.find("JPEG:Comment").is_none());

        let inserted = rewrite_jpeg_to_vec(
            &[0xFF, 0xD8, 0xFF, 0xD9],
            FileInfo::new("empty.jpg".into(), 4, FileFormat::Jpeg),
            ParseLimits::default(),
            &[JpegEdit::SetComment("inserted".to_owned())],
        )
        .unwrap();
        assert!(inserted.windows(2).any(|window| window == [0xFF, 0xFE]));
    }

    #[test]
    fn path_rewrite_validates_before_atomic_replace() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("metra-jpeg-rewrite-{unique}"));
        fs::create_dir(&directory).unwrap();
        let path = directory.join("atomic.jpg");
        fs::write(&path, jpeg_with_comment()).unwrap();

        rewrite_jpeg_path(
            &path,
            ParseLimits::default(),
            &[JpegEdit::SetComment("atomic".to_owned())],
        )
        .unwrap();
        let rewritten = fs::read(&path).unwrap();
        let metadata = read_jpeg(
            &mut Cursor::new(rewritten),
            FileInfo::new("atomic.jpg".into(), 0, FileFormat::Jpeg),
            ParseLimits::default(),
        )
        .unwrap();
        assert_eq!(
            metadata.find("JPEG:Comment").unwrap().display_value(),
            "atomic"
        );
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn rejects_oversized_comment_before_writing() {
        let info = FileInfo::new("rewrite.jpg".into(), 4, FileFormat::Jpeg);
        let error = rewrite_jpeg_to_vec(
            &[0xFF, 0xD8, 0xFF, 0xD9],
            info,
            ParseLimits::default(),
            &[JpegEdit::SetComment("x".repeat(65_534))],
        )
        .unwrap_err();
        assert!(error.to_string().contains("65533-byte"));
    }
}
