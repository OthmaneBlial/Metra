use metra_core::{
    Metadata, MetraError, ParseLimits, Result, Source, Tag, TagValue, ValueType, Warning,
};

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
        if tag_signature == "desc" {
            if let Some(description) = parse_desc(&bytes[value_start..value_end]) {
                add_tag(
                    metadata,
                    "Description",
                    TagValue::String(description),
                    ValueType::String,
                    data_offset + value_offset as u64,
                    value_size as u64,
                );
            }
        } else if tag_signature == "mluc"
            && let Some(description) = parse_mluc(&bytes[value_start..value_end])
        {
            add_tag(
                metadata,
                "Description",
                TagValue::String(description),
                ValueType::String,
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
    metadata.add_tag(Tag {
        namespace: "ICC".to_owned(),
        group: "Profile".to_owned(),
        id: None,
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
    }
}
