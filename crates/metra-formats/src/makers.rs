use metra_core::{
    Metadata, ParseLimits, Source, Tag, TagValue, ValueType, Warning, tag_definition,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MakerNoteIdentity {
    vendor: &'static str,
    format: &'static str,
}

pub(crate) fn inspect_maker_note(
    bytes: &[u8],
    data_offset: u64,
    metadata: &mut Metadata,
    limits: ParseLimits,
) {
    let Some(identity) = identify(bytes) else {
        return;
    };
    add_tag(
        metadata,
        "Vendor",
        identity.vendor,
        data_offset,
        bytes.len() as u64,
    );
    add_tag(
        metadata,
        "Format",
        identity.format,
        data_offset,
        bytes.len() as u64,
    );
    if identity.format == "Nikon Type 2" {
        parse_nikon_type2(bytes, data_offset, metadata, limits);
    } else if identity.format == "Canon MakerNote" {
        parse_canon_makernote(bytes, data_offset, metadata, limits);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Endian {
    Little,
    Big,
}

fn parse_nikon_type2(bytes: &[u8], data_offset: u64, metadata: &mut Metadata, limits: ParseLimits) {
    let Some(tiff) = bytes.get(10..) else {
        metadata.add_warning(
            Warning::new(
                "truncated-nikon-makernote",
                "Nikon Type 2 TIFF header is missing",
            )
            .at(data_offset),
        );
        return;
    };
    if tiff.len() < 8 {
        metadata.add_warning(
            Warning::new(
                "truncated-nikon-makernote",
                "Nikon Type 2 TIFF header is truncated",
            )
            .at(data_offset + 10),
        );
        return;
    }
    let endian = match &tiff[..2] {
        b"II" => Endian::Little,
        b"MM" => Endian::Big,
        _ => {
            metadata.add_warning(
                Warning::new(
                    "invalid-nikon-makernote",
                    "Nikon Type 2 MakerNote has an invalid byte order",
                )
                .at(data_offset + 10),
            );
            return;
        }
    };
    if read_u16(tiff, 2, endian) != Some(42) {
        metadata.add_warning(
            Warning::new(
                "invalid-nikon-makernote",
                "Nikon Type 2 MakerNote does not contain TIFF magic 42",
            )
            .at(data_offset + 12),
        );
        return;
    }
    let Some(first_ifd) = read_u32(tiff, 4, endian) else {
        return;
    };
    parse_nikon_ifd(tiff, first_ifd, endian, data_offset + 10, metadata, limits);
}

fn parse_nikon_ifd(
    bytes: &[u8],
    offset: u32,
    endian: Endian,
    source_base: u64,
    metadata: &mut Metadata,
    limits: ParseLimits,
) {
    let Some(offset) = usize::try_from(offset).ok() else {
        return;
    };
    let Some(count) = read_u16(bytes, offset, endian).map(usize::from) else {
        metadata.add_warning(
            Warning::new(
                "truncated-nikon-makernote",
                "Nikon MakerNote IFD count is missing",
            )
            .at(source_base + offset as u64),
        );
        return;
    };
    let count_to_read = count.min(limits.max_ifd_entries);
    if count > count_to_read {
        metadata.add_warning(
            Warning::new(
                "nikon-makernote-entry-limit",
                format!("Nikon MakerNote declares {count} entries; reading only {count_to_read}"),
            )
            .at(source_base + offset as u64),
        );
    }
    let Some(entries_start) = offset.checked_add(2) else {
        return;
    };
    for index in 0..count_to_read {
        let Some(entry_offset) = entries_start.checked_add(index.saturating_mul(12)) else {
            return;
        };
        let Some(entry) = bytes.get(entry_offset..entry_offset.saturating_add(12)) else {
            metadata.add_warning(
                Warning::new(
                    "truncated-nikon-makernote",
                    "Nikon MakerNote entry extends beyond its payload",
                )
                .at(source_base + entry_offset as u64),
            );
            return;
        };
        parse_nikon_entry(
            entry,
            entry_offset,
            endian,
            source_base,
            bytes,
            metadata,
            limits,
        );
    }
}

fn parse_nikon_entry(
    entry: &[u8],
    entry_offset: usize,
    endian: Endian,
    source_base: u64,
    bytes: &[u8],
    metadata: &mut Metadata,
    limits: ParseLimits,
) {
    let Some(id) = read_u16(entry, 0, endian) else {
        return;
    };
    let Some(type_id) = read_u16(entry, 2, endian) else {
        return;
    };
    let Some(count) = read_u32(entry, 4, endian) else {
        return;
    };
    let Some(item_size) = type_size(type_id) else {
        return;
    };
    let Some(total_size) = usize::try_from(count)
        .ok()
        .and_then(|count| count.checked_mul(item_size))
    else {
        metadata.add_warning(
            Warning::new(
                "invalid-nikon-makernote-size",
                "Nikon MakerNote value size overflows",
            )
            .at(source_base + entry_offset as u64),
        );
        return;
    };
    if total_size > limits.max_value_bytes {
        metadata.add_warning(
            Warning::new(
                "nikon-makernote-value-limit",
                format!("Nikon MakerNote tag 0x{id:04X} exceeds the value budget"),
            )
            .at(source_base + entry_offset as u64),
        );
        return;
    }
    let (value_bytes, value_offset) = if total_size <= 4 {
        let Some(value_bytes) = entry.get(8..8 + total_size) else {
            return;
        };
        (value_bytes, entry_offset + 8)
    } else {
        let Some(value_start) =
            read_u32(entry, 8, endian).and_then(|value| usize::try_from(value).ok())
        else {
            return;
        };
        let Some(value_bytes) = bytes.get(value_start..value_start.saturating_add(total_size))
        else {
            metadata.add_warning(
                Warning::new(
                    "invalid-nikon-makernote-offset",
                    format!("Nikon MakerNote tag 0x{id:04X} is outside its payload"),
                )
                .at(source_base + value_start as u64),
            );
            return;
        };
        (value_bytes, value_start)
    };
    let definition = tag_definition("MakerNotes", u32::from(id));
    if definition.name == "Unknown" {
        return;
    }
    let Some(value) = decode_value(type_id, count, value_bytes, endian) else {
        return;
    };
    let value_type = value_type(&value);
    metadata.add_tag(Tag {
        namespace: "MakerNotes".to_owned(),
        group: "Nikon".to_owned(),
        id: Some(u32::from(id)),
        name: definition.name.to_owned(),
        description: Some(definition.description.to_owned()),
        raw_value: Some(value_bytes.to_vec()),
        value,
        value_type,
        source: Source::new(
            "EXIF/MakerNote/Nikon",
            Some(source_base + value_offset as u64),
            Some(total_size as u64),
        ),
        writable: false,
    });
}

fn parse_canon_makernote(
    bytes: &[u8],
    data_offset: u64,
    metadata: &mut Metadata,
    limits: ParseLimits,
) {
    if bytes.len() < 10 {
        metadata.add_warning(
            Warning::new(
                "truncated-canon-makernote",
                "Canon MakerNote does not contain a complete IFD count",
            )
            .at(data_offset),
        );
        return;
    }
    parse_canon_ifd(bytes, 8, data_offset, metadata, limits);
}

fn parse_canon_ifd(
    bytes: &[u8],
    offset: usize,
    data_offset: u64,
    metadata: &mut Metadata,
    limits: ParseLimits,
) {
    let Some(count) = read_u16(bytes, offset, Endian::Little).map(usize::from) else {
        metadata.add_warning(
            Warning::new(
                "truncated-canon-makernote",
                "Canon MakerNote IFD count is missing",
            )
            .at(data_offset.saturating_add(offset as u64)),
        );
        return;
    };
    let count_to_read = count.min(limits.max_ifd_entries);
    if count > count_to_read {
        metadata.add_warning(
            Warning::new(
                "canon-makernote-entry-limit",
                format!("Canon MakerNote declares {count} entries; reading only {count_to_read}"),
            )
            .at(data_offset.saturating_add(offset as u64)),
        );
    }
    let Some(entries_start) = offset.checked_add(2) else {
        return;
    };
    for index in 0..count_to_read {
        let Some(entry_offset) = entries_start.checked_add(index.saturating_mul(12)) else {
            metadata.add_warning(
                Warning::new(
                    "invalid-canon-makernote-size",
                    "Canon MakerNote IFD entry offset overflows",
                )
                .at(data_offset.saturating_add(entries_start as u64)),
            );
            return;
        };
        let Some(entry_end) = entry_offset.checked_add(12) else {
            return;
        };
        let Some(entry) = bytes.get(entry_offset..entry_end) else {
            metadata.add_warning(
                Warning::new(
                    "truncated-canon-makernote",
                    "Canon MakerNote entry extends beyond its payload",
                )
                .at(data_offset.saturating_add(entry_offset as u64)),
            );
            return;
        };
        parse_canon_entry(entry, entry_offset, data_offset, bytes, metadata, limits);
    }
}

fn parse_canon_entry(
    entry: &[u8],
    entry_offset: usize,
    data_offset: u64,
    bytes: &[u8],
    metadata: &mut Metadata,
    limits: ParseLimits,
) {
    let Some(id) = read_u16(entry, 0, Endian::Little) else {
        return;
    };
    let Some(type_id) = read_u16(entry, 2, Endian::Little) else {
        return;
    };
    let Some(count) = read_u32(entry, 4, Endian::Little) else {
        return;
    };
    let Some(item_size) = type_size(type_id) else {
        return;
    };
    let Some(total_size) = usize::try_from(count)
        .ok()
        .and_then(|count| count.checked_mul(item_size))
    else {
        metadata.add_warning(
            Warning::new(
                "invalid-canon-makernote-size",
                format!("Canon MakerNote tag 0x{id:04X} value size overflows"),
            )
            .at(data_offset.saturating_add(entry_offset as u64)),
        );
        return;
    };
    if total_size > limits.max_value_bytes {
        metadata.add_warning(
            Warning::new(
                "canon-makernote-value-limit",
                format!("Canon MakerNote tag 0x{id:04X} exceeds the value budget"),
            )
            .at(data_offset.saturating_add(entry_offset as u64)),
        );
        return;
    }
    let value_offset = if total_size <= 4 {
        entry_offset.saturating_add(8)
    } else {
        let Some(value_offset) =
            read_u32(entry, 8, Endian::Little).and_then(|value| usize::try_from(value).ok())
        else {
            return;
        };
        value_offset
    };
    let Some(value_end) = value_offset.checked_add(total_size) else {
        return;
    };
    let Some(value_bytes) = bytes.get(value_offset..value_end) else {
        metadata.add_warning(
            Warning::new(
                "invalid-canon-makernote-offset",
                format!("Canon MakerNote tag 0x{id:04X} is outside its payload"),
            )
            .at(data_offset.saturating_add(value_offset as u64)),
        );
        return;
    };
    let Some((name, description)) = canon_tag_definition(id) else {
        return;
    };
    let Some(value) = decode_value(type_id, count, value_bytes, Endian::Little) else {
        return;
    };
    metadata.add_tag(Tag {
        namespace: "MakerNotes".to_owned(),
        group: "Canon".to_owned(),
        id: Some(u32::from(id)),
        name: name.to_owned(),
        description: Some(description.to_owned()),
        raw_value: Some(value_bytes.to_vec()),
        value_type: value_type(&value),
        value,
        source: Source::new(
            "EXIF/MakerNote/Canon",
            Some(data_offset.saturating_add(value_offset as u64)),
            Some(total_size as u64),
        ),
        writable: false,
    });
}

fn canon_tag_definition(id: u16) -> Option<(&'static str, &'static str)> {
    Some(match id {
        0x0001 => ("Canon:CameraSettings", "Canon camera settings"),
        0x0002 => ("Canon:FocalLength", "Canon focal-length data"),
        0x0004 => ("Canon:FlashInfo", "Canon flash information"),
        0x0006 => ("Canon:ImageType", "Canon image type"),
        0x0007 => ("Canon:FirmwareVersion", "Canon firmware version"),
        0x0009 => ("Canon:OwnerName", "Canon owner name"),
        0x000C => ("Canon:SerialNumber", "Canon camera serial number"),
        _ => return None,
    })
}

fn read_u16(bytes: &[u8], offset: usize, endian: Endian) -> Option<u16> {
    let bytes = bytes.get(offset..offset.checked_add(2)?)?;
    Some(match endian {
        Endian::Little => u16::from_le_bytes(bytes.try_into().ok()?),
        Endian::Big => u16::from_be_bytes(bytes.try_into().ok()?),
    })
}

fn read_u32(bytes: &[u8], offset: usize, endian: Endian) -> Option<u32> {
    let bytes = bytes.get(offset..offset.checked_add(4)?)?;
    Some(match endian {
        Endian::Little => u32::from_le_bytes(bytes.try_into().ok()?),
        Endian::Big => u32::from_be_bytes(bytes.try_into().ok()?),
    })
}

fn read_u64(bytes: &[u8], offset: usize, endian: Endian) -> Option<u64> {
    let bytes = bytes.get(offset..offset.checked_add(8)?)?;
    Some(match endian {
        Endian::Little => u64::from_le_bytes(bytes.try_into().ok()?),
        Endian::Big => u64::from_be_bytes(bytes.try_into().ok()?),
    })
}

fn read_i16(bytes: &[u8], offset: usize, endian: Endian) -> Option<i16> {
    read_u16(bytes, offset, endian).map(|value| i16::from_ne_bytes(value.to_ne_bytes()))
}

fn read_i32(bytes: &[u8], offset: usize, endian: Endian) -> Option<i32> {
    read_u32(bytes, offset, endian).map(|value| i32::from_ne_bytes(value.to_ne_bytes()))
}

fn type_size(type_id: u16) -> Option<usize> {
    match type_id {
        1 | 2 | 6 | 7 => Some(1),
        3 | 8 => Some(2),
        4 | 9 | 11 | 13 => Some(4),
        5 | 10 | 12 => Some(8),
        _ => None,
    }
}

fn decode_value(type_id: u16, count: u32, bytes: &[u8], endian: Endian) -> Option<TagValue> {
    if type_id == 2 {
        return Some(TagValue::String(
            String::from_utf8_lossy(bytes)
                .trim_end_matches('\0')
                .to_owned(),
        ));
    }
    if type_id == 7 {
        return Some(TagValue::Bytes(bytes.to_vec()));
    }
    let values = match type_id {
        1 => Some(
            bytes
                .iter()
                .map(|byte| TagValue::Unsigned(u64::from(*byte)))
                .collect::<Vec<_>>(),
        ),
        3 => bytes
            .chunks_exact(2)
            .map(|chunk| {
                read_u16(chunk, 0, endian).map(|value| TagValue::Unsigned(u64::from(value)))
            })
            .collect::<Option<Vec<_>>>(),
        4 | 13 => bytes
            .chunks_exact(4)
            .map(|chunk| {
                read_u32(chunk, 0, endian).map(|value| TagValue::Unsigned(u64::from(value)))
            })
            .collect::<Option<Vec<_>>>(),
        5 => bytes
            .chunks_exact(8)
            .map(|chunk| {
                Some(TagValue::UnsignedRational {
                    numerator: u64::from(read_u32(chunk, 0, endian)?),
                    denominator: u64::from(read_u32(chunk, 4, endian)?),
                })
            })
            .collect::<Option<Vec<_>>>(),
        6 => Some(
            bytes
                .iter()
                .map(|byte| TagValue::Signed(i64::from(i8::from_ne_bytes([*byte]))))
                .collect::<Vec<_>>(),
        ),
        8 => bytes
            .chunks_exact(2)
            .map(|chunk| read_i16(chunk, 0, endian).map(|value| TagValue::Signed(i64::from(value))))
            .collect::<Option<Vec<_>>>(),
        9 => bytes
            .chunks_exact(4)
            .map(|chunk| read_i32(chunk, 0, endian).map(|value| TagValue::Signed(i64::from(value))))
            .collect::<Option<Vec<_>>>(),
        10 => bytes
            .chunks_exact(8)
            .map(|chunk| {
                Some(TagValue::Rational {
                    numerator: i64::from(read_i32(chunk, 0, endian)?),
                    denominator: i64::from(read_i32(chunk, 4, endian)?),
                })
            })
            .collect::<Option<Vec<_>>>(),
        11 => bytes
            .chunks_exact(4)
            .map(|chunk| {
                read_u32(chunk, 0, endian)
                    .map(|value| TagValue::Float(f32::from_bits(value) as f64))
            })
            .collect::<Option<Vec<_>>>(),
        12 => bytes
            .chunks_exact(8)
            .map(|chunk| {
                read_u64(chunk, 0, endian).map(|value| TagValue::Float(f64::from_bits(value)))
            })
            .collect::<Option<Vec<_>>>(),
        _ => None,
    }?;
    if values
        .iter()
        .any(|value| matches!(value, TagValue::Float(number) if !number.is_finite()))
    {
        return Some(TagValue::Unknown {
            type_id,
            bytes: bytes.to_vec(),
        });
    }
    (values.len() == usize::try_from(count).ok()?).then(|| {
        if values.len() == 1 {
            values.into_iter().next().expect("length checked")
        } else {
            TagValue::Array(values)
        }
    })
}

fn value_type(value: &TagValue) -> ValueType {
    match value {
        TagValue::String(_) => ValueType::String,
        TagValue::Unsigned(_) => ValueType::UnsignedInteger,
        TagValue::Signed(_) => ValueType::SignedInteger,
        TagValue::Float(_) => ValueType::Float,
        TagValue::Rational { .. } => ValueType::Rational,
        TagValue::UnsignedRational { .. } => ValueType::UnsignedRational,
        TagValue::Bytes(_) => ValueType::Bytes,
        TagValue::Array(_) => ValueType::Array,
        TagValue::Structure(_) => ValueType::Structure,
        TagValue::Unknown { .. } => ValueType::Unknown,
    }
}

fn identify(bytes: &[u8]) -> Option<MakerNoteIdentity> {
    if bytes.starts_with(b"Nikon\0") {
        let format = match bytes.get(6..10) {
            Some([2, 0, 0, 0]) => "Nikon Type 2",
            Some([1, 0, 0, 0]) => "Nikon Type 1",
            _ => "Nikon MakerNote",
        };
        return Some(MakerNoteIdentity {
            vendor: "Nikon",
            format,
        });
    }
    if bytes.starts_with(b"Canon\0") {
        return Some(MakerNoteIdentity {
            vendor: "Canon",
            format: "Canon MakerNote",
        });
    }
    if bytes.starts_with(b"FUJIFILM") {
        return Some(MakerNoteIdentity {
            vendor: "Fujifilm",
            format: "Fujifilm MakerNote",
        });
    }
    if bytes.starts_with(b"SONY DSC ") {
        return Some(MakerNoteIdentity {
            vendor: "Sony",
            format: "Sony MakerNote",
        });
    }
    if bytes.starts_with(b"Panasonic") {
        return Some(MakerNoteIdentity {
            vendor: "Panasonic",
            format: "Panasonic MakerNote",
        });
    }
    if bytes.starts_with(b"OLYMP") {
        return Some(MakerNoteIdentity {
            vendor: "Olympus",
            format: "Olympus MakerNote",
        });
    }
    None
}

fn add_tag(metadata: &mut Metadata, name: &str, value: &str, offset: u64, length: u64) {
    metadata.add_tag(Tag {
        namespace: "MakerNotes".to_owned(),
        group: "Detection".to_owned(),
        id: None,
        name: name.to_owned(),
        description: Some("Detected MakerNote vendor or container format".to_owned()),
        raw_value: None,
        value: TagValue::String(value.to_owned()),
        value_type: ValueType::String,
        source: Source::new("EXIF/MakerNote", Some(offset), Some(length)),
        writable: false,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use metra_core::{FileFormat, FileInfo};

    #[test]
    fn identifies_known_maker_note_headers_without_decoding_proprietary_tags() {
        let mut metadata =
            Metadata::new(FileInfo::new("maker-note.jpg".into(), 16, FileFormat::Jpeg));
        inspect_maker_note(
            b"Nikon\0\x02\0\0\0opaque",
            100,
            &mut metadata,
            ParseLimits::default(),
        );
        assert_eq!(
            metadata.find("MakerNotes:Vendor").unwrap().display_value(),
            "Nikon"
        );
        assert_eq!(
            metadata.find("MakerNotes:Format").unwrap().display_value(),
            "Nikon Type 2"
        );
        assert_eq!(
            metadata.find("MakerNotes:Vendor").unwrap().source.offset,
            Some(100)
        );
    }

    #[test]
    fn ignores_unknown_maker_note_payloads() {
        let mut metadata =
            Metadata::new(FileInfo::new("maker-note.jpg".into(), 4, FileFormat::Jpeg));
        inspect_maker_note(b"opaque", 0, &mut metadata, ParseLimits::default());
        assert!(metadata.tags.is_empty());
    }

    #[test]
    fn reads_bounded_nikon_type_two_ifd_values() {
        let mut tiff = vec![b'I', b'I', 42, 0, 8, 0, 0, 0, 3, 0];
        tiff.extend_from_slice(&[1, 0, 2, 0, 8, 0, 0, 0]);
        tiff.extend_from_slice(&50_u32.to_le_bytes());
        tiff.extend_from_slice(&[2, 0, 3, 0, 1, 0, 0, 0, 100, 0, 0, 0]);
        tiff.extend_from_slice(&[0x0B, 0, 8, 0, 1, 0, 0, 0, 0xFE, 0xFF, 0, 0]);
        tiff.extend_from_slice(&[0, 0, 0, 0]);
        tiff.resize(50, 0);
        tiff.extend_from_slice(b"v1.0\0\0\0\0");
        let mut maker_note = b"Nikon\0\x02\0\0\0".to_vec();
        maker_note.extend_from_slice(&tiff);
        let mut metadata = Metadata::new(FileInfo::new(
            "nikon.jpg".into(),
            maker_note.len() as u64,
            FileFormat::Jpeg,
        ));
        inspect_maker_note(&maker_note, 200, &mut metadata, ParseLimits::default());
        assert_eq!(
            metadata.find("MakerNotes:Nikon:Version").unwrap().value,
            TagValue::String("v1.0".to_owned())
        );
        assert_eq!(
            metadata.find("MakerNotes:Nikon:ISO").unwrap().value,
            TagValue::Unsigned(100)
        );
        assert_eq!(metadata.find("MakerNotes:Nikon:ISO").unwrap().id, Some(2));
        assert_eq!(
            metadata
                .find("MakerNotes:Nikon:WhiteBalanceFineTune")
                .unwrap()
                .value,
            TagValue::Signed(-2)
        );
    }

    #[test]
    fn reads_bounded_canon_ifd_string_values() {
        let mut maker_note = b"Canon\0\0\0".to_vec();
        maker_note.extend_from_slice(&2_u16.to_le_bytes());
        maker_note.extend_from_slice(&[6, 0, 2, 0]);
        maker_note.extend_from_slice(&9_u32.to_le_bytes());
        maker_note.extend_from_slice(&38_u32.to_le_bytes());
        maker_note.extend_from_slice(&[9, 0, 2, 0]);
        maker_note.extend_from_slice(&6_u32.to_le_bytes());
        maker_note.extend_from_slice(&47_u32.to_le_bytes());
        maker_note.extend_from_slice(&[0, 0, 0, 0]);
        maker_note.extend_from_slice(b"IMG_0001\0");
        maker_note.extend_from_slice(b"Alice\0");

        let mut metadata = Metadata::new(FileInfo::new(
            "canon.jpg".into(),
            maker_note.len() as u64,
            FileFormat::Jpeg,
        ));
        inspect_maker_note(&maker_note, 400, &mut metadata, ParseLimits::default());

        assert_eq!(
            metadata.find("MakerNotes:Canon:ImageType").unwrap().value,
            TagValue::String("IMG_0001".to_owned())
        );
        assert_eq!(
            metadata.find("MakerNotes:Canon:OwnerName").unwrap().value,
            TagValue::String("Alice".to_owned())
        );
        assert_eq!(
            metadata
                .find("MakerNotes:Canon:OwnerName")
                .unwrap()
                .source
                .offset,
            Some(447)
        );
    }
}
