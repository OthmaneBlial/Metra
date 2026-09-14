use std::io::{Read, Seek, SeekFrom};

use metra_core::{
    FileInfo, Metadata, MetraError, ParseLimits, Result, Source, Tag, TagValue, ValueType, Warning,
};

const X3F_SIGNATURE: &[u8; 4] = b"FOVb";
const X3F_HEADER_V20_LENGTH: usize = 40;
const X3F_HEADER_V21_LENGTH: usize = 232;
const X3F_DIRECTORY_HEADER_LENGTH: usize = 12;
const X3F_DIRECTORY_ENTRY_LENGTH: usize = 12;
const X3F_PROP_HEADER_LENGTH: usize = 24;
const X3F_IMAGE_HEADER_LENGTH: usize = 28;
const X3F_MAX_DIRECTORY_ENTRIES: usize = 256;

/// Read bounded Sigma/Foveon X3F metadata without touching image payloads.
pub fn read_x3f<R: Read + Seek>(
    reader: &mut R,
    file_info: FileInfo,
    limits: ParseLimits,
) -> Result<Metadata> {
    let file_length = file_info.size;
    let mut metadata = Metadata::new(file_info.clone());
    crate::raw::add_identity(&mut metadata, "X3F");
    if file_length < X3F_HEADER_V20_LENGTH as u64 {
        metadata.add_warning(
            Warning::new(
                "raw-x3f-partial",
                "X3F header is truncated; only container identity is available",
            )
            .at(file_length),
        );
        return Ok(metadata);
    }

    let header = read_at(
        reader,
        0,
        X3F_HEADER_V20_LENGTH,
        file_length,
        &file_info,
        "X3F header",
    )?;
    if !header.starts_with(X3F_SIGNATURE) {
        return Err(MetraError::InvalidHeader {
            context: "X3F".to_owned(),
            message: "invalid Sigma/Foveon signature".to_owned(),
        });
    }

    let version = read_u32_le(&header[4..8]);
    add_header_u32(
        &mut metadata,
        "Version",
        4,
        u64::from(version),
        &header[4..8],
        "X3F/header",
    );
    add_header_u32(
        &mut metadata,
        "VersionMajor",
        4,
        u64::from(version >> 16),
        &header[4..8],
        "X3F/header",
    );
    add_header_u32(
        &mut metadata,
        "VersionMinor",
        4,
        u64::from(version & 0xFFFF),
        &header[4..8],
        "X3F/header",
    );
    add_header_tag(
        &mut metadata,
        "UniqueIdentifier".to_owned(),
        None,
        TagValue::Bytes(header[8..24].to_vec()),
        header[8..24].to_vec(),
        Source::new("X3F/header", Some(8), Some(16)),
    );
    add_header_u32(
        &mut metadata,
        "MarkBits",
        24,
        u64::from(read_u32_le(&header[24..28])),
        &header[24..28],
        "X3F/header",
    );
    add_header_u32(
        &mut metadata,
        "ImageColumns",
        28,
        u64::from(read_u32_le(&header[28..32])),
        &header[28..32],
        "X3F/header",
    );
    add_header_u32(
        &mut metadata,
        "ImageRows",
        32,
        u64::from(read_u32_le(&header[32..36])),
        &header[32..36],
        "X3F/header",
    );
    let rotation = read_u32_le(&header[36..40]);
    add_header_u32(
        &mut metadata,
        "Rotation",
        36,
        u64::from(rotation),
        &header[36..40],
        "X3F/header",
    );
    if !matches!(rotation, 0 | 90 | 180 | 270) {
        metadata.add_warning(
            Warning::new(
                "raw-x3f-rotation",
                format!("X3F rotation {rotation} is outside the documented values"),
            )
            .at(36),
        );
    }

    if version >= 0x0002_0001 {
        if file_length < X3F_HEADER_V21_LENGTH as u64 {
            metadata.add_warning(
                Warning::new(
                    "raw-x3f-header",
                    "X3F version advertises extended header data but the header is truncated",
                )
                .at(file_length),
            );
        } else {
            let extended = read_at(
                reader,
                40,
                X3F_HEADER_V21_LENGTH - 40,
                file_length,
                &file_info,
                "X3F extended header",
            )?;
            if let Some(label) = string_field(&extended[..32]) {
                add_header_tag(
                    &mut metadata,
                    "WhiteBalanceLabel".to_owned(),
                    None,
                    TagValue::String(label),
                    extended[..32].to_vec(),
                    Source::new("X3F/header", Some(40), Some(32)),
                );
            }
            for (index, kind) in extended[32..64].iter().copied().enumerate() {
                if kind == 0 {
                    continue;
                }
                let name = format!("Extended{}", extended_type_name(kind));
                add_header_tag(
                    &mut metadata,
                    name,
                    Some(index as u32),
                    TagValue::Float(f32::from_le_bytes([
                        extended[64 + index * 4],
                        extended[65 + index * 4],
                        extended[66 + index * 4],
                        extended[67 + index * 4],
                    ]) as f64),
                    extended[64 + index * 4..68 + index * 4].to_vec(),
                    Source::new("X3F/header", Some(104 + (index * 4) as u64), Some(4)),
                );
            }
        }
    }

    let pointer_offset = file_length
        .checked_sub(4)
        .ok_or(MetraError::UnexpectedEof {
            context: "X3F directory pointer".to_owned(),
        })?;
    let pointer = read_at(
        reader,
        pointer_offset,
        4,
        file_length,
        &file_info,
        "X3F directory pointer",
    )?;
    let directory_offset = u64::from(read_u32_le(&pointer));
    let directory_limit = pointer_offset;
    let directory_header_end = directory_offset.checked_add(X3F_DIRECTORY_HEADER_LENGTH as u64);
    let Some(directory_header_end) = directory_header_end else {
        metadata.add_warning(
            Warning::new("raw-x3f-range", "X3F directory header offset overflowed")
                .at(pointer_offset),
        );
        metadata.sort_tags();
        return Ok(metadata);
    };
    if directory_header_end > directory_limit {
        metadata.add_warning(
            Warning::new(
                "raw-x3f-range",
                "X3F directory header is outside the file before the directory pointer",
            )
            .at(directory_offset),
        );
        metadata.sort_tags();
        return Ok(metadata);
    }
    let directory_header = read_at(
        reader,
        directory_offset,
        X3F_DIRECTORY_HEADER_LENGTH,
        file_length,
        &file_info,
        "X3F directory header",
    )?;
    if !directory_header.starts_with(b"SECd") {
        metadata.add_warning(
            Warning::new(
                "raw-x3f-directory",
                "X3F directory does not start with SECd",
            )
            .at(directory_offset),
        );
        metadata.sort_tags();
        return Ok(metadata);
    }
    add_header_u32(
        &mut metadata,
        "DirectoryVersion",
        directory_offset + 4,
        u64::from(read_u32_le(&directory_header[4..8])),
        &directory_header[4..8],
        "X3F/directory",
    );
    let declared_entries = read_u32_le(&directory_header[8..12]) as usize;
    let entry_count = declared_entries
        .min(X3F_MAX_DIRECTORY_ENTRIES)
        .min(limits.max_ifd_entries);
    add_header_tag(
        &mut metadata,
        "DirectoryEntryCount".to_owned(),
        None,
        TagValue::Unsigned(declared_entries as u64),
        directory_header[8..12].to_vec(),
        Source::new("X3F/directory", Some(directory_offset + 8), Some(4)),
    );
    if declared_entries > entry_count {
        metadata.add_warning(
            Warning::new(
                "raw-x3f-directory-limit",
                format!(
                    "X3F directory declares {declared_entries} entries; reading only {entry_count}"
                ),
            )
            .at(directory_offset + 8),
        );
    }

    let mut entry_offset = directory_offset + X3F_DIRECTORY_HEADER_LENGTH as u64;
    for index in 0..entry_count {
        let entry_end = match entry_offset.checked_add(X3F_DIRECTORY_ENTRY_LENGTH as u64) {
            Some(value) => value,
            None => {
                metadata.add_warning(
                    Warning::new("raw-x3f-range", "X3F directory entry offset overflowed")
                        .at(entry_offset),
                );
                break;
            }
        };
        if entry_end > directory_limit {
            metadata.add_warning(
                Warning::new(
                    "raw-x3f-directory",
                    "X3F directory entry table is truncated",
                )
                .at(entry_offset),
            );
            break;
        }
        let entry = read_at(
            reader,
            entry_offset,
            X3F_DIRECTORY_ENTRY_LENGTH,
            file_length,
            &file_info,
            "X3F directory entry",
        )?;
        let section_offset = u64::from(read_u32_le(&entry[..4]));
        let section_length = u64::from(read_u32_le(&entry[4..8]));
        let section_type = String::from_utf8_lossy(&entry[8..12]).to_string();
        let section_name = format!("Section{}", index + 1);
        add_header_tag(
            &mut metadata,
            format!("{section_name}Type"),
            Some(index as u32),
            TagValue::String(section_type.clone()),
            entry[8..12].to_vec(),
            Source::new("X3F/directory", Some(entry_offset + 8), Some(4)),
        );
        add_header_tag(
            &mut metadata,
            format!("{section_name}Offset"),
            Some(index as u32),
            TagValue::Unsigned(section_offset),
            entry[..4].to_vec(),
            Source::new("X3F/directory", Some(entry_offset), Some(4)),
        );
        add_header_tag(
            &mut metadata,
            format!("{section_name}Length"),
            Some(index as u32),
            TagValue::Unsigned(section_length),
            entry[4..8].to_vec(),
            Source::new("X3F/directory", Some(entry_offset + 4), Some(4)),
        );
        let Some(section_end) = section_offset.checked_add(section_length) else {
            metadata.add_warning(
                Warning::new("raw-x3f-range", "X3F section range overflowed").at(section_offset),
            );
            entry_offset = entry_end;
            continue;
        };
        if section_offset < X3F_HEADER_V20_LENGTH as u64 || section_end > directory_offset {
            metadata.add_warning(
                Warning::new(
                    "raw-x3f-range",
                    format!("X3F {section_type} section overlaps the header or directory"),
                )
                .at(section_offset),
            );
            entry_offset = entry_end;
            continue;
        }
        match section_type.as_bytes() {
            b"PROP" if section_length <= limits.max_metadata_bytes as u64 => {
                let length =
                    usize::try_from(section_length).map_err(|_| MetraError::InvalidOffset {
                        context: "X3F PROP section length".to_owned(),
                        offset: section_length,
                    })?;
                let section = read_at(
                    reader,
                    section_offset,
                    length,
                    file_length,
                    &file_info,
                    "X3F PROP section",
                )?;
                parse_prop(&section, section_offset, &mut metadata, limits);
            }
            b"PROP" => metadata.add_warning(
                Warning::new(
                    "raw-x3f-range-limit",
                    "X3F PROP section exceeds the metadata budget",
                )
                .at(section_offset),
            ),
            b"IMAG" | b"IMA2" => {
                if section_length < X3F_IMAGE_HEADER_LENGTH as u64 {
                    metadata.add_warning(
                        Warning::new("raw-x3f-image", "X3F image section header is truncated")
                            .at(section_offset),
                    );
                } else {
                    let image_header = read_at(
                        reader,
                        section_offset,
                        X3F_IMAGE_HEADER_LENGTH,
                        file_length,
                        &file_info,
                        "X3F image section header",
                    )?;
                    parse_image_header(&image_header, section_offset, index, &mut metadata);
                }
            }
            _ => {}
        }
        entry_offset = entry_end;
    }
    metadata.sort_tags();
    Ok(metadata)
}

fn parse_prop(section: &[u8], offset: u64, metadata: &mut Metadata, limits: ParseLimits) {
    if section.len() < X3F_PROP_HEADER_LENGTH || !section.starts_with(b"SECp") {
        metadata.add_warning(
            Warning::new(
                "raw-x3f-prop",
                "X3F PROP section header is invalid or truncated",
            )
            .at(offset),
        );
        return;
    }
    let declared_entries = read_u32_le(&section[8..12]) as usize;
    let entry_count = declared_entries.min(limits.max_ifd_entries);
    let character_format = read_u32_le(&section[12..16]);
    let total_characters = read_u32_le(&section[20..24]) as usize;
    if character_format != 0 {
        metadata.add_warning(
            Warning::new(
                "raw-x3f-prop-format",
                format!("unsupported X3F PROP character format {character_format}"),
            )
            .at(offset + 12),
        );
        return;
    }
    if declared_entries > entry_count {
        metadata.add_warning(
            Warning::new(
                "raw-x3f-prop-limit",
                format!("X3F PROP declares {declared_entries} entries; reading only {entry_count}"),
            )
            .at(offset + 8),
        );
    }
    let entries_length = match entry_count.checked_mul(8) {
        Some(value) => value,
        None => {
            metadata.add_warning(
                Warning::new("raw-x3f-prop", "X3F PROP entry table length overflowed").at(offset),
            );
            return;
        }
    };
    let character_data_start = match X3F_PROP_HEADER_LENGTH.checked_add(entries_length) {
        Some(value) => value,
        None => return,
    };
    let character_data_length = match total_characters.checked_mul(2) {
        Some(value) => value,
        None => {
            metadata.add_warning(
                Warning::new("raw-x3f-prop", "X3F PROP character data length overflowed")
                    .at(offset + 20),
            );
            return;
        }
    };
    let Some(character_data_end) = character_data_start.checked_add(character_data_length) else {
        metadata.add_warning(
            Warning::new("raw-x3f-prop", "X3F PROP character data range overflowed").at(offset),
        );
        return;
    };
    if character_data_end > section.len() {
        metadata.add_warning(
            Warning::new("raw-x3f-prop", "X3F PROP character data is truncated").at(offset),
        );
        return;
    }
    for index in 0..entry_count {
        let entry_start = X3F_PROP_HEADER_LENGTH + index * 8;
        let name_offset = read_u32_le(&section[entry_start..entry_start + 4]) as usize;
        let value_offset = read_u32_le(&section[entry_start + 4..entry_start + 8]) as usize;
        let Some((property_name, _, _)) = decode_prop_string(
            &section[character_data_start..character_data_end],
            name_offset,
            total_characters,
            limits,
        ) else {
            metadata.add_warning(
                Warning::new("raw-x3f-prop", "X3F PROP name offset is invalid")
                    .at(offset + entry_start as u64),
            );
            continue;
        };
        let Some((property_value, raw_value, raw_length)) = decode_prop_string(
            &section[character_data_start..character_data_end],
            value_offset,
            total_characters,
            limits,
        ) else {
            metadata.add_warning(
                Warning::new("raw-x3f-prop", "X3F PROP value offset is invalid")
                    .at(offset + entry_start as u64 + 4),
            );
            continue;
        };
        if property_name.is_empty() {
            metadata.add_warning(
                Warning::new("raw-x3f-prop", "X3F PROP name is empty")
                    .at(offset + character_data_start as u64 + (name_offset as u64 * 2)),
            );
            continue;
        }
        add_header_tag(
            metadata,
            format!("PROP:{property_name}"),
            Some(index as u32),
            TagValue::String(property_value),
            raw_value,
            Source::new(
                "X3F/PROP",
                Some(offset + character_data_start as u64 + (value_offset as u64 * 2)),
                Some(raw_length as u64),
            ),
        );
    }
}

fn decode_prop_string(
    bytes: &[u8],
    start_character: usize,
    total_characters: usize,
    limits: ParseLimits,
) -> Option<(String, Vec<u8>, usize)> {
    if start_character >= total_characters {
        return None;
    }
    let max_characters = (limits.max_value_bytes / 2).max(1);
    let mut values = Vec::new();
    let mut raw_length = 0_usize;
    for character in start_character..total_characters {
        let byte_offset = character.checked_mul(2)?;
        let pair = bytes.get(byte_offset..byte_offset + 2)?;
        let value = u16::from_le_bytes([pair[0], pair[1]]);
        raw_length = raw_length.saturating_add(2);
        if value == 0 {
            break;
        }
        if values.len() >= max_characters {
            return None;
        }
        values.push(value);
    }
    let start = start_character.checked_mul(2)?;
    let raw_end = start.checked_add(raw_length)?;
    let raw_value = bytes.get(start..raw_end)?.to_vec();
    Some((String::from_utf16_lossy(&values), raw_value, raw_length))
}

fn parse_image_header(section: &[u8], offset: u64, index: usize, metadata: &mut Metadata) {
    if !section.starts_with(b"SECi") {
        metadata.add_warning(
            Warning::new(
                "raw-x3f-image",
                "X3F image section does not start with SECi",
            )
            .at(offset),
        );
        return;
    }
    let prefix = format!("Image{}", index + 1);
    for (name, position) in [
        ("Version", 4),
        ("Type", 8),
        ("Format", 12),
        ("Columns", 16),
        ("Rows", 20),
        ("RowBytes", 24),
    ] {
        let raw_value = section[position..position + 4].to_vec();
        add_header_tag(
            metadata,
            format!("{prefix}{name}"),
            Some(index as u32),
            TagValue::Unsigned(u64::from(read_u32_le(&raw_value))),
            raw_value,
            Source::new("X3F/image", Some(offset + position as u64), Some(4)),
        );
    }
}

fn add_header_u32(
    metadata: &mut Metadata,
    name: &str,
    position: u64,
    value: u64,
    raw_value: &[u8],
    group: &str,
) {
    add_header_tag(
        metadata,
        name.to_owned(),
        None,
        TagValue::Unsigned(value),
        raw_value.to_vec(),
        Source::new(group, Some(position), Some(raw_value.len() as u64)),
    );
}

fn add_header_tag(
    metadata: &mut Metadata,
    name: String,
    id: Option<u32>,
    value: TagValue,
    raw_value: Vec<u8>,
    source: Source,
) {
    let value_type = match value {
        TagValue::String(_) => ValueType::String,
        TagValue::Unsigned(_) => ValueType::UnsignedInteger,
        TagValue::Float(_) => ValueType::Float,
        TagValue::Bytes(_) => ValueType::Bytes,
        _ => ValueType::Unknown,
    };
    metadata.add_tag(Tag {
        namespace: "X3F".to_owned(),
        group: "X3F".to_owned(),
        id,
        name,
        description: Some("Sigma/Foveon X3F metadata".to_owned()),
        raw_value: Some(raw_value),
        value,
        value_type,
        source,
        writable: false,
    });
}

fn extended_type_name(kind: u8) -> &'static str {
    match kind {
        1 => "ExposureAdjust",
        2 => "ContrastAdjust",
        3 => "ShadowAdjust",
        4 => "HighlightAdjust",
        5 => "SaturationAdjust",
        6 => "SharpnessAdjust",
        7 => "ColorAdjustRed",
        8 => "ColorAdjustGreen",
        9 => "ColorAdjustBlue",
        10 => "FillLightAdjust",
        _ => "Unknown",
    }
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

fn read_u32_le(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
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

    fn push_utf16le(output: &mut Vec<u8>, value: &str) {
        for character in value.encode_utf16().chain(std::iter::once(0)) {
            output.extend_from_slice(&character.to_le_bytes());
        }
    }

    fn x3f_fixture() -> Vec<u8> {
        let mut bytes = vec![0_u8; X3F_HEADER_V21_LENGTH];
        bytes[..4].copy_from_slice(X3F_SIGNATURE);
        bytes[4..8].copy_from_slice(&0x0002_0002_u32.to_le_bytes());
        bytes[8..24].copy_from_slice(b"X3F-TEST-IDENT\0\0");
        bytes[28..32].copy_from_slice(&2640_u32.to_le_bytes());
        bytes[32..36].copy_from_slice(&1760_u32.to_le_bytes());
        bytes[36..40].copy_from_slice(&90_u32.to_le_bytes());
        bytes[40..48].copy_from_slice(b"Sunlight");
        bytes[48] = 0;
        bytes[72] = 1;
        bytes[104..108].copy_from_slice(&1.5_f32.to_le_bytes());

        let section_offset = bytes.len();
        let mut prop = Vec::new();
        prop.extend_from_slice(b"SECp");
        prop.extend_from_slice(&0x0002_0000_u32.to_le_bytes());
        prop.extend_from_slice(&1_u32.to_le_bytes());
        prop.extend_from_slice(&0_u32.to_le_bytes());
        prop.extend_from_slice(&0_u32.to_le_bytes());
        prop.extend_from_slice(&13_u32.to_le_bytes());
        prop.extend_from_slice(&0_u32.to_le_bytes());
        prop.extend_from_slice(&9_u32.to_le_bytes());
        push_utf16le(&mut prop, "CAMMODEL");
        push_utf16le(&mut prop, "SD1");
        while prop.len() % 4 != 0 {
            prop.push(0);
        }
        bytes.extend_from_slice(&prop);

        let directory_offset = bytes.len();
        bytes.extend_from_slice(b"SECd");
        bytes.extend_from_slice(&0x0002_0000_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u32.to_le_bytes());
        bytes.extend_from_slice(&(section_offset as u32).to_le_bytes());
        bytes.extend_from_slice(&(prop.len() as u32).to_le_bytes());
        bytes.extend_from_slice(b"PROP");
        bytes.extend_from_slice(&(directory_offset as u32).to_le_bytes());
        bytes
    }

    #[test]
    fn reads_x3f_header_directory_and_properties_without_image_payload() {
        let bytes = x3f_fixture();
        let metadata = read_x3f(
            &mut Cursor::new(bytes.clone()),
            FileInfo::new("capture.x3f".into(), bytes.len() as u64, FileFormat::Raw),
            ParseLimits::default(),
        )
        .unwrap();
        assert_eq!(metadata.find("RAW:Variant").unwrap().display_value(), "X3F");
        assert_eq!(
            metadata.find("X3F:ImageColumns").unwrap().display_value(),
            "2640"
        );
        assert_eq!(
            metadata
                .find("X3F:WhiteBalanceLabel")
                .unwrap()
                .display_value(),
            "Sunlight"
        );
        assert_eq!(
            metadata.find("X3F:PROP:CAMMODEL").unwrap().display_value(),
            "SD1"
        );
        assert!(metadata.warnings.is_empty());
    }

    #[test]
    fn warns_when_x3f_directory_is_missing() {
        let mut bytes = vec![0_u8; X3F_HEADER_V20_LENGTH];
        bytes[..4].copy_from_slice(X3F_SIGNATURE);
        bytes[4..8].copy_from_slice(&0x0002_0000_u32.to_le_bytes());
        let metadata = read_x3f(
            &mut Cursor::new(bytes.clone()),
            FileInfo::new("capture.x3f".into(), bytes.len() as u64, FileFormat::Raw),
            ParseLimits::default(),
        )
        .unwrap();
        assert!(
            metadata
                .warnings
                .iter()
                .any(|warning| warning.code == "raw-x3f-directory")
        );
    }
}
