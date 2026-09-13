use metra_core::{
    Metadata, MetraError, ParseLimits, Result, Source, Tag, TagValue, ValueType, Warning,
};

const PHOTOSHOP_PREFIX: &[u8] = b"Photoshop 3.0\0";

pub(crate) fn parse_photoshop_resources(
    bytes: &[u8],
    data_offset: u64,
    metadata: &mut Metadata,
    limits: ParseLimits,
) -> Result<()> {
    if !bytes.starts_with(PHOTOSHOP_PREFIX) {
        return Err(MetraError::InvalidHeader {
            context: "Photoshop resource block".to_owned(),
            message: "missing Photoshop 3.0 prefix".to_owned(),
        });
    }
    let mut cursor = PHOTOSHOP_PREFIX.len();
    let mut blocks = 0_usize;
    while cursor < bytes.len() {
        if blocks >= limits.max_jpeg_segments {
            metadata.add_warning(
                Warning::new(
                    "photoshop-resource-limit",
                    format!(
                        "stopped after {} Photoshop resources",
                        limits.max_jpeg_segments
                    ),
                )
                .at(data_offset + cursor as u64),
            );
            break;
        }
        if bytes.len().saturating_sub(cursor) < 8 {
            metadata.add_warning(
                Warning::new(
                    "truncated-photoshop",
                    "Photoshop resource header is truncated",
                )
                .at(data_offset + cursor as u64),
            );
            break;
        }
        if &bytes[cursor..cursor + 4] != b"8BIM" {
            metadata.add_warning(
                Warning::new(
                    "invalid-photoshop-signature",
                    "expected 8BIM resource signature",
                )
                .at(data_offset + cursor as u64),
            );
            break;
        }
        let resource_id = u16::from_be_bytes([bytes[cursor + 4], bytes[cursor + 5]]);
        let name_length = usize::from(bytes[cursor + 6]);
        let name_start = cursor + 7;
        let name_end = name_start
            .checked_add(name_length)
            .ok_or(MetraError::InvalidOffset {
                context: "Photoshop resource name".to_owned(),
                offset: name_start as u64,
            })?;
        let padded_name_end = name_end + (name_end - name_start + 1) % 2;
        let size_start = padded_name_end;
        let size_end = size_start.checked_add(4).ok_or(MetraError::InvalidOffset {
            context: "Photoshop resource size".to_owned(),
            offset: size_start as u64,
        })?;
        if size_end > bytes.len() {
            metadata.add_warning(
                Warning::new(
                    "truncated-photoshop",
                    "Photoshop resource size is truncated",
                )
                .at(data_offset + cursor as u64),
            );
            break;
        }
        let resource_length = u64::from(u32::from_be_bytes(
            bytes[size_start..size_end].try_into().expect("size"),
        ));
        let resource_start = size_end;
        let resource_end = resource_start
            .checked_add(usize::try_from(resource_length).unwrap_or(usize::MAX))
            .ok_or(MetraError::InvalidOffset {
                context: format!("Photoshop resource 0x{resource_id:04X}"),
                offset: resource_length,
            })?;
        if resource_end > bytes.len() {
            metadata.add_warning(
                Warning::new(
                    "truncated-photoshop",
                    format!("resource 0x{resource_id:04X} extends beyond APP13"),
                )
                .at(data_offset + resource_start as u64),
            );
            break;
        }
        let resource = &bytes[resource_start..resource_end];
        if resource_length > limits.max_value_bytes as u64 {
            metadata.add_warning(
                Warning::new(
                    "photoshop-resource-value-limit",
                    format!("resource 0x{resource_id:04X} is too large to materialize"),
                )
                .at(data_offset + resource_start as u64),
            );
        } else if resource_id == 0x0404 {
            parse_iptc_iim(
                resource,
                data_offset + resource_start as u64,
                metadata,
                limits,
            )?;
        }
        let padded_resource_end = resource_end + resource.len() % 2;
        if padded_resource_end > bytes.len() {
            metadata.add_warning(
                Warning::new("truncated-photoshop", "resource padding is truncated")
                    .at(data_offset + resource_end as u64),
            );
            break;
        }
        cursor = padded_resource_end;
        blocks += 1;
    }
    Ok(())
}

fn parse_iptc_iim(
    bytes: &[u8],
    data_offset: u64,
    metadata: &mut Metadata,
    limits: ParseLimits,
) -> Result<()> {
    let mut cursor = 0_usize;
    let mut datasets = 0_usize;
    while cursor < bytes.len() {
        if datasets >= limits.max_ifd_entries {
            metadata.add_warning(
                Warning::new("iptc-dataset-limit", "IPTC dataset limit reached")
                    .at(data_offset + cursor as u64),
            );
            break;
        }
        if bytes.len().saturating_sub(cursor) < 5 {
            metadata.add_warning(
                Warning::new("truncated-iptc", "IPTC dataset header is truncated")
                    .at(data_offset + cursor as u64),
            );
            break;
        }
        if bytes[cursor] != 0x1C {
            metadata.add_warning(
                Warning::new("invalid-iptc-marker", "expected IPTC dataset marker 0x1C")
                    .at(data_offset + cursor as u64),
            );
            break;
        }
        let record = bytes[cursor + 1];
        let dataset = bytes[cursor + 2];
        let length_marker = u16::from_be_bytes([bytes[cursor + 3], bytes[cursor + 4]]);
        cursor += 5;
        let value_length = if length_marker & 0x8000 == 0 {
            usize::from(length_marker)
        } else {
            let byte_count = usize::from(length_marker & 0x7FFF);
            if byte_count == 0 || byte_count > 8 || bytes.len().saturating_sub(cursor) < byte_count
            {
                metadata.add_warning(
                    Warning::new("invalid-iptc-length", "invalid extended IPTC length")
                        .at(data_offset + cursor as u64),
                );
                break;
            }
            let mut length = 0_usize;
            for byte in &bytes[cursor..cursor + byte_count] {
                length = length
                    .checked_mul(256)
                    .and_then(|value| value.checked_add(usize::from(*byte)))
                    .ok_or(MetraError::InvalidOffset {
                        context: "IPTC extended length".to_owned(),
                        offset: length as u64,
                    })?;
            }
            cursor += byte_count;
            length
        };
        let end = cursor
            .checked_add(value_length)
            .ok_or(MetraError::InvalidOffset {
                context: "IPTC dataset".to_owned(),
                offset: value_length as u64,
            })?;
        if end > bytes.len() {
            metadata.add_warning(
                Warning::new("truncated-iptc", "IPTC dataset extends beyond its resource")
                    .at(data_offset + cursor as u64),
            );
            break;
        }
        if let Some(name) = dataset_name(record, dataset) {
            let value = String::from_utf8_lossy(&bytes[cursor..end]).into_owned();
            add_iptc_tag(
                metadata,
                name,
                value,
                &bytes[cursor..end],
                data_offset + cursor as u64,
            );
        }
        cursor = end;
        datasets += 1;
    }
    Ok(())
}

fn dataset_name(record: u8, dataset: u8) -> Option<&'static str> {
    if record != 2 {
        return None;
    }
    Some(match dataset {
        3 => "ObjectName",
        5 => "EditStatus",
        10 => "Urgency",
        15 => "Category",
        20 => "SupplementalCategories",
        25 => "Keywords",
        55 => "DateCreated",
        60 => "TimeCreated",
        80 => "Byline",
        85 => "BylineTitle",
        90 => "City",
        92 => "SubLocation",
        95 => "ProvinceState",
        101 => "CountryCode",
        102 => "Country",
        105 => "Headline",
        110 => "Credit",
        115 => "Source",
        116 => "CopyrightNotice",
        120 => "CaptionAbstract",
        122 => "WriterEditor",
        _ => return None,
    })
}

fn add_iptc_tag(metadata: &mut Metadata, name: &str, value: String, raw_value: &[u8], offset: u64) {
    if let Some(existing) = metadata
        .tags
        .iter_mut()
        .find(|tag| tag.namespace == "IPTC" && tag.name == name)
    {
        let previous = std::mem::replace(&mut existing.value, TagValue::String(String::new()));
        existing.value = match previous {
            TagValue::Array(mut values) => {
                values.push(TagValue::String(value));
                TagValue::Array(values)
            }
            previous => TagValue::Array(vec![previous, TagValue::String(value)]),
        };
        existing.value_type = ValueType::Array;
        existing.raw_value = None;
        return;
    }
    metadata.add_tag(Tag {
        namespace: "IPTC".to_owned(),
        group: "IIM".to_owned(),
        id: None,
        name: name.to_owned(),
        description: Some("IPTC-IIM dataset".to_owned()),
        raw_value: Some(raw_value.to_vec()),
        value: TagValue::String(value),
        value_type: ValueType::String,
        source: Source::new(
            "Photoshop/IRB/IPTC",
            Some(offset),
            Some(raw_value.len() as u64),
        ),
        writable: false,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use metra_core::{FileFormat, FileInfo};

    fn dataset(number: u8, value: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0x1C, 2, number];
        bytes.extend_from_slice(&(value.len() as u16).to_be_bytes());
        bytes.extend_from_slice(value);
        bytes
    }

    #[test]
    fn reads_iptc_keywords_and_caption_from_photoshop_resource() {
        let mut resource = b"Photoshop 3.0\0".to_vec();
        let mut iptc = dataset(25, b"rust");
        iptc.extend_from_slice(&dataset(25, b"metadata"));
        resource.extend_from_slice(b"8BIM");
        resource.extend_from_slice(&0x0404_u16.to_be_bytes());
        resource.push(0); // empty Pascal name
        resource.push(0); // Pascal name padding
        resource.extend_from_slice(&(iptc.len() as u32).to_be_bytes());
        resource.extend_from_slice(&iptc);
        if iptc.len() & 1 == 1 {
            resource.push(0);
        }
        let mut metadata = Metadata::new(FileInfo::new(
            "iptc.jpg".into(),
            resource.len() as u64,
            FileFormat::Jpeg,
        ));
        parse_photoshop_resources(&resource, 0, &mut metadata, ParseLimits::default())
            .expect("Photoshop resource should parse");
        assert!(matches!(
            metadata.find("IPTC:Keywords").unwrap().value,
            TagValue::Array(_)
        ));
    }
}
