use std::io::{Read, Seek, SeekFrom};

use metra_core::{
    FileInfo, Metadata, MetraError, ParseLimits, Result, Source, Tag, TagValue, ValueType, Warning,
};

const MRW_BIG_ENDIAN: &[u8; 4] = b"\0MRM";
const MRW_LITTLE_ENDIAN: &[u8; 4] = b"\0MRI";
const MRW_HEADER_LENGTH: usize = 8;

#[derive(Clone, Copy)]
enum Endian {
    Big,
    Little,
}

/// Read bounded Minolta MRW metadata segments without touching raw image data.
pub fn read_mrw<R: Read + Seek>(
    reader: &mut R,
    file_info: FileInfo,
    limits: ParseLimits,
) -> Result<Metadata> {
    let mut metadata = Metadata::new(file_info.clone());
    crate::raw::add_identity(&mut metadata, "MRW");
    if file_info.size < MRW_HEADER_LENGTH as u64 {
        metadata.add_warning(
            Warning::new(
                "raw-mrw-partial",
                "MRW header is truncated; only container identity is available",
            )
            .at(file_info.size),
        );
        return Ok(metadata);
    }
    let header = read_at(
        reader,
        0,
        MRW_HEADER_LENGTH,
        file_info.size,
        &file_info,
        "MRW header",
    )?;
    let endian = if header.starts_with(MRW_BIG_ENDIAN) {
        Endian::Big
    } else if header.starts_with(MRW_LITTLE_ENDIAN) {
        Endian::Little
    } else {
        return Err(MetraError::InvalidHeader {
            context: "MRW".to_owned(),
            message: "invalid Minolta MRW signature".to_owned(),
        });
    };

    let data_length = u64::from(read_u32(endian, &header[4..8]));
    let metadata_end =
        (MRW_HEADER_LENGTH as u64)
            .checked_add(data_length)
            .ok_or(MetraError::InvalidOffset {
                context: "MRW metadata boundary".to_owned(),
                offset: data_length,
            })?;
    if metadata_end > file_info.size {
        metadata.add_warning(
            Warning::new(
                "raw-mrw-range",
                format!("MRW metadata boundary {metadata_end} exceeds file length"),
            )
            .at(4),
        );
        metadata.sort_tags();
        return Ok(metadata);
    }

    let mut cursor = MRW_HEADER_LENGTH as u64;
    let mut segment_count = 0_usize;
    while cursor < metadata_end {
        segment_count = segment_count.saturating_add(1);
        if segment_count > limits.max_ifd_entries {
            metadata.add_warning(
                Warning::new(
                    "raw-mrw-segment-limit",
                    "MRW segment count exceeded the configured entry budget",
                )
                .at(cursor),
            );
            break;
        }
        let header_end = cursor.checked_add(8).ok_or(MetraError::InvalidOffset {
            context: "MRW segment header".to_owned(),
            offset: cursor,
        })?;
        if header_end > metadata_end {
            metadata.add_warning(
                Warning::new("raw-mrw-partial", "MRW segment header is truncated").at(cursor),
            );
            break;
        }
        let segment_header = read_at(
            reader,
            cursor,
            8,
            file_info.size,
            &file_info,
            "MRW segment header",
        )?;
        let segment_length = u64::from(read_u32(endian, &segment_header[4..8]));
        let data_offset = header_end;
        let data_end =
            data_offset
                .checked_add(segment_length)
                .ok_or(MetraError::InvalidOffset {
                    context: "MRW segment".to_owned(),
                    offset: data_offset,
                })?;
        if data_end > metadata_end || data_end > file_info.size {
            metadata.add_warning(
                Warning::new(
                    "raw-mrw-range",
                    "MRW segment value exceeds its metadata region",
                )
                .at(data_offset),
            );
            break;
        }
        let segment_name = String::from_utf8_lossy(&segment_header[..4]).to_string();
        if segment_length > limits.max_metadata_bytes as u64 {
            metadata.add_warning(
                Warning::new(
                    "raw-mrw-range-limit",
                    format!("MRW segment {segment_name:?} exceeds the metadata budget"),
                )
                .at(data_offset),
            );
        } else {
            let length =
                usize::try_from(segment_length).map_err(|_| MetraError::InvalidOffset {
                    context: "MRW segment length".to_owned(),
                    offset: segment_length,
                })?;
            let data = read_at(
                reader,
                data_offset,
                length,
                file_info.size,
                &file_info,
                "MRW segment",
            )?;
            parse_segment(
                reader,
                endian,
                &segment_name,
                data_offset,
                &data,
                &mut metadata,
                limits,
            );
        }
        cursor = data_end;
    }
    if segment_count == 0 {
        metadata.add_warning(
            Warning::new("raw-mrw-partial", "MRW contains no metadata segments").at(cursor),
        );
    }
    metadata.sort_tags();
    Ok(metadata)
}

fn parse_segment<R: Read + Seek>(
    reader: &mut R,
    endian: Endian,
    name: &str,
    data_offset: u64,
    data: &[u8],
    metadata: &mut Metadata,
    limits: ParseLimits,
) {
    match name.as_bytes() {
        b"\0TTW" => {
            if let Err(error) = crate::tiff::parse_tiff_from_reader(
                reader,
                data_offset,
                data.len() as u64,
                data_offset,
                metadata,
                limits,
            ) {
                metadata.add_warning(
                    Warning::new(
                        "raw-mrw-tiff",
                        format!("embedded MRW TIFF metadata was not decoded: {error}"),
                    )
                    .at(data_offset),
                );
            }
        }
        b"\0PRD" => parse_prd(endian, data_offset, data, metadata),
        b"\0WBG" => parse_wbg(endian, data_offset, data, metadata),
        b"\0RIF" => parse_rif(data_offset, data, metadata),
        _ => add_tag(
            metadata,
            format!("Segment:{name}"),
            None,
            TagValue::Bytes(data.to_vec()),
            Some(data.to_vec()),
            Source::new("MRW/segment", Some(data_offset), Some(data.len() as u64)),
        ),
    }
}

fn parse_prd(endian: Endian, offset: u64, data: &[u8], metadata: &mut Metadata) {
    if data.len() < 24 {
        metadata.add_warning(
            Warning::new("raw-mrw-prd", "MRW PRD segment is shorter than 24 bytes").at(offset),
        );
        return;
    }
    if let Some(firmware) = string_field(&data[..8]) {
        add_tag(
            metadata,
            "FirmwareID".to_owned(),
            Some(0),
            TagValue::String(firmware),
            Some(data[..8].to_vec()),
            Source::new("MRW/PRD", Some(offset), Some(8)),
        );
    }
    for (name, position) in [
        ("SensorHeight", 8),
        ("SensorWidth", 10),
        ("ImageHeight", 12),
        ("ImageWidth", 14),
    ] {
        add_tag(
            metadata,
            name.to_owned(),
            Some(position as u32),
            TagValue::Unsigned(u64::from(read_u16(endian, &data[position..position + 2]))),
            Some(data[position..position + 2].to_vec()),
            Source::new(
                "MRW/PRD",
                Some(offset.saturating_add(position as u64)),
                Some(2),
            ),
        );
    }
    for (name, position) in [
        ("RawDepth", 16),
        ("BitDepth", 17),
        ("StorageMethod", 18),
        ("BayerPattern", 23),
    ] {
        add_tag(
            metadata,
            name.to_owned(),
            Some(position as u32),
            TagValue::Unsigned(u64::from(data[position])),
            Some(vec![data[position]]),
            Source::new(
                "MRW/PRD",
                Some(offset.saturating_add(position as u64)),
                Some(1),
            ),
        );
    }
}

fn parse_wbg(endian: Endian, offset: u64, data: &[u8], metadata: &mut Metadata) {
    if data.len() < 4 {
        metadata.add_warning(
            Warning::new(
                "raw-mrw-wbg",
                "MRW WBG segment is shorter than its scale field",
            )
            .at(offset),
        );
        return;
    }
    add_tag(
        metadata,
        "WBScale".to_owned(),
        Some(0),
        TagValue::Array(
            data[..4]
                .iter()
                .map(|value| TagValue::Unsigned(u64::from(*value)))
                .collect(),
        ),
        Some(data[..4].to_vec()),
        Source::new("MRW/WBG", Some(offset), Some(4)),
    );
    if data.len() >= 12 && data[4..].len().is_multiple_of(2) {
        let values = data[4..]
            .chunks_exact(2)
            .map(|value| TagValue::Unsigned(u64::from(read_u16(endian, value))))
            .collect();
        add_tag(
            metadata,
            "WB_RGGBLevels".to_owned(),
            Some(4),
            TagValue::Array(values),
            Some(data[4..].to_vec()),
            Source::new(
                "MRW/WBG",
                Some(offset.saturating_add(4)),
                Some((data.len() - 4) as u64),
            ),
        );
    }
}

fn parse_rif(offset: u64, data: &[u8], metadata: &mut Metadata) {
    for (name, position) in [
        ("Saturation", 1),
        ("Contrast", 2),
        ("Sharpness", 3),
        ("WBMode", 4),
        ("ProgramMode", 5),
        ("ISOSetting", 6),
    ] {
        if let Some(value) = data.get(position) {
            add_tag(
                metadata,
                name.to_owned(),
                Some(position as u32),
                TagValue::Signed(i64::from(i8::from_ne_bytes([*value]))),
                Some(vec![*value]),
                Source::new(
                    "MRW/RIF",
                    Some(offset.saturating_add(position as u64)),
                    Some(1),
                ),
            );
        }
    }
}

fn add_tag(
    metadata: &mut Metadata,
    name: String,
    id: Option<u32>,
    value: TagValue,
    raw_value: Option<Vec<u8>>,
    source: Source,
) {
    let value_type = match &value {
        TagValue::String(_) => ValueType::String,
        TagValue::Unsigned(_) => ValueType::UnsignedInteger,
        TagValue::Signed(_) => ValueType::SignedInteger,
        TagValue::Array(_) => ValueType::Array,
        TagValue::Bytes(_) => ValueType::Bytes,
        _ => ValueType::Unknown,
    };
    metadata.add_tag(Tag {
        namespace: "MRW".to_owned(),
        group: "MRW".to_owned(),
        id,
        name,
        description: Some("Minolta MRW metadata".to_owned()),
        raw_value,
        value,
        value_type,
        source,
        writable: false,
    });
}

fn string_field(bytes: &[u8]) -> Option<String> {
    let end = bytes
        .iter()
        .position(|value| *value == 0)
        .unwrap_or(bytes.len());
    if end == 0 {
        return None;
    }
    Some(String::from_utf8_lossy(&bytes[..end]).to_string())
}

fn read_u16(endian: Endian, bytes: &[u8]) -> u16 {
    match endian {
        Endian::Big => u16::from_be_bytes([bytes[0], bytes[1]]),
        Endian::Little => u16::from_le_bytes([bytes[0], bytes[1]]),
    }
}

fn read_u32(endian: Endian, bytes: &[u8]) -> u32 {
    match endian {
        Endian::Big => u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
        Endian::Little => u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
    }
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

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use metra_core::FileFormat;

    use super::*;

    fn mrw_fixture() -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(MRW_BIG_ENDIAN);
        bytes.extend_from_slice(&32_u32.to_be_bytes());
        bytes.extend_from_slice(b"\0PRD");
        bytes.extend_from_slice(&24_u32.to_be_bytes());
        let mut prd = [0_u8; 24];
        prd[..5].copy_from_slice(b"FW-1\0");
        prd[8..10].copy_from_slice(&3000_u16.to_be_bytes());
        prd[10..12].copy_from_slice(&4000_u16.to_be_bytes());
        prd[12..14].copy_from_slice(&2000_u16.to_be_bytes());
        prd[14..16].copy_from_slice(&3000_u16.to_be_bytes());
        prd[16] = 14;
        prd[17] = 14;
        prd[18] = 82;
        prd[23] = 1;
        bytes.extend_from_slice(&prd);
        bytes
    }

    #[test]
    fn reads_mrw_prd_without_touching_image_data() {
        let bytes = mrw_fixture();
        let metadata = read_mrw(
            &mut Cursor::new(bytes.clone()),
            FileInfo::new("capture.mrw".into(), bytes.len() as u64, FileFormat::Raw),
            ParseLimits::default(),
        )
        .unwrap();
        assert_eq!(metadata.find("RAW:Variant").unwrap().display_value(), "MRW");
        assert_eq!(
            metadata.find("MRW:FirmwareID").unwrap().display_value(),
            "FW-1"
        );
        assert_eq!(
            metadata.find("MRW:ImageWidth").unwrap().display_value(),
            "3000"
        );
        assert_eq!(metadata.find("MRW:RawDepth").unwrap().display_value(), "14");
        assert!(metadata.warnings.is_empty());
    }

    #[test]
    fn warns_for_mrw_without_metadata_segments() {
        let bytes = [b'\0', b'M', b'R', b'M', 0, 0, 0, 0];
        let metadata = read_mrw(
            &mut Cursor::new(bytes.as_slice()),
            FileInfo::new("capture.mrw".into(), bytes.len() as u64, FileFormat::Raw),
            ParseLimits::default(),
        )
        .unwrap();
        assert!(
            metadata
                .warnings
                .iter()
                .any(|warning| warning.code == "raw-mrw-partial")
        );
    }
}
