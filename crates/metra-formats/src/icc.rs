use std::io::{Read, Seek};

use metra_core::{
    FileInfo, Metadata, MetraError, ParseLimits, Result, Source, Tag, TagValue, ValueType, Warning,
};

pub fn read_icc<R: Read + Seek>(
    reader: &mut R,
    file_info: FileInfo,
    limits: ParseLimits,
) -> Result<Metadata> {
    let bytes = crate::read_bounded_document(reader, &file_info, limits, "ICC profile")?;
    let mut metadata = Metadata::new(file_info);
    parse_icc_profile(&bytes, 0, &mut metadata, limits)?;
    metadata.sort_tags();
    Ok(metadata)
}

pub(crate) fn is_icc_signature(bytes: &[u8]) -> bool {
    bytes.len() >= 40 && &bytes[36..40] == b"acsp"
}

pub(crate) fn parse_icc_profile(
    bytes: &[u8],
    data_offset: u64,
    metadata: &mut Metadata,
    limits: ParseLimits,
) -> Result<()> {
    if bytes.len() > limits.max_value_bytes {
        return Err(MetraError::ResourceLimitExceeded {
            resource: "ICC profile".to_owned(),
            limit: limits.max_value_bytes,
        });
    }
    if bytes.len() < 132 {
        return Err(MetraError::UnexpectedEof {
            context: "ICC profile header".to_owned(),
        });
    }
    if &bytes[36..40] != b"acsp" {
        return Err(MetraError::InvalidHeader {
            context: "ICC profile".to_owned(),
            message: "missing acsp signature".to_owned(),
        });
    }
    let profile_size = u32::from_be_bytes(bytes[..4].try_into().expect("profile size"));
    if u64::from(profile_size) > bytes.len() as u64 {
        metadata.add_warning(Warning::new(
            "icc-truncated",
            format!(
                "ICC profile declares {profile_size} bytes, chunk contains {}",
                bytes.len()
            ),
        ));
    }
    add_tag(
        metadata,
        "ProfileSize",
        TagValue::Unsigned(u64::from(profile_size)),
        ValueType::UnsignedInteger,
        data_offset,
        4,
    );
    let version = format!("{}.{}", bytes[8], bytes[9] >> 4);
    add_tag(
        metadata,
        "Version",
        TagValue::String(version),
        ValueType::String,
        data_offset + 8,
        4,
    );
    add_tag(
        metadata,
        "ColorSpace",
        TagValue::String(signature(&bytes[16..20])),
        ValueType::String,
        data_offset + 16,
        4,
    );
    add_tag(
        metadata,
        "PCS",
        TagValue::String(signature(&bytes[20..24])),
        ValueType::String,
        data_offset + 20,
        4,
    );
    add_tag(
        metadata,
        "DeviceClass",
        TagValue::String(signature(&bytes[12..16])),
        ValueType::String,
        data_offset + 12,
        4,
    );
    add_tag(
        metadata,
        "Platform",
        TagValue::String(signature(&bytes[40..44])),
        ValueType::String,
        data_offset + 40,
        4,
    );
    add_tag(
        metadata,
        "Manufacturer",
        TagValue::String(signature(&bytes[48..52])),
        ValueType::String,
        data_offset + 48,
        4,
    );
    add_tag(
        metadata,
        "Model",
        TagValue::String(signature(&bytes[52..56])),
        ValueType::String,
        data_offset + 52,
        4,
    );
    add_tag(
        metadata,
        "CreationDate",
        TagValue::String(parse_datetime(&bytes[24..36])),
        ValueType::String,
        data_offset + 24,
        12,
    );
    add_tag(
        metadata,
        "RenderingIntent",
        TagValue::Unsigned(u64::from(u32::from_be_bytes(
            bytes[64..68].try_into().expect("rendering intent"),
        ))),
        ValueType::UnsignedInteger,
        data_offset + 64,
        4,
    );
    add_tag(
        metadata,
        "Illuminant",
        TagValue::Array(
            [68..72, 72..76, 76..80]
                .into_iter()
                .map(|range| TagValue::Float(parse_fixed(&bytes[range])))
                .collect(),
        ),
        ValueType::Array,
        data_offset + 68,
        12,
    );
    if bytes[84..100].iter().any(|byte| *byte != 0) {
        add_tag(
            metadata,
            "ProfileID",
            TagValue::Bytes(bytes[84..100].to_vec()),
            ValueType::Bytes,
            data_offset + 84,
            16,
        );
    }

    let tag_count = u32::from_be_bytes(bytes[128..132].try_into().expect("tag count"));
    let tag_count_usize =
        usize::try_from(tag_count).map_err(|_| MetraError::ResourceLimitExceeded {
            resource: "ICC tag table".to_owned(),
            limit: limits.max_ifd_entries,
        })?;
    let table_end = 132_usize
        .checked_add(
            tag_count_usize
                .checked_mul(12)
                .ok_or(MetraError::InvalidOffset {
                    context: "ICC tag table".to_owned(),
                    offset: tag_count as u64,
                })?,
        )
        .ok_or(MetraError::InvalidOffset {
            context: "ICC tag table".to_owned(),
            offset: tag_count as u64,
        })?;
    if table_end > bytes.len() {
        metadata.add_warning(Warning::new(
            "icc-tag-table",
            "ICC tag table extends beyond the profile payload",
        ));
        return Ok(());
    }
    let tag_count = tag_count_usize;
    if tag_count > limits.max_ifd_entries {
        metadata.add_warning(Warning::new(
            "icc-tag-limit",
            format!("ICC declares {tag_count} tags; table was not decoded"),
        ));
        return Ok(());
    }
    for index in 0..tag_count {
        let start = 132 + index * 12;
        let tag_signature = signature(&bytes[start..start + 4]);
        let tag_id = Some(u32::from_be_bytes(
            bytes[start..start + 4]
                .try_into()
                .expect("ICC tag signature"),
        ));
        let value_offset =
            u32::from_be_bytes(bytes[start + 4..start + 8].try_into().expect("offset"));
        let value_size = u32::from_be_bytes(bytes[start + 8..start + 12].try_into().expect("size"));
        let value_start = usize::try_from(value_offset).unwrap_or(usize::MAX);
        let value_end = value_start.checked_add(usize::try_from(value_size).unwrap_or(usize::MAX));
        let Some(value_end) = value_end else {
            metadata.add_warning(Warning::new(
                "icc-tag-offset",
                format!("ICC tag {tag_signature} overflows"),
            ));
            continue;
        };
        if value_end > bytes.len() {
            metadata.add_warning(Warning::new(
                "icc-tag-offset",
                format!("ICC tag {tag_signature} is outside the profile payload"),
            ));
            continue;
        }
        if let Some((name, value, value_type)) =
            parse_table_tag(&tag_signature, &bytes[value_start..value_end])
        {
            add_tag_with_id(
                metadata,
                tag_id,
                name,
                value,
                value_type,
                data_offset + value_offset as u64,
                value_size as u64,
            );
        }
    }
    Ok(())
}

fn add_tag(
    metadata: &mut Metadata,
    name: &str,
    value: TagValue,
    value_type: ValueType,
    offset: u64,
    length: u64,
) {
    add_tag_with_id(metadata, None, name, value, value_type, offset, length);
}

fn add_tag_with_id(
    metadata: &mut Metadata,
    id: Option<u32>,
    name: &str,
    value: TagValue,
    value_type: ValueType,
    offset: u64,
    length: u64,
) {
    metadata.add_tag(Tag {
        namespace: "ICC".to_owned(),
        group: "Profile".to_owned(),
        id,
        name: name.to_owned(),
        description: Some("ICC profile property".to_owned()),
        raw_value: None,
        value,
        value_type,
        source: Source::new("ICC/Profile", Some(offset), Some(length)),
        writable: false,
    });
}

fn signature(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .trim_end_matches(' ')
        .to_owned()
}

fn parse_datetime(bytes: &[u8]) -> String {
    let values = bytes
        .chunks_exact(2)
        .map(|chunk| u16::from_be_bytes([chunk[0], chunk[1]]))
        .collect::<Vec<_>>();
    if values.len() == 6 {
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            values[0], values[1], values[2], values[3], values[4], values[5]
        )
    } else {
        "unknown".to_owned()
    }
}

fn parse_fixed(bytes: &[u8]) -> f64 {
    let value = i32::from_be_bytes(bytes.try_into().expect("ICC fixed-point value"));
    f64::from(value) / 65_536.0
}

fn parse_table_tag(signature: &str, bytes: &[u8]) -> Option<(&'static str, TagValue, ValueType)> {
    let name = match signature {
        "desc" => "Description",
        "cprt" => "Copyright",
        "dmnd" => "ManufacturerDescription",
        "dmdd" => "ModelDescription",
        "wtpt" => "MediaWhitePoint",
        "bkpt" => "MediaBlackPoint",
        "lumi" => "Luminance",
        "rXYZ" => "RedMatrixColumn",
        "gXYZ" => "GreenMatrixColumn",
        "bXYZ" => "BlueMatrixColumn",
        _ => return None,
    };
    if matches!(
        signature,
        "wtpt" | "bkpt" | "lumi" | "rXYZ" | "gXYZ" | "bXYZ"
    ) {
        let value = parse_xyz(bytes)?;
        return Some((name, value, ValueType::Array));
    }
    let text = parse_desc(bytes)
        .or_else(|| parse_mluc(bytes))
        .or_else(|| parse_text(bytes))?;
    Some((name, TagValue::String(text), ValueType::String))
}

fn parse_xyz(bytes: &[u8]) -> Option<TagValue> {
    if bytes.len() < 20 || &bytes[..4] != b"XYZ " {
        return None;
    }
    Some(TagValue::Array(
        bytes[8..20]
            .chunks_exact(4)
            .map(|chunk| TagValue::Float(parse_fixed(chunk)))
            .collect(),
    ))
}

fn parse_text(bytes: &[u8]) -> Option<String> {
    if bytes.len() < 8 || &bytes[..4] != b"text" {
        return None;
    }
    Some(
        String::from_utf8_lossy(&bytes[8..])
            .trim_end_matches('\0')
            .to_owned(),
    )
}

fn parse_desc(bytes: &[u8]) -> Option<String> {
    if bytes.len() < 12 || &bytes[..4] != b"desc" {
        return None;
    }
    let length = usize::try_from(u32::from_be_bytes(bytes[8..12].try_into().ok()?)).ok()?;
    let end = 12_usize.checked_add(length)?.min(bytes.len());
    Some(
        String::from_utf8_lossy(&bytes[12..end])
            .trim_end_matches('\0')
            .to_owned(),
    )
}

fn parse_mluc(bytes: &[u8]) -> Option<String> {
    if bytes.len() < 28 || &bytes[..4] != b"mluc" {
        return None;
    }
    let count = usize::try_from(u32::from_be_bytes(bytes[8..12].try_into().ok()?)).ok()?;
    let record_size = usize::try_from(u32::from_be_bytes(bytes[12..16].try_into().ok()?)).ok()?;
    if count == 0 || record_size < 12 {
        return None;
    }
    let record_end = 16_usize.checked_add(record_size)?;
    if record_end > bytes.len() {
        return None;
    }
    let length = usize::try_from(u32::from_be_bytes(bytes[20..24].try_into().ok()?)).ok()?;
    let offset = usize::try_from(u32::from_be_bytes(bytes[24..28].try_into().ok()?)).ok()?;
    let end = offset.checked_add(length)?;
    if end > bytes.len() || length % 2 != 0 {
        return None;
    }
    let units = bytes[offset..end]
        .chunks_exact(2)
        .map(|chunk| u16::from_be_bytes([chunk[0], chunk[1]]))
        .collect::<Vec<_>>();
    Some(String::from_utf16_lossy(&units))
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;
    use metra_core::{FileFormat, FileInfo};

    #[test]
    fn reads_profile_header_and_legacy_description() {
        let mut description = b"desc".to_vec();
        description.extend_from_slice(&[0; 4]);
        description.extend_from_slice(&6_u32.to_be_bytes());
        description.extend_from_slice(b"Metra\0");
        let profile_offset = 144_u32;
        let mut bytes = vec![0_u8; profile_offset as usize];
        bytes[8] = 4;
        bytes[9] = 0x30;
        bytes[16..20].copy_from_slice(b"RGB ");
        bytes[20..24].copy_from_slice(b"XYZ ");
        bytes[12..16].copy_from_slice(b"mntr");
        bytes[24..36].copy_from_slice(&[0x07, 0xEA, 0, 9, 0, 13, 0, 12, 0, 34, 0, 56]);
        bytes[40..44].copy_from_slice(b"APPL");
        bytes[48..52].copy_from_slice(b"TEST");
        bytes[52..56].copy_from_slice(b"MODL");
        bytes[64..68].copy_from_slice(&1_u32.to_be_bytes());
        bytes[68..72].copy_from_slice(&(((95_047_i64 * 65_536) / 100_000) as i32).to_be_bytes());
        bytes[72..76].copy_from_slice(&(((100_000_i64 * 65_536) / 100_000) as i32).to_be_bytes());
        bytes[76..80].copy_from_slice(&(((108_883_i64 * 65_536) / 100_000) as i32).to_be_bytes());
        bytes[84] = 1;
        bytes[36..40].copy_from_slice(b"acsp");
        bytes[128..132].copy_from_slice(&1_u32.to_be_bytes());
        bytes[132..136].copy_from_slice(b"desc");
        bytes[136..140].copy_from_slice(&profile_offset.to_be_bytes());
        bytes[140..144].copy_from_slice(&(description.len() as u32).to_be_bytes());
        bytes.extend_from_slice(&description);
        let size = bytes.len() as u32;
        bytes[0..4].copy_from_slice(&size.to_be_bytes());

        let mut metadata = Metadata::new(FileInfo::new(
            "profile.jpg".into(),
            bytes.len() as u64,
            FileFormat::Jpeg,
        ));
        parse_icc_profile(&bytes, 0, &mut metadata, ParseLimits::default())
            .expect("ICC fixture should parse");
        assert_eq!(
            metadata.find("ICC:Description").unwrap().display_value(),
            "Metra"
        );
        assert_eq!(
            metadata.find("ICC:Description").unwrap().id,
            Some(u32::from_be_bytes(*b"desc"))
        );
        assert_eq!(
            metadata.find("ICC:DeviceClass").unwrap().display_value(),
            "mntr"
        );
        assert_eq!(
            metadata.find("ICC:CreationDate").unwrap().display_value(),
            "2026-09-13 12:34:56"
        );
        assert_eq!(
            metadata
                .find("ICC:RenderingIntent")
                .unwrap()
                .display_value(),
            "1"
        );
        assert!(matches!(
            metadata.find("ICC:Illuminant").unwrap().value,
            TagValue::Array(_)
        ));
        assert!(matches!(
            metadata.find("ICC:ProfileID").unwrap().value,
            TagValue::Bytes(_)
        ));

        let standalone = read_icc(
            &mut Cursor::new(bytes.clone()),
            FileInfo::new("profile.icc".into(), bytes.len() as u64, FileFormat::Icc),
            ParseLimits::default(),
        )
        .expect("standalone ICC fixture should parse");
        assert_eq!(
            standalone.file_info.format,
            FileFormat::Icc,
            "standalone readers should retain their detected format"
        );
        assert_eq!(
            standalone.find("ICC:Description").unwrap().display_value(),
            "Metra"
        );
    }

    #[test]
    fn reads_common_icc_text_and_xyz_table_values() {
        let mut text = b"text".to_vec();
        text.extend_from_slice(&[0; 4]);
        text.extend_from_slice(b"Copyright Metra\0");
        let (name, value, value_type) = parse_table_tag("cprt", &text).expect("text tag");
        assert_eq!(name, "Copyright");
        assert_eq!(value, TagValue::String("Copyright Metra".to_owned()));
        assert_eq!(value_type, ValueType::String);

        let mut xyz = b"XYZ ".to_vec();
        xyz.extend_from_slice(&[0; 4]);
        xyz.extend_from_slice(&(65_536_i32).to_be_bytes());
        xyz.extend_from_slice(&(32_768_i32).to_be_bytes());
        xyz.extend_from_slice(&(-65_536_i32).to_be_bytes());
        let (_, value, value_type) = parse_table_tag("wtpt", &xyz).expect("XYZ tag");
        assert_eq!(value_type, ValueType::Array);
        assert_eq!(value.to_display_string(), "1, 0.5, -1");
    }
}
