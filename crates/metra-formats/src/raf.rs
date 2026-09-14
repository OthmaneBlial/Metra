use std::io::{Read, Seek, SeekFrom};

use metra_core::{
    FileInfo, Metadata, MetraError, ParseLimits, Result, Source, Tag, TagValue, ValueType, Warning,
};

const RAF_SIGNATURE: &[u8; 16] = b"FUJIFILMCCD-RAW ";
const RAF_HEADER_LENGTH: usize = 0x70;
const RAF_MAX_DIRECTORY_ENTRIES: usize = 256;

/// Read the bounded metadata structures exposed by a FujiFilm RAF container.
///
/// RAF pixel payloads remain untouched. The fixed header and proprietary RAF
/// directory are decoded when their declared ranges are valid; embedded Fuji
/// TIFF metadata is delegated to the shared TIFF reader.
pub fn read_raf<R: Read + Seek>(
    reader: &mut R,
    file_info: FileInfo,
    limits: ParseLimits,
) -> Result<Metadata> {
    let file_length = file_info.size;
    let mut metadata = Metadata::new(file_info.clone());
    crate::raw::add_identity(&mut metadata, "RAF");
    if file_length < RAF_HEADER_LENGTH as u64 {
        metadata.add_warning(
            Warning::new(
                "raw-raf-partial",
                "RAF header is truncated; only container identity is available",
            )
            .at(file_length),
        );
        return Ok(metadata);
    }
    let header = read_at(
        reader,
        0,
        RAF_HEADER_LENGTH,
        file_length,
        &file_info,
        "RAF header",
    )?;
    if !header.starts_with(RAF_SIGNATURE) {
        return Err(MetraError::InvalidHeader {
            context: "RAF".to_owned(),
            message: "invalid FujiFilm RAF signature".to_owned(),
        });
    }

    if let Some(firmware) = string_field(&header[0x3c..0x40]) {
        add_tag(
            &mut metadata,
            "FirmwareVersion",
            None,
            TagValue::String(firmware),
            Source::new("RAF/header", Some(0x3c), Some(4)),
        );
    }
    add_header_u32(&mut metadata, "PreviewOffset", &header, 0x54);
    add_header_u32(&mut metadata, "PreviewLength", &header, 0x58);
    add_header_u32(&mut metadata, "DirectoryOffset", &header, 0x5c);
    add_header_u32(&mut metadata, "DirectoryLength", &header, 0x60);
    add_header_u32(&mut metadata, "FujiIfdOffset", &header, 0x64);
    add_header_u32(&mut metadata, "FujiIfdLength", &header, 0x68);
    if header[0x6c..0x70].starts_with(&[0, 0, 0]) {
        add_tag(
            &mut metadata,
            "RAFCompression",
            None,
            TagValue::Unsigned(u64::from(read_u32_be(&header[0x6c..0x70]))),
            Source::new("RAF/header", Some(0x6c), Some(4)),
        );
    }

    let directory_offset = read_u32_be(&header[0x5c..0x60]) as u64;
    let directory_length = read_u32_be(&header[0x60..0x64]) as u64;
    if let Some((offset, length)) = checked_region(
        directory_offset,
        directory_length,
        file_length,
        limits,
        &mut metadata,
        "RAF directory",
    ) {
        let directory = read_at(
            reader,
            offset,
            length,
            file_length,
            &file_info,
            "RAF directory",
        )?;
        parse_directory(&directory, offset, &mut metadata, limits);
    }

    let fuji_ifd_offset = read_u32_be(&header[0x64..0x68]) as u64;
    let fuji_ifd_length = read_u32_be(&header[0x68..0x6c]) as u64;
    if let Some((offset, length)) = checked_region(
        fuji_ifd_offset,
        fuji_ifd_length,
        file_length,
        limits,
        &mut metadata,
        "RAF Fuji IFD",
    ) && let Err(error) = crate::tiff::parse_tiff_from_reader(
        reader,
        offset,
        length as u64,
        offset,
        &mut metadata,
        limits,
    ) {
        metadata.add_warning(
            Warning::new(
                "raw-raf-fuji-ifd",
                format!("Fuji RAF TIFF directory was not decoded: {error}"),
            )
            .at(offset),
        );
    }
    metadata.sort_tags();
    Ok(metadata)
}

fn parse_directory(
    bytes: &[u8],
    absolute_offset: u64,
    metadata: &mut Metadata,
    limits: ParseLimits,
) {
    if bytes.len() < 4 {
        metadata.add_warning(
            Warning::new(
                "raw-raf-directory",
                "RAF directory is shorter than its entry count",
            )
            .at(absolute_offset),
        );
        return;
    }
    let declared_entries = read_u32_be(&bytes[..4]) as usize;
    let entry_count = declared_entries
        .min(RAF_MAX_DIRECTORY_ENTRIES)
        .min(limits.max_ifd_entries);
    if declared_entries > entry_count {
        metadata.add_warning(
            Warning::new(
                "raw-raf-directory-limit",
                format!(
                    "RAF directory declares {declared_entries} entries; reading only {entry_count}"
                ),
            )
            .at(absolute_offset),
        );
    }
    let mut cursor = 4_usize;
    for _ in 0..entry_count {
        let Some(end) = cursor.checked_add(4) else {
            break;
        };
        if end > bytes.len() {
            metadata.add_warning(
                Warning::new(
                    "raw-raf-directory",
                    "RAF directory entry header is truncated",
                )
                .at(absolute_offset.saturating_add(cursor as u64)),
            );
            break;
        }
        let tag_id = read_u16_be(&bytes[cursor..cursor + 2]);
        let value_len = usize::from(read_u16_be(&bytes[cursor + 2..cursor + 4]));
        let value_start = end;
        let Some(value_end) = value_start.checked_add(value_len) else {
            metadata.add_warning(
                Warning::new("raw-raf-directory", "RAF directory value length overflow")
                    .at(absolute_offset.saturating_add(cursor as u64)),
            );
            break;
        };
        if value_end > bytes.len() {
            metadata.add_warning(
                Warning::new("raw-raf-directory", "RAF directory value is truncated")
                    .at(absolute_offset.saturating_add(value_start as u64)),
            );
            break;
        }
        let value = &bytes[value_start..value_end];
        let source = Source::new(
            "RAF/directory",
            Some(absolute_offset.saturating_add(cursor as u64)),
            Some((4 + value_len) as u64),
        );
        let raw_value = value.to_vec();
        let (name, value, value_type) = decode_directory_value(tag_id, value);
        add_tag(metadata, name, Some(u32::from(tag_id)), value, source);
        if let Some(tag) = metadata.tags.last_mut() {
            tag.value_type = value_type;
            tag.raw_value = Some(raw_value);
        }
        cursor = value_end;
        if cursor > limits.max_metadata_bytes {
            metadata.add_warning(
                Warning::new(
                    "raw-raf-directory-limit",
                    "RAF directory scan reached the metadata budget",
                )
                .at(absolute_offset.saturating_add(cursor as u64)),
            );
            break;
        }
    }
}

fn decode_directory_value(tag_id: u16, bytes: &[u8]) -> (&'static str, TagValue, ValueType) {
    let name = raf_tag_name(tag_id);
    match tag_id {
        0x0100 | 0x0110 | 0x0111 | 0x0115 | 0x0118 | 0x0119 => {
            if bytes.len().is_multiple_of(2) {
                let values = bytes
                    .chunks_exact(2)
                    .map(|chunk| TagValue::Unsigned(u64::from(read_u16_be(chunk))))
                    .collect();
                (name, TagValue::Array(values), ValueType::Array)
            } else {
                (name, TagValue::Bytes(bytes.to_vec()), ValueType::Bytes)
            }
        }
        0x0117 if bytes.len() >= 4 => (
            name,
            TagValue::Unsigned(u64::from(read_u32_be(&bytes[..4]))),
            ValueType::UnsignedInteger,
        ),
        0x0130 if let Some(value) = bytes.first() => (
            name,
            TagValue::Unsigned(u64::from(*value)),
            ValueType::UnsignedInteger,
        ),
        _ if bytes.len() == 4 => (
            name,
            TagValue::Unsigned(u64::from(read_u32_be(bytes))),
            ValueType::UnsignedInteger,
        ),
        _ => (name, TagValue::Bytes(bytes.to_vec()), ValueType::Bytes),
    }
}

fn raf_tag_name(tag_id: u16) -> &'static str {
    match tag_id {
        0x0100 => "RawImageFullSize",
        0x0110 => "RawImageCropTopLeft",
        0x0111 => "RawImageCroppedSize",
        0x0115 => "RawImageAspectRatio",
        0x0117 => "RawZoomActive",
        0x0118 => "RawZoomTopLeft",
        0x0119 => "RawZoomSize",
        0x0121 => "RawImageSize",
        0x0130 => "FujiLayout",
        0x0131 => "XTransLayout",
        0x2000 => "WB_GRGBLevelsAuto",
        0x2100 => "WB_GRGBLevelsDaylight",
        0x2200 => "WB_GRGBLevelsCloudy",
        0x2ff0 => "WB_GRGBLevels",
        0x9200 => "RelativeExposure",
        0x9650 => "RawExposureBias",
        0xc000 => "RAFData",
        _ => "Unknown",
    }
}

fn add_header_u32(metadata: &mut Metadata, name: &'static str, header: &[u8], offset: usize) {
    add_tag(
        metadata,
        name,
        None,
        TagValue::Unsigned(u64::from(read_u32_be(&header[offset..offset + 4]))),
        Source::new("RAF/header", Some(offset as u64), Some(4)),
    );
}

fn add_tag(
    metadata: &mut Metadata,
    name: &'static str,
    id: Option<u32>,
    value: TagValue,
    source: Source,
) {
    let value_type = match &value {
        TagValue::String(_) => ValueType::String,
        TagValue::Unsigned(_) => ValueType::UnsignedInteger,
        TagValue::Bytes(_) => ValueType::Bytes,
        TagValue::Array(_) => ValueType::Array,
        _ => ValueType::Unknown,
    };
    metadata.add_tag(Tag {
        namespace: "RAF".to_owned(),
        group: "RAF".to_owned(),
        id,
        name: name.to_owned(),
        description: Some("FujiFilm RAF metadata".to_owned()),
        raw_value: None,
        value,
        value_type,
        source,
        writable: false,
    });
}

fn string_field(bytes: &[u8]) -> Option<String> {
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    if end == 0 {
        return None;
    }
    Some(String::from_utf8_lossy(&bytes[..end]).trim().to_owned())
}

fn checked_region(
    offset: u64,
    length: u64,
    file_length: u64,
    limits: ParseLimits,
    metadata: &mut Metadata,
    context: &str,
) -> Option<(u64, usize)> {
    if offset == 0 || length == 0 {
        return None;
    }
    let end = offset.checked_add(length);
    let Some(end) = end else {
        metadata.add_warning(
            Warning::new("raw-raf-range", format!("{context} range overflows")).at(offset),
        );
        return None;
    };
    if end > file_length {
        metadata.add_warning(
            Warning::new(
                "raw-raf-range",
                format!("{context} range is outside the file"),
            )
            .at(offset),
        );
        return None;
    }
    let Ok(length) = usize::try_from(length) else {
        metadata.add_warning(
            Warning::new(
                "raw-raf-range",
                format!("{context} length is not addressable"),
            )
            .at(offset),
        );
        return None;
    };
    if length > limits.max_metadata_bytes {
        metadata.add_warning(
            Warning::new(
                "raw-raf-range-limit",
                format!("{context} length {length} exceeds the metadata budget"),
            )
            .at(offset),
        );
        return None;
    }
    Some((offset, length))
}

fn read_at<R: Read + Seek>(
    reader: &mut R,
    offset: u64,
    length: usize,
    file_length: u64,
    file_info: &FileInfo,
    context: &str,
) -> Result<Vec<u8>> {
    let end = offset
        .checked_add(
            u64::try_from(length).map_err(|_| MetraError::InvalidOffset {
                context: context.to_owned(),
                offset,
            })?,
        )
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
            path: file_info.path.clone(),
            source,
        })?;
    let mut bytes = vec![0_u8; length];
    reader
        .read_exact(&mut bytes)
        .map_err(|source| MetraError::Io {
            path: file_info.path.clone(),
            source,
        })?;
    Ok(bytes)
}

fn read_u16_be(bytes: &[u8]) -> u16 {
    u16::from_be_bytes([bytes[0], bytes[1]])
}

fn read_u32_be(bytes: &[u8]) -> u32 {
    u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    fn raf_fixture() -> Vec<u8> {
        let directory_offset = 0x120_u32;
        let mut bytes = vec![0_u8; directory_offset as usize + 4 + 4 + 4 + 4 + 2 + 4];
        bytes[..RAF_SIGNATURE.len()].copy_from_slice(RAF_SIGNATURE);
        bytes[0x3c..0x40].copy_from_slice(b"0201");
        bytes[0x5c..0x60].copy_from_slice(&directory_offset.to_be_bytes());
        bytes[0x60..0x64].copy_from_slice(&20_u32.to_be_bytes());
        bytes[0x64..0x68].copy_from_slice(&0_u32.to_be_bytes());
        bytes[directory_offset as usize..directory_offset as usize + 4]
            .copy_from_slice(&2_u32.to_be_bytes());
        let mut cursor = directory_offset as usize + 4;
        bytes[cursor..cursor + 2].copy_from_slice(&0x0100_u16.to_be_bytes());
        bytes[cursor + 2..cursor + 4].copy_from_slice(&4_u16.to_be_bytes());
        bytes[cursor + 4..cursor + 6].copy_from_slice(&4000_u16.to_be_bytes());
        bytes[cursor + 6..cursor + 8].copy_from_slice(&3000_u16.to_be_bytes());
        cursor += 8;
        bytes[cursor..cursor + 2].copy_from_slice(&0x0117_u16.to_be_bytes());
        bytes[cursor + 2..cursor + 4].copy_from_slice(&4_u16.to_be_bytes());
        bytes[cursor + 4..cursor + 8].copy_from_slice(&1_u32.to_be_bytes());
        bytes
    }

    #[test]
    fn reads_raf_header_and_directory_without_touching_pixels() {
        let bytes = raf_fixture();
        let metadata = read_raf(
            &mut Cursor::new(bytes.clone()),
            FileInfo::new(
                "capture.raf".into(),
                bytes.len() as u64,
                metra_core::FileFormat::Raw,
            ),
            ParseLimits::default(),
        )
        .unwrap();
        assert_eq!(metadata.find("RAW:Variant").unwrap().display_value(), "RAF");
        assert_eq!(
            metadata
                .find("RAF:FirmwareVersion")
                .unwrap()
                .display_value(),
            "0201"
        );
        assert_eq!(
            metadata
                .find("RAF:RawImageFullSize")
                .unwrap()
                .display_value(),
            "4000, 3000"
        );
        assert_eq!(
            metadata
                .find("RAF:RawImageFullSize")
                .unwrap()
                .raw_value
                .as_deref(),
            Some(&[0x0f, 0xa0, 0x0b, 0xb8][..])
        );
        assert_eq!(
            metadata.find("RAF:RawZoomActive").unwrap().display_value(),
            "1"
        );
        assert!(metadata.find("RAF:PreviewOffset").is_some());
    }

    #[test]
    fn retains_partial_warning_for_truncated_raf_header() {
        let bytes = RAF_SIGNATURE.to_vec();
        let metadata = read_raf(
            &mut Cursor::new(bytes.clone()),
            FileInfo::new(
                "capture.raf".into(),
                bytes.len() as u64,
                metra_core::FileFormat::Raw,
            ),
            ParseLimits::default(),
        )
        .unwrap();
        assert!(
            metadata
                .warnings
                .iter()
                .any(|warning| warning.code == "raw-raf-partial")
        );
    }
}
