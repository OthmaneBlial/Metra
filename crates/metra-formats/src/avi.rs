use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use metra_core::{
    FileInfo, Metadata, MetraError, ParseLimits, Result, Source, Tag, TagValue, ValueType, Warning,
};

const RIFF_HEADER_LENGTH: usize = 12;
const AVIH_LENGTH: usize = 56;

pub fn read_avi<R: Read + Seek>(
    reader: &mut R,
    file_info: FileInfo,
    limits: ParseLimits,
) -> Result<Metadata> {
    let path = file_info.path.clone();
    let file_length = file_info.size;
    let mut metadata = Metadata::new(file_info);
    if file_length < RIFF_HEADER_LENGTH as u64 {
        return Err(MetraError::InvalidHeader {
            context: "AVI".to_owned(),
            message: "file is shorter than the RIFF/AVI header".to_owned(),
        });
    }
    let header = read_at(
        reader,
        0,
        RIFF_HEADER_LENGTH,
        file_length,
        &path,
        "AVI header",
    )?;
    if &header[..4] != b"RIFF" || &header[8..12] != b"AVI " {
        return Err(MetraError::InvalidHeader {
            context: "AVI".to_owned(),
            message: "expected RIFF/AVI signature".to_owned(),
        });
    }
    let riff_size = u64::from(u32::from_le_bytes(
        header[4..8].try_into().expect("RIFF size"),
    ));
    let declared_end = 8_u64
        .checked_add(riff_size)
        .ok_or(MetraError::InvalidOffset {
            context: "AVI RIFF size".to_owned(),
            offset: riff_size,
        })?;
    let scan_end = if declared_end > file_length {
        metadata.add_warning(
            Warning::new(
                "avi-riff-size",
                format!(
                    "RIFF declares an end at {declared_end}, beyond the file length {file_length}"
                ),
            )
            .at(4),
        );
        file_length
    } else {
        declared_end
    };
    if scan_end < RIFF_HEADER_LENGTH as u64 {
        return Err(MetraError::InvalidHeader {
            context: "AVI".to_owned(),
            message: "RIFF payload is shorter than the AVI form type".to_owned(),
        });
    }

    let mut materialized = 0_usize;
    let mut chunk_count = 0_usize;
    scan_region(
        reader,
        12,
        scan_end,
        0,
        false,
        &mut metadata,
        limits,
        &mut materialized,
        &mut chunk_count,
        &path,
        file_length,
    )?;
    metadata.sort_tags();
    Ok(metadata)
}

#[allow(clippy::too_many_arguments)]
fn scan_region<R: Read + Seek>(
    reader: &mut R,
    mut cursor: u64,
    end: u64,
    depth: usize,
    in_info: bool,
    metadata: &mut Metadata,
    limits: ParseLimits,
    materialized: &mut usize,
    chunk_count: &mut usize,
    path: &Path,
    file_length: u64,
) -> Result<()> {
    if depth > limits.max_recursion_depth {
        metadata.add_warning(
            Warning::new("avi-recursion-limit", "AVI list nesting limit reached").at(cursor),
        );
        return Ok(());
    }
    while cursor < end {
        if *chunk_count >= limits.max_jpeg_segments {
            metadata
                .add_warning(Warning::new("avi-chunk-limit", "AVI chunk limit reached").at(cursor));
            break;
        }
        if end.saturating_sub(cursor) < 8 {
            metadata.add_warning(
                Warning::new("truncated-avi-chunk", "AVI chunk header is truncated").at(cursor),
            );
            break;
        }
        let header = read_at(reader, cursor, 8, file_length, path, "AVI chunk header")?;
        let kind = &header[..4];
        let payload_length = u64::from(u32::from_le_bytes(
            header[4..8].try_into().expect("AVI chunk length"),
        ));
        let payload_start = cursor.checked_add(8).ok_or(MetraError::InvalidOffset {
            context: "AVI chunk payload".to_owned(),
            offset: cursor,
        })?;
        let payload_end =
            payload_start
                .checked_add(payload_length)
                .ok_or(MetraError::InvalidOffset {
                    context: "AVI chunk payload".to_owned(),
                    offset: payload_length,
                })?;
        if payload_end > end {
            metadata.add_warning(
                Warning::new(
                    "truncated-avi-chunk",
                    format!("{} chunk extends beyond its containing list", fourcc(kind)),
                )
                .at(payload_start),
            );
            break;
        }
        if kind == b"LIST" || kind == b"RIFF" {
            if payload_length < 4 {
                metadata.add_warning(
                    Warning::new(
                        "invalid-avi-list",
                        format!("{} list has no form type", fourcc(kind)),
                    )
                    .at(payload_start),
                );
            } else {
                let form = read_at(reader, payload_start, 4, file_length, path, "AVI list form")?;
                let child_info = in_info || &form == b"INFO";
                scan_region(
                    reader,
                    payload_start + 4,
                    payload_end,
                    depth + 1,
                    child_info,
                    metadata,
                    limits,
                    materialized,
                    chunk_count,
                    path,
                    file_length,
                )?;
            }
        } else if kind == b"avih" {
            if let Some(bytes) = read_payload(
                reader,
                payload_start,
                payload_length,
                metadata,
                limits,
                materialized,
                path,
                file_length,
                "AVI avih",
            )? {
                parse_avih(&bytes, payload_start, metadata);
            }
        } else if in_info
            && let Some(bytes) = read_payload(
                reader,
                payload_start,
                payload_length,
                metadata,
                limits,
                materialized,
                path,
                file_length,
                "AVI INFO value",
            )?
        {
            parse_info(kind, &bytes, payload_start, metadata);
        }
        let padded_end =
            payload_end
                .checked_add(payload_length % 2)
                .ok_or(MetraError::InvalidOffset {
                    context: "AVI chunk padding".to_owned(),
                    offset: payload_end,
                })?;
        if padded_end > end {
            metadata.add_warning(
                Warning::new("truncated-avi-chunk", "AVI chunk padding is truncated")
                    .at(payload_end),
            );
            break;
        }
        cursor = padded_end;
        *chunk_count += 1;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn read_payload<R: Read + Seek>(
    reader: &mut R,
    offset: u64,
    length: u64,
    metadata: &mut Metadata,
    limits: ParseLimits,
    materialized: &mut usize,
    path: &Path,
    file_length: u64,
    context: &str,
) -> Result<Option<Vec<u8>>> {
    if length > u64::try_from(limits.max_value_bytes).unwrap_or(u64::MAX) {
        metadata.add_warning(
            Warning::new(
                "avi-value-limit",
                format!("{context} contains {length} bytes; value omitted"),
            )
            .at(offset),
        );
        return Ok(None);
    }
    let length = usize::try_from(length).map_err(|_| MetraError::ResourceLimitExceeded {
        resource: context.to_owned(),
        limit: limits.max_value_bytes,
    })?;
    let total = materialized
        .checked_add(length)
        .ok_or(MetraError::ResourceLimitExceeded {
            resource: "AVI metadata values".to_owned(),
            limit: limits.max_metadata_bytes,
        })?;
    if total > limits.max_metadata_bytes {
        metadata.add_warning(
            Warning::new(
                "avi-metadata-limit",
                format!("{context} would exceed the metadata budget"),
            )
            .at(offset),
        );
        return Ok(None);
    }
    let bytes = read_at(reader, offset, length, file_length, path, context)?;
    *materialized = total;
    Ok(Some(bytes))
}

fn parse_avih(bytes: &[u8], offset: u64, metadata: &mut Metadata) {
    if bytes.len() < AVIH_LENGTH {
        metadata.add_warning(
            Warning::new(
                "truncated-avi-avih",
                "AVI main header is shorter than 56 bytes",
            )
            .at(offset),
        );
        return;
    }
    add_tag(
        metadata,
        "MicrosecondsPerFrame",
        TagValue::Unsigned(u64::from(read_u32(bytes, 0))),
        ValueType::UnsignedInteger,
        offset,
        4,
        "AVI/avih",
    );
    add_tag(
        metadata,
        "MaxBytesPerSecond",
        TagValue::Unsigned(u64::from(read_u32(bytes, 4))),
        ValueType::UnsignedInteger,
        offset + 4,
        4,
        "AVI/avih",
    );
    add_tag(
        metadata,
        "TotalFrames",
        TagValue::Unsigned(u64::from(read_u32(bytes, 16))),
        ValueType::UnsignedInteger,
        offset + 16,
        4,
        "AVI/avih",
    );
    add_tag(
        metadata,
        "Streams",
        TagValue::Unsigned(u64::from(read_u32(bytes, 24))),
        ValueType::UnsignedInteger,
        offset + 24,
        4,
        "AVI/avih",
    );
    add_tag(
        metadata,
        "ImageWidth",
        TagValue::Unsigned(u64::from(read_u32(bytes, 32))),
        ValueType::UnsignedInteger,
        offset + 32,
        4,
        "AVI/avih",
    );
    add_tag(
        metadata,
        "ImageHeight",
        TagValue::Unsigned(u64::from(read_u32(bytes, 36))),
        ValueType::UnsignedInteger,
        offset + 36,
        4,
        "AVI/avih",
    );
    let microseconds = read_u32(bytes, 0);
    if microseconds != 0 {
        add_tag(
            metadata,
            "FrameRate",
            TagValue::Float(1_000_000.0 / f64::from(microseconds)),
            ValueType::Float,
            offset,
            4,
            "AVI/derived",
        );
    }
}

fn parse_info(kind: &[u8], bytes: &[u8], offset: u64, metadata: &mut Metadata) {
    let name = match kind {
        b"INAM" => "Title",
        b"IART" => "Artist",
        b"ICMT" => "Comment",
        b"ICOP" => "Copyright",
        b"ISFT" => "Software",
        b"IGNR" => "Genre",
        b"IPRD" => "Product",
        b"IKEY" => "Keywords",
        b"IDIT" => "DateTime",
        _ => {
            return add_tag(
                metadata,
                &format!("Info:{}", fourcc(kind)),
                TagValue::Bytes(bytes.to_vec()),
                ValueType::Bytes,
                offset,
                bytes.len() as u64,
                "AVI/INFO",
            );
        }
    };
    add_tag(
        metadata,
        name,
        TagValue::String(
            String::from_utf8_lossy(bytes)
                .trim_end_matches('\0')
                .to_owned(),
        ),
        ValueType::String,
        offset,
        bytes.len() as u64,
        "AVI/INFO",
    );
}

fn add_tag(
    metadata: &mut Metadata,
    name: &str,
    value: TagValue,
    value_type: ValueType,
    offset: u64,
    length: u64,
    container: &str,
) {
    metadata.add_tag(Tag {
        namespace: "AVI".to_owned(),
        group: if container.ends_with("INFO") {
            "INFO".to_owned()
        } else {
            "avih".to_owned()
        },
        id: None,
        name: name.to_owned(),
        description: Some("AVI metadata property".to_owned()),
        raw_value: None,
        value,
        value_type,
        source: Source::new(container, Some(offset), Some(length)),
        writable: false,
    });
}

fn fourcc(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().expect("AVI u32"))
}

fn read_at<R: Read + Seek>(
    reader: &mut R,
    offset: u64,
    length: usize,
    file_length: u64,
    path: &Path,
    context: &str,
) -> Result<Vec<u8>> {
    let length_u64 = u64::try_from(length).map_err(|_| MetraError::InvalidOffset {
        context: context.to_owned(),
        offset,
    })?;
    let end = offset
        .checked_add(length_u64)
        .ok_or(MetraError::InvalidOffset {
            context: context.to_owned(),
            offset,
        })?;
    if end > file_length {
        return Err(MetraError::UnexpectedEof {
            context: context.to_owned(),
        });
    }
    reader
        .seek(SeekFrom::Start(offset))
        .map_err(|source| MetraError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    let mut bytes = vec![0_u8; length];
    reader
        .read_exact(&mut bytes)
        .map_err(|source| MetraError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut output = kind.to_vec();
        output.extend_from_slice(&(data.len() as u32).to_le_bytes());
        output.extend_from_slice(data);
        if data.len() % 2 == 1 {
            output.push(0);
        }
        output
    }

    fn list(form: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut payload = form.to_vec();
        payload.extend_from_slice(data);
        chunk(b"LIST", &payload)
    }

    fn minimal_avi() -> Vec<u8> {
        let mut avih = vec![0_u8; AVIH_LENGTH];
        avih[0..4].copy_from_slice(&40_000_u32.to_le_bytes());
        avih[16..20].copy_from_slice(&120_u32.to_le_bytes());
        avih[24..28].copy_from_slice(&2_u32.to_le_bytes());
        avih[32..36].copy_from_slice(&1_920_u32.to_le_bytes());
        avih[36..40].copy_from_slice(&1_080_u32.to_le_bytes());
        let hdrl = list(b"hdrl", &chunk(b"avih", &avih));
        let info = list(b"INFO", &chunk(b"INAM", b"Metra sample\0"));
        let mut body = hdrl;
        body.extend_from_slice(&info);
        let mut output = b"RIFF".to_vec();
        output.extend_from_slice(&((4 + body.len()) as u32).to_le_bytes());
        output.extend_from_slice(b"AVI ");
        output.extend_from_slice(&body);
        output
    }

    #[test]
    fn reads_avi_main_header_and_info_text() {
        let bytes = minimal_avi();
        let info = FileInfo::new(
            "sample.avi".into(),
            bytes.len() as u64,
            metra_core::FileFormat::Avi,
        );
        let metadata = read_avi(&mut Cursor::new(bytes), info, ParseLimits::default())
            .expect("AVI fixture should parse");

        assert_eq!(
            metadata.find("AVI:ImageWidth").unwrap().value,
            TagValue::Unsigned(1_920)
        );
        assert_eq!(
            metadata.find("AVI:ImageHeight").unwrap().value,
            TagValue::Unsigned(1_080)
        );
        assert_eq!(
            metadata.find("AVI:FrameRate").unwrap().display_value(),
            "25"
        );
        assert_eq!(
            metadata.find("AVI:Title").unwrap().value,
            TagValue::String("Metra sample".to_owned())
        );
    }

    #[test]
    fn warns_when_riff_declares_more_bytes_than_available() {
        let mut bytes = b"RIFF".to_vec();
        bytes.extend_from_slice(&100_u32.to_le_bytes());
        bytes.extend_from_slice(b"AVI ");
        let info = FileInfo::new(
            "short.avi".into(),
            bytes.len() as u64,
            metra_core::FileFormat::Avi,
        );
        let metadata = read_avi(&mut Cursor::new(bytes), info, ParseLimits::default())
            .expect("short AVI should remain inspectable");
        assert!(
            metadata
                .warnings()
                .iter()
                .any(|warning| warning.code == "avi-riff-size")
        );
    }
}
