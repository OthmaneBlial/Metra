use std::io::{Read, Seek, SeekFrom};

use metra_core::{
    FileInfo, Metadata, MetraError, ParseLimits, Result, Source, Tag, TagValue, ValueType, Warning,
};

const CRW_HEADER_LENGTH: usize = 26;
const CRW_DIRECTORY_ENTRY_LENGTH: usize = 10;
const CRW_MAX_DIRECTORY_ENTRIES: usize = 256;

#[derive(Clone, Copy)]
enum Endian {
    Big,
    Little,
}

/// Read bounded Canon CIFF/CRW metadata without loading image payloads.
pub fn read_crw<R: Read + Seek>(
    reader: &mut R,
    file_info: FileInfo,
    limits: ParseLimits,
) -> Result<Metadata> {
    let file_length = file_info.size;
    let mut metadata = Metadata::new(file_info.clone());
    crate::raw::add_identity(&mut metadata, "CRW");
    if file_length < CRW_HEADER_LENGTH as u64 {
        metadata.add_warning(
            Warning::new(
                "raw-crw-partial",
                "CRW header is truncated; only container identity is available",
            )
            .at(file_length),
        );
        return Ok(metadata);
    }
    let header = read_at(
        reader,
        0,
        CRW_HEADER_LENGTH,
        file_length,
        &file_info,
        "CRW header",
    )?;
    let endian = if &header[..2] == b"II" {
        Endian::Little
    } else if &header[..2] == b"MM" {
        Endian::Big
    } else {
        return Err(MetraError::InvalidHeader {
            context: "CRW".to_owned(),
            message: "invalid CIFF byte-order marker".to_owned(),
        });
    };
    if &header[6..14] != b"HEAPCCDR" {
        return Err(MetraError::InvalidHeader {
            context: "CRW".to_owned(),
            message: "invalid CIFF signature".to_owned(),
        });
    }

    let root_offset = u64::from(read_u32(endian, &header[2..6]));
    add_tag(
        &mut metadata,
        "HeaderVersion".to_owned(),
        None,
        TagValue::Bytes(header[14..18].to_vec()),
        header[14..18].to_vec(),
        Source::new("CRW/header", Some(14), Some(4)),
    );
    add_u32_tag(
        &mut metadata,
        "RootDirectoryOffset",
        root_offset,
        header[2..6].to_vec(),
        Source::new("CRW/header", Some(2), Some(4)),
    );
    if root_offset < CRW_HEADER_LENGTH as u64 || root_offset >= file_length {
        metadata.add_warning(
            Warning::new(
                "raw-crw-range",
                format!("CRW root directory offset {root_offset} is outside the file"),
            )
            .at(2),
        );
        metadata.sort_tags();
        return Ok(metadata);
    }

    parse_directory(
        reader,
        root_offset,
        file_length - root_offset,
        0,
        0,
        endian,
        &file_info,
        &mut metadata,
        limits,
    )?;
    metadata.sort_tags();
    Ok(metadata)
}

#[allow(clippy::too_many_arguments)]
fn parse_directory<R: Read + Seek>(
    reader: &mut R,
    base_offset: u64,
    region_length: u64,
    directory_offset: u64,
    depth: usize,
    endian: Endian,
    file_info: &FileInfo,
    metadata: &mut Metadata,
    limits: ParseLimits,
) -> Result<()> {
    if depth > limits.max_recursion_depth {
        metadata.add_warning(
            Warning::new("raw-crw-depth", "CRW directory recursion limit exceeded")
                .at(base_offset.saturating_add(directory_offset)),
        );
        return Ok(());
    }
    let footer_offset = region_length
        .checked_sub(4)
        .ok_or(MetraError::UnexpectedEof {
            context: "CRW directory pointer".to_owned(),
        })?;
    let footer = read_at(
        reader,
        base_offset + footer_offset,
        4,
        file_info.size,
        file_info,
        "CRW directory pointer",
    )?;
    let local_directory = u64::from(read_u32(endian, &footer));
    if local_directory > footer_offset.saturating_sub(2) {
        metadata.add_warning(
            Warning::new(
                "raw-crw-directory",
                "CRW directory pointer is outside its region",
            )
            .at(base_offset + footer_offset),
        );
        return Ok(());
    }
    let count_position =
        base_offset
            .checked_add(local_directory)
            .ok_or(MetraError::InvalidOffset {
                context: "CRW directory count".to_owned(),
                offset: local_directory,
            })?;
    let count_bytes = read_at(
        reader,
        count_position,
        2,
        file_info.size,
        file_info,
        "CRW directory count",
    )?;
    let declared_count = usize::from(read_u16(endian, &count_bytes));
    let entry_count = declared_count
        .min(CRW_MAX_DIRECTORY_ENTRIES)
        .min(limits.max_ifd_entries);
    if declared_count > entry_count {
        metadata.add_warning(
            Warning::new(
                "raw-crw-directory-limit",
                format!(
                    "CRW directory declares {declared_count} entries; reading only {entry_count}"
                ),
            )
            .at(count_position),
        );
    }
    let entries_local = local_directory + 2;
    for index in 0..entry_count {
        let entry_local = match entries_local.checked_add(
            u64::try_from(index * CRW_DIRECTORY_ENTRY_LENGTH).map_err(|_| {
                MetraError::InvalidOffset {
                    context: "CRW directory entry".to_owned(),
                    offset: index as u64,
                }
            })?,
        ) {
            Some(value) => value,
            None => {
                metadata.add_warning(
                    Warning::new("raw-crw-range", "CRW directory entry offset overflowed")
                        .at(count_position),
                );
                break;
            }
        };
        let entry_end = entry_local
            .checked_add(CRW_DIRECTORY_ENTRY_LENGTH as u64)
            .ok_or(MetraError::InvalidOffset {
                context: "CRW directory entry".to_owned(),
                offset: entry_local,
            })?;
        if entry_end > footer_offset {
            metadata.add_warning(
                Warning::new(
                    "raw-crw-directory",
                    "CRW directory entry table is truncated",
                )
                .at(base_offset + entry_local),
            );
            break;
        }
        let entry = read_at(
            reader,
            base_offset + entry_local,
            CRW_DIRECTORY_ENTRY_LENGTH,
            file_info.size,
            file_info,
            "CRW directory entry",
        )?;
        let tag = read_u16(endian, &entry[..2]);
        let type_bits = tag & 0x3800;
        let location_bits = tag & 0xC000;
        let tag_id = tag & 0x3FFF;
        if matches!(type_bits, 0x2800 | 0x3000) {
            metadata.add_warning(
                Warning::new(
                    "raw-crw-directory",
                    format!("nested CRW directory tag 0x{tag_id:04X} is not expanded"),
                )
                .at(base_offset + entry_local),
            );
            continue;
        }

        let (raw_value, source_offset, value_length) = if location_bits == 0x4000 {
            (entry[2..10].to_vec(), base_offset + entry_local + 2, 8_u64)
        } else if location_bits == 0 {
            let value_length = u64::from(read_u32(endian, &entry[2..6]));
            let value_offset = u64::from(read_u32(endian, &entry[6..10]));
            let Some(value_end) = value_offset.checked_add(value_length) else {
                metadata.add_warning(
                    Warning::new("raw-crw-range", "CRW value range overflowed")
                        .at(base_offset + entry_local),
                );
                continue;
            };
            if value_end > region_length || overlaps_entry(value_offset, value_length, entry_local)
            {
                metadata.add_warning(
                    Warning::new(
                        "raw-crw-range",
                        "CRW value range is outside its directory region",
                    )
                    .at(base_offset + entry_local),
                );
                continue;
            }
            if value_length > limits.max_value_bytes as u64 || tag_id == 0x2008 {
                add_descriptor(
                    metadata,
                    tag_id,
                    value_offset,
                    value_length,
                    base_offset + entry_local,
                    &entry,
                );
                continue;
            }
            let length = usize::try_from(value_length).map_err(|_| MetraError::InvalidOffset {
                context: "CRW value length".to_owned(),
                offset: value_length,
            })?;
            (
                read_at(
                    reader,
                    base_offset + value_offset,
                    length,
                    file_info.size,
                    file_info,
                    "CRW value",
                )?,
                base_offset + value_offset,
                value_length,
            )
        } else {
            metadata.add_warning(
                Warning::new(
                    "raw-crw-directory",
                    "CRW entry uses an unsupported data location",
                )
                .at(base_offset + entry_local),
            );
            continue;
        };
        decode_entry(
            metadata,
            tag_id,
            type_bits,
            &raw_value,
            source_offset,
            value_length,
            base_offset + entry_local,
            endian,
        );
    }
    Ok(())
}

fn overlaps_entry(value_offset: u64, value_length: u64, entry_offset: u64) -> bool {
    if value_offset < entry_offset {
        value_length > entry_offset - value_offset
    } else {
        value_offset < entry_offset.saturating_add(CRW_DIRECTORY_ENTRY_LENGTH as u64)
    }
}

fn add_descriptor(
    metadata: &mut Metadata,
    tag_id: u16,
    value_offset: u64,
    value_length: u64,
    entry_offset: u64,
    entry: &[u8],
) {
    let name = if tag_id == 0x2008 {
        "Preview"
    } else {
        "SkippedValue"
    };
    add_tag(
        metadata,
        format!("{name}Offset"),
        Some(u32::from(tag_id)),
        TagValue::Unsigned(value_offset),
        entry[..10].to_vec(),
        Source::new("CRW/directory", Some(entry_offset), Some(10)),
    );
    add_tag(
        metadata,
        format!("{name}Length"),
        Some(u32::from(tag_id)),
        TagValue::Unsigned(value_length),
        entry[..10].to_vec(),
        Source::new("CRW/directory", Some(entry_offset), Some(10)),
    );
}

#[allow(clippy::too_many_arguments)]
fn decode_entry(
    metadata: &mut Metadata,
    tag_id: u16,
    type_bits: u16,
    raw_value: &[u8],
    source_offset: u64,
    value_length: u64,
    entry_offset: u64,
    endian: Endian,
) {
    let source = Source::new("CRW/value", Some(source_offset), Some(value_length));
    match tag_id {
        0x0805 => {
            if let Some(value) = string_field(raw_value) {
                add_tag(
                    metadata,
                    "Comment".to_owned(),
                    Some(u32::from(tag_id)),
                    TagValue::String(value),
                    raw_value.to_vec(),
                    source,
                );
            }
        }
        0x080A => {
            let fields = raw_value
                .split(|value| *value == 0)
                .filter(|value| !value.is_empty());
            for (name, field) in ["Make", "Model"].into_iter().zip(fields) {
                add_tag(
                    metadata,
                    name.to_owned(),
                    Some(u32::from(tag_id)),
                    TagValue::String(String::from_utf8_lossy(field).to_string()),
                    field.to_vec(),
                    source.clone(),
                );
            }
        }
        0x1810 if raw_value.len() >= 8 => {
            add_scalar_from_bytes(
                metadata,
                "ImageWidth",
                tag_id,
                &raw_value[..4],
                source.clone(),
                endian,
            );
            add_scalar_from_bytes(
                metadata,
                "ImageHeight",
                tag_id,
                &raw_value[4..8],
                source.clone(),
                endian,
            );
            if raw_value.len() >= 16 {
                let rotation = read_i32(endian, &raw_value[12..16]);
                add_tag(
                    metadata,
                    "Rotation".to_owned(),
                    Some(u32::from(tag_id)),
                    TagValue::Signed(i64::from(rotation)),
                    raw_value[12..16].to_vec(),
                    source.clone(),
                );
            }
        }
        _ => {
            let value = match type_bits {
                0x0000 => {
                    if raw_value.len() == 1 {
                        TagValue::Unsigned(u64::from(raw_value[0]))
                    } else {
                        TagValue::Array(
                            raw_value
                                .iter()
                                .map(|value| TagValue::Unsigned(u64::from(*value)))
                                .collect(),
                        )
                    }
                }
                0x0800 => TagValue::String(string_field(raw_value).unwrap_or_default()),
                0x1000 => typed_array(raw_value, endian, 2),
                0x1800 => typed_array(raw_value, endian, 4),
                0x2000 => TagValue::Bytes(raw_value.to_vec()),
                _ => TagValue::Bytes(raw_value.to_vec()),
            };
            add_tag(
                metadata,
                format!("Tag0x{tag_id:04X}"),
                Some(u32::from(tag_id)),
                value,
                raw_value.to_vec(),
                Source::new("CRW/value", Some(entry_offset), Some(value_length)),
            );
        }
    }
}

fn typed_array(raw_value: &[u8], endian: Endian, width: usize) -> TagValue {
    if !raw_value.len().is_multiple_of(width) {
        return TagValue::Bytes(raw_value.to_vec());
    }
    TagValue::Array(
        raw_value
            .chunks_exact(width)
            .map(|chunk| {
                let value = if width == 2 {
                    u64::from(read_u16(endian, chunk))
                } else {
                    u64::from(read_u32(endian, chunk))
                };
                TagValue::Unsigned(value)
            })
            .collect(),
    )
}

fn add_scalar_from_bytes(
    metadata: &mut Metadata,
    name: &str,
    tag_id: u16,
    raw_value: &[u8],
    source: Source,
    endian: Endian,
) {
    add_tag(
        metadata,
        name.to_owned(),
        Some(u32::from(tag_id)),
        TagValue::Unsigned(u64::from(read_u32(endian, raw_value))),
        raw_value.to_vec(),
        source,
    );
}

fn add_u32_tag(
    metadata: &mut Metadata,
    name: &str,
    value: u64,
    raw_value: Vec<u8>,
    source: Source,
) {
    add_tag(
        metadata,
        name.to_owned(),
        None,
        TagValue::Unsigned(value),
        raw_value,
        source,
    );
}

fn add_tag(
    metadata: &mut Metadata,
    name: String,
    id: Option<u32>,
    value: TagValue,
    raw_value: Vec<u8>,
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
        namespace: "CRW".to_owned(),
        group: "CRW".to_owned(),
        id,
        name,
        description: Some("Canon CIFF/CRW metadata".to_owned()),
        raw_value: Some(raw_value),
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

fn read_i32(endian: Endian, bytes: &[u8]) -> i32 {
    match endian {
        Endian::Big => i32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
        Endian::Little => i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
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

    use metra_core::{FileFormat, FileInfo};

    use super::*;

    fn crw_fixture() -> Vec<u8> {
        let root_offset = CRW_HEADER_LENGTH;
        let make_model = b"Canon\0EOS-1D\0";
        let mut dimensions = [0_u8; 28];
        dimensions[..4].copy_from_slice(&5184_u32.to_le_bytes());
        dimensions[4..8].copy_from_slice(&3456_u32.to_le_bytes());
        dimensions[12..16].copy_from_slice(&90_i32.to_le_bytes());
        let directory_offset = make_model.len() + dimensions.len();
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"II");
        bytes.extend_from_slice(&(root_offset as u32).to_le_bytes());
        bytes.extend_from_slice(b"HEAPCCDR");
        bytes.extend_from_slice(&[2, 0, 0, 0]);
        bytes.extend_from_slice(&[0_u8; 8]);
        bytes.extend_from_slice(make_model);
        bytes.extend_from_slice(&dimensions);
        bytes.extend_from_slice(&2_u16.to_le_bytes());
        bytes.extend_from_slice(&0x080A_u16.to_le_bytes());
        bytes.extend_from_slice(&(make_model.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&0_u32.to_le_bytes());
        bytes.extend_from_slice(&0x1810_u16.to_le_bytes());
        bytes.extend_from_slice(&(dimensions.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&(make_model.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&(directory_offset as u32).to_le_bytes());
        bytes
    }

    #[test]
    fn reads_crw_common_ciff_values_without_loading_preview_data() {
        let bytes = crw_fixture();
        let metadata = read_crw(
            &mut Cursor::new(bytes.clone()),
            FileInfo::new("capture.crw".into(), bytes.len() as u64, FileFormat::Raw),
            ParseLimits::default(),
        )
        .unwrap();
        assert_eq!(metadata.find("RAW:Variant").unwrap().display_value(), "CRW");
        assert_eq!(metadata.find("CRW:Make").unwrap().display_value(), "Canon");
        assert_eq!(
            metadata.find("CRW:Model").unwrap().display_value(),
            "EOS-1D"
        );
        assert_eq!(
            metadata.find("CRW:ImageWidth").unwrap().display_value(),
            "5184"
        );
        assert_eq!(metadata.find("CRW:Rotation").unwrap().display_value(), "90");
        assert!(metadata.warnings.is_empty());
    }
}
