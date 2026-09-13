use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use metra_core::{
    FileInfo, Metadata, MetraError, ParseLimits, Result, Source, Tag, TagValue, ValueType, Warning,
};

use crate::tiff::parse_tiff_from_reader;

const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1A\n";

pub fn read_png<R: Read + Seek>(
    reader: &mut R,
    file_info: FileInfo,
    limits: ParseLimits,
) -> Result<Metadata> {
    let path = file_info.path.clone();
    let file_length = file_info.size;
    let mut metadata = Metadata::new(file_info);
    let mut signature = [0_u8; 8];
    read_exact(reader, &mut signature, &path)?;
    if &signature != PNG_SIGNATURE {
        return Err(MetraError::InvalidHeader {
            context: "PNG".to_owned(),
            message: "missing PNG signature".to_owned(),
        });
    }

    let mut offset = 8_u64;
    let mut chunks = 0_usize;
    let mut consumed_metadata = 0_usize;
    loop {
        if chunks >= limits.max_jpeg_segments {
            metadata.add_warning(
                Warning::new(
                    "png-chunk-limit",
                    format!("stopped after {} PNG chunks", limits.max_jpeg_segments),
                )
                .at(offset),
            );
            break;
        }
        let mut header = [0_u8; 8];
        match reader.read_exact(&mut header) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => {
                metadata.add_warning(
                    Warning::new("truncated-png", "PNG ended before an IEND chunk").at(offset),
                );
                break;
            }
            Err(error) => return Err(io_error(&path, error)),
        }
        offset = checked_add(offset, 8, "PNG chunk header")?;
        let data_length = u64::from(u32::from_be_bytes(header[..4].try_into().expect("length")));
        let chunk_type: [u8; 4] = header[4..8].try_into().expect("chunk type");
        let chunk_name = String::from_utf8_lossy(&chunk_type).into_owned();
        let chunk_end = data_length
            .checked_add(4)
            .and_then(|value| offset.checked_add(value))
            .ok_or(MetraError::InvalidOffset {
                context: format!("PNG {chunk_name} chunk end"),
                offset,
            })?;
        if chunk_end > file_length {
            return Err(MetraError::UnexpectedEof {
                context: format!("PNG {chunk_name} chunk"),
            });
        }

        let remaining = limits.max_metadata_bytes.saturating_sub(consumed_metadata);
        let data = if data_length <= remaining as u64 {
            let data_length_usize =
                usize::try_from(data_length).map_err(|_| MetraError::ResourceLimitExceeded {
                    resource: format!("PNG {chunk_name} chunk"),
                    limit: limits.max_metadata_bytes,
                })?;
            let mut data = vec![0_u8; data_length_usize];
            read_exact(reader, &mut data, &path)?;
            consumed_metadata = consumed_metadata.saturating_add(data_length_usize);
            Some(data)
        } else {
            metadata.add_warning(
                Warning::new(
                    "png-metadata-limit",
                    format!("skipped {data_length}-byte {chunk_name} chunk"),
                )
                .at(offset),
            );
            seek_forward(reader, data_length, &path)?;
            consumed_metadata = limits.max_metadata_bytes;
            None
        };
        offset = checked_add(offset, data_length, "PNG chunk data")?;

        let mut crc_bytes = [0_u8; 4];
        read_exact(reader, &mut crc_bytes, &path)?;
        let expected_crc = u32::from_be_bytes(crc_bytes);
        if let Some(data) = data.as_deref() {
            let actual_crc = crc32(&chunk_type, data);
            if actual_crc != expected_crc {
                metadata.add_warning(
                    Warning::new(
                        "png-crc",
                        format!(
                            "CRC mismatch for {chunk_name}: expected {expected_crc:08X}, calculated {actual_crc:08X}"
                        ),
                    )
                    .at(offset),
                );
            }
            process_chunk(
                &chunk_type,
                data,
                offset.saturating_sub(data_length),
                &mut metadata,
                limits,
            )?;
        }
        offset = checked_add(offset, 4, "PNG chunk CRC")?;
        chunks += 1;
        if &chunk_type == b"IEND" {
            break;
        }
    }
    metadata.sort_tags();
    Ok(metadata)
}

fn process_chunk(
    chunk_type: &[u8; 4],
    data: &[u8],
    data_offset: u64,
    metadata: &mut Metadata,
    limits: ParseLimits,
) -> Result<()> {
    match chunk_type {
        b"tEXt" => parse_text_chunk(data, data_offset, metadata),
        b"zTXt" => metadata.add_warning(
            Warning::new(
                "unsupported-ztxt",
                "PNG contains compressed tEXt metadata; decompression is planned",
            )
            .at(data_offset),
        ),
        b"iTXt" => parse_itxt_chunk(data, data_offset, metadata),
        b"eXIf" => {
            if data.len() < 8 {
                metadata.add_warning(
                    Warning::new(
                        "truncated-exif",
                        "PNG eXIf chunk is shorter than a TIFF header",
                    )
                    .at(data_offset),
                );
            } else {
                let mut cursor = std::io::Cursor::new(data);
                parse_tiff_from_reader(
                    &mut cursor,
                    0,
                    data.len() as u64,
                    data_offset,
                    metadata,
                    limits,
                )?;
            }
        }
        b"iCCP" => metadata.add_warning(
            Warning::new(
                "unsupported-icc",
                "PNG contains an ICC profile; ICC parsing is planned",
            )
            .at(data_offset),
        ),
        b"tIME" => parse_time_chunk(data, data_offset, metadata),
        b"pHYs" => parse_phys_chunk(data, data_offset, metadata),
        _ => {}
    }
    Ok(())
}

fn parse_text_chunk(data: &[u8], data_offset: u64, metadata: &mut Metadata) {
    let Some(separator) = data.iter().position(|byte| *byte == 0) else {
        metadata.add_warning(
            Warning::new("invalid-text", "PNG tEXt chunk has no keyword separator").at(data_offset),
        );
        return;
    };
    let keyword = String::from_utf8_lossy(&data[..separator]);
    let value = String::from_utf8_lossy(&data[separator + 1..]).into_owned();
    add_text_tag(metadata, &keyword, value, data, data_offset, "tEXt");
}

fn parse_itxt_chunk(data: &[u8], data_offset: u64, metadata: &mut Metadata) {
    let mut fields = data.splitn(6, |byte| *byte == 0);
    let Some(keyword) = fields.next() else {
        return;
    };
    let compression_flag = fields
        .next()
        .and_then(|field| field.first().copied())
        .unwrap_or(0);
    let _compression_method = fields.next();
    let _language = fields.next();
    let _translated_keyword = fields.next();
    let Some(text) = fields.next() else {
        metadata.add_warning(
            Warning::new("invalid-itxt", "PNG iTXt chunk is missing text").at(data_offset),
        );
        return;
    };
    if compression_flag != 0 {
        metadata.add_warning(
            Warning::new(
                "unsupported-itxt-compression",
                "PNG iTXt text is compressed; decompression is planned",
            )
            .at(data_offset),
        );
        return;
    }
    let keyword = String::from_utf8_lossy(keyword);
    let value = String::from_utf8_lossy(text).into_owned();
    add_text_tag(metadata, &keyword, value, data, data_offset, "iTXt");
}

fn add_text_tag(
    metadata: &mut Metadata,
    keyword: &str,
    value: String,
    raw_value: &[u8],
    data_offset: u64,
    container: &str,
) {
    metadata.add_tag(Tag {
        namespace: "PNG".to_owned(),
        group: container.to_owned(),
        id: None,
        name: format!("Text:{keyword}"),
        description: Some("PNG textual metadata".to_owned()),
        raw_value: Some(raw_value.to_vec()),
        value: TagValue::String(value),
        value_type: ValueType::String,
        source: Source::new(
            format!("PNG/{container}"),
            Some(data_offset),
            Some(raw_value.len() as u64),
        ),
        writable: false,
    });
}

fn parse_time_chunk(data: &[u8], data_offset: u64, metadata: &mut Metadata) {
    if data.len() != 7 {
        metadata.add_warning(
            Warning::new("invalid-time", "PNG tIME chunk must contain 7 bytes").at(data_offset),
        );
        return;
    }
    let year = u16::from_be_bytes([data[0], data[1]]);
    let value = format!(
        "{year:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        data[2], data[3], data[4], data[5], data[6]
    );
    metadata.add_tag(Tag {
        namespace: "PNG".to_owned(),
        group: "tIME".to_owned(),
        id: None,
        name: "ModificationTime".to_owned(),
        description: Some("Last modification time reported by the PNG".to_owned()),
        raw_value: Some(data.to_vec()),
        value: TagValue::String(value),
        value_type: ValueType::String,
        source: Source::new("PNG/tIME", Some(data_offset), Some(7)),
        writable: false,
    });
}

fn parse_phys_chunk(data: &[u8], data_offset: u64, metadata: &mut Metadata) {
    if data.len() != 9 {
        metadata.add_warning(
            Warning::new("invalid-phys", "PNG pHYs chunk must contain 9 bytes").at(data_offset),
        );
        return;
    }
    let x = u32::from_be_bytes(data[..4].try_into().expect("x pixels per unit"));
    let y = u32::from_be_bytes(data[4..8].try_into().expect("y pixels per unit"));
    let unit = match data[8] {
        1 => "meter",
        0 => "unknown",
        other => {
            metadata.add_warning(
                Warning::new(
                    "invalid-phys-unit",
                    format!("unknown PNG pHYs unit {other}"),
                )
                .at(data_offset + 8),
            );
            "unknown"
        }
    };
    for (name, value, start) in [("PixelsPerUnitX", x, 0_u64), ("PixelsPerUnitY", y, 4_u64)] {
        metadata.add_tag(Tag {
            namespace: "PNG".to_owned(),
            group: "pHYs".to_owned(),
            id: None,
            name: name.to_owned(),
            description: Some("Pixels per physical unit".to_owned()),
            raw_value: None,
            value: TagValue::Unsigned(u64::from(value)),
            value_type: ValueType::UnsignedInteger,
            source: Source::new("PNG/pHYs", Some(data_offset + start), Some(4)),
            writable: false,
        });
    }
    metadata.add_tag(Tag {
        namespace: "PNG".to_owned(),
        group: "pHYs".to_owned(),
        id: None,
        name: "Unit".to_owned(),
        description: Some("pHYs unit".to_owned()),
        raw_value: Some(vec![data[8]]),
        value: TagValue::String(unit.to_owned()),
        value_type: ValueType::String,
        source: Source::new("PNG/pHYs", Some(data_offset + 8), Some(1)),
        writable: false,
    });
}

fn crc32(chunk_type: &[u8; 4], data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFF_u32;
    for byte in chunk_type.iter().chain(data.iter()) {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = 0_u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

fn checked_add(left: u64, right: u64, context: &str) -> Result<u64> {
    left.checked_add(right).ok_or(MetraError::InvalidOffset {
        context: context.to_owned(),
        offset: left,
    })
}

fn seek_forward<R: Seek>(reader: &mut R, length: u64, path: &Path) -> Result<()> {
    let distance = i64::try_from(length).map_err(|_| MetraError::InvalidOffset {
        context: "PNG chunk skip".to_owned(),
        offset: length,
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
    use metra_core::{FileFormat, FileInfo};

    fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&(data.len() as u32).to_be_bytes());
        bytes.extend_from_slice(kind);
        bytes.extend_from_slice(data);
        bytes.extend_from_slice(&crc32(kind, data).to_be_bytes());
        bytes
    }

    #[test]
    fn reads_text_time_and_physical_resolution_chunks() {
        let mut bytes = PNG_SIGNATURE.to_vec();
        bytes.extend_from_slice(&chunk(b"tEXt", b"Comment\0hello"));
        bytes.extend_from_slice(&chunk(b"tIME", &[0x07, 0xEA, 9, 13, 12, 34, 56]));
        bytes.extend_from_slice(&chunk(b"pHYs", &[0, 0, 0, 96, 0, 0, 0, 96, 1]));
        bytes.extend_from_slice(&chunk(b"IEND", &[]));
        let info = FileInfo::new("test.png".into(), bytes.len() as u64, FileFormat::Png);
        let metadata = read_png(&mut Cursor::new(bytes), info, ParseLimits::default()).unwrap();
        assert_eq!(
            metadata.find("PNG:Text:Comment").unwrap().display_value(),
            "hello"
        );
        assert_eq!(
            metadata
                .find("PNG:ModificationTime")
                .unwrap()
                .display_value(),
            "2026-09-13 12:34:56"
        );
        assert_eq!(
            metadata.find("PNG:PixelsPerUnitX").unwrap().display_value(),
            "96"
        );
    }

    #[test]
    fn reports_bad_crc_as_warning() {
        let mut bytes = PNG_SIGNATURE.to_vec();
        let mut bad = chunk(b"tEXt", b"x\0y");
        let last = bad.len() - 1;
        bad[last] ^= 0xFF;
        bytes.extend_from_slice(&bad);
        bytes.extend_from_slice(&chunk(b"IEND", &[]));
        let info = FileInfo::new("bad.png".into(), bytes.len() as u64, FileFormat::Png);
        let metadata = read_png(&mut Cursor::new(bytes), info, ParseLimits::default()).unwrap();
        assert!(
            metadata
                .warnings
                .iter()
                .any(|warning| warning.code == "png-crc")
        );
    }
}
