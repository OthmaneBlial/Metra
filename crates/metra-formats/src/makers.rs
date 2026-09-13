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
    if identity.format == "Nikon Type 1" {
        parse_nikon_type1(bytes, data_offset, metadata, limits);
    } else if identity.format == "Nikon Type 2" {
        parse_nikon_type2(bytes, data_offset, metadata, limits);
    } else if identity.format == "Canon MakerNote" {
        parse_canon_makernote(bytes, data_offset, metadata, limits);
    } else if identity.format == "Sony MakerNote" {
        parse_sony_makernote(bytes, data_offset, metadata, limits);
    } else if identity.format == "Fujifilm MakerNote" {
        parse_fujifilm_makernote(bytes, data_offset, metadata, limits);
    } else if identity.format == "Panasonic MakerNote" {
        parse_panasonic_makernote(bytes, data_offset, metadata, limits);
    } else if identity.format == "Olympus MakerNote" {
        parse_olympus_makernote(bytes, data_offset, metadata, limits);
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

fn parse_nikon_type1(bytes: &[u8], data_offset: u64, metadata: &mut Metadata, limits: ParseLimits) {
    parse_vendor_little_ifd(
        bytes,
        8,
        data_offset,
        metadata,
        limits,
        VendorIfdConfig {
            group: "Nikon",
            name_prefix: "Nikon:",
            source: "EXIF/MakerNote/NikonType1",
            warning_prefix: "nikon-type1-makernote",
            unknown_description: "Unknown Nikon Type 1 MakerNote tag",
            definition: nikon_type1_tag_definition,
        },
    );
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
    let Some(value) = decode_value(type_id, count, value_bytes, endian) else {
        return;
    };
    let definition = tag_definition("MakerNotes", u32::from(id));
    let (name, description) = if definition.name == "Unknown" {
        (
            format!("Nikon:Tag0x{id:04X}"),
            "Unknown Nikon MakerNote tag".to_owned(),
        )
    } else {
        (
            definition.name.to_owned(),
            definition.description.to_owned(),
        )
    };
    let value_type = value_type(&value);
    metadata.add_tag(Tag {
        namespace: "MakerNotes".to_owned(),
        group: "Nikon".to_owned(),
        id: Some(u32::from(id)),
        name,
        description: Some(description),
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
    let Some(value) = decode_value(type_id, count, value_bytes, Endian::Little) else {
        return;
    };
    let (name, description) = canon_tag_definition(id)
        .map(|(name, description)| (name.to_owned(), description.to_owned()))
        .unwrap_or_else(|| {
            (
                format!("Canon:Tag0x{id:04X}"),
                "Unknown Canon MakerNote tag".to_owned(),
            )
        });
    metadata.add_tag(Tag {
        namespace: "MakerNotes".to_owned(),
        group: "Canon".to_owned(),
        id: Some(u32::from(id)),
        name,
        description: Some(description),
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

fn parse_sony_makernote(
    bytes: &[u8],
    data_offset: u64,
    metadata: &mut Metadata,
    limits: ParseLimits,
) {
    // The legacy "SONY DSC " layout keeps three reserved bytes before a
    // little-endian IFD. Value offsets are relative to the MakerNote start.
    if bytes.len() < 14 {
        metadata.add_warning(
            Warning::new(
                "truncated-sony-makernote",
                "Sony MakerNote does not contain a complete IFD count",
            )
            .at(data_offset),
        );
        return;
    }
    parse_sony_ifd(bytes, 12, data_offset, metadata, limits);
}

fn parse_sony_ifd(
    bytes: &[u8],
    offset: usize,
    data_offset: u64,
    metadata: &mut Metadata,
    limits: ParseLimits,
) {
    let Some(count) = read_u16(bytes, offset, Endian::Little).map(usize::from) else {
        metadata.add_warning(
            Warning::new(
                "truncated-sony-makernote",
                "Sony MakerNote IFD count is missing",
            )
            .at(data_offset.saturating_add(offset as u64)),
        );
        return;
    };
    let count_to_read = count.min(limits.max_ifd_entries);
    if count > count_to_read {
        metadata.add_warning(
            Warning::new(
                "sony-makernote-entry-limit",
                format!("Sony MakerNote declares {count} entries; reading only {count_to_read}"),
            )
            .at(data_offset.saturating_add(offset as u64)),
        );
    }
    let Some(entries_start) = offset.checked_add(2) else {
        return;
    };
    for index in 0..count_to_read {
        let Some(entry_offset) = index
            .checked_mul(12)
            .and_then(|delta| entries_start.checked_add(delta))
        else {
            metadata.add_warning(
                Warning::new(
                    "invalid-sony-makernote-size",
                    "Sony MakerNote IFD entry offset overflows",
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
                    "truncated-sony-makernote",
                    "Sony MakerNote entry extends beyond its payload",
                )
                .at(data_offset.saturating_add(entry_offset as u64)),
            );
            return;
        };
        parse_sony_entry(entry, entry_offset, data_offset, bytes, metadata, limits);
    }
}

fn parse_sony_entry(
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
                "invalid-sony-makernote-size",
                format!("Sony MakerNote tag 0x{id:04X} value size overflows"),
            )
            .at(data_offset.saturating_add(entry_offset as u64)),
        );
        return;
    };
    if total_size > limits.max_value_bytes {
        metadata.add_warning(
            Warning::new(
                "sony-makernote-value-limit",
                format!("Sony MakerNote tag 0x{id:04X} exceeds the value budget"),
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
                "invalid-sony-makernote-offset",
                format!("Sony MakerNote tag 0x{id:04X} is outside its payload"),
            )
            .at(data_offset.saturating_add(value_offset as u64)),
        );
        return;
    };
    let Some(value) = decode_value(type_id, count, value_bytes, Endian::Little) else {
        return;
    };
    let definition = tag_definition("MakerNotes", u32::from(id));
    let (name, description) = if definition.name.starts_with("Sony:") {
        (
            definition.name.to_owned(),
            definition.description.to_owned(),
        )
    } else {
        (
            format!("Sony:Tag0x{id:04X}"),
            "Unknown Sony MakerNote tag".to_owned(),
        )
    };
    metadata.add_tag(Tag {
        namespace: "MakerNotes".to_owned(),
        group: "Sony".to_owned(),
        id: Some(u32::from(id)),
        name,
        description: Some(description),
        raw_value: Some(value_bytes.to_vec()),
        value_type: value_type(&value),
        value,
        source: Source::new(
            "EXIF/MakerNote/Sony",
            Some(data_offset.saturating_add(value_offset as u64)),
            Some(total_size as u64),
        ),
        writable: false,
    });
}

fn parse_fujifilm_makernote(
    bytes: &[u8],
    data_offset: u64,
    metadata: &mut Metadata,
    limits: ParseLimits,
) {
    // Fujifilm replaces the TIFF byte-order/magic header with an 8-byte
    // signature and stores the first IFD offset at byte 8.
    let Some(first_ifd) = read_u32(bytes, 8, Endian::Little) else {
        metadata.add_warning(
            Warning::new(
                "truncated-fujifilm-makernote",
                "Fujifilm MakerNote IFD offset is missing",
            )
            .at(data_offset.saturating_add(8)),
        );
        return;
    };
    let Some(first_ifd) = usize::try_from(first_ifd).ok() else {
        return;
    };
    parse_fujifilm_ifd(bytes, first_ifd, data_offset, metadata, limits);
}

fn parse_fujifilm_ifd(
    bytes: &[u8],
    offset: usize,
    data_offset: u64,
    metadata: &mut Metadata,
    limits: ParseLimits,
) {
    let Some(count) = read_u16(bytes, offset, Endian::Little).map(usize::from) else {
        metadata.add_warning(
            Warning::new(
                "truncated-fujifilm-makernote",
                "Fujifilm MakerNote IFD count is missing",
            )
            .at(data_offset.saturating_add(offset as u64)),
        );
        return;
    };
    let count_to_read = count.min(limits.max_ifd_entries);
    if count > count_to_read {
        metadata.add_warning(
            Warning::new(
                "fujifilm-makernote-entry-limit",
                format!(
                    "Fujifilm MakerNote declares {count} entries; reading only {count_to_read}"
                ),
            )
            .at(data_offset.saturating_add(offset as u64)),
        );
    }
    let Some(entries_start) = offset.checked_add(2) else {
        return;
    };
    for index in 0..count_to_read {
        let Some(entry_offset) = index
            .checked_mul(12)
            .and_then(|delta| entries_start.checked_add(delta))
        else {
            metadata.add_warning(
                Warning::new(
                    "invalid-fujifilm-makernote-size",
                    "Fujifilm MakerNote IFD entry offset overflows",
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
                    "truncated-fujifilm-makernote",
                    "Fujifilm MakerNote entry extends beyond its payload",
                )
                .at(data_offset.saturating_add(entry_offset as u64)),
            );
            return;
        };
        parse_fujifilm_entry(entry, entry_offset, data_offset, bytes, metadata, limits);
    }
}

fn parse_fujifilm_entry(
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
                "invalid-fujifilm-makernote-size",
                format!("Fujifilm MakerNote tag 0x{id:04X} value size overflows"),
            )
            .at(data_offset.saturating_add(entry_offset as u64)),
        );
        return;
    };
    if total_size > limits.max_value_bytes {
        metadata.add_warning(
            Warning::new(
                "fujifilm-makernote-value-limit",
                format!("Fujifilm MakerNote tag 0x{id:04X} exceeds the value budget"),
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
                "invalid-fujifilm-makernote-offset",
                format!("Fujifilm MakerNote tag 0x{id:04X} is outside its payload"),
            )
            .at(data_offset.saturating_add(value_offset as u64)),
        );
        return;
    };
    let Some(value) = decode_value(type_id, count, value_bytes, Endian::Little) else {
        return;
    };
    let definition = tag_definition("MakerNotes", u32::from(id));
    let (name, description) = if definition.name.starts_with("FujiFilm:") {
        (
            definition.name.to_owned(),
            definition.description.to_owned(),
        )
    } else {
        (
            format!("FujiFilm:Tag0x{id:04X}"),
            "Unknown Fujifilm MakerNote tag".to_owned(),
        )
    };
    metadata.add_tag(Tag {
        namespace: "MakerNotes".to_owned(),
        group: "Fujifilm".to_owned(),
        id: Some(u32::from(id)),
        name,
        description: Some(description),
        raw_value: Some(value_bytes.to_vec()),
        value_type: value_type(&value),
        value,
        source: Source::new(
            "EXIF/MakerNote/Fujifilm",
            Some(data_offset.saturating_add(value_offset as u64)),
            Some(total_size as u64),
        ),
        writable: false,
    });
}

#[derive(Clone, Copy)]
struct VendorIfdConfig {
    group: &'static str,
    name_prefix: &'static str,
    source: &'static str,
    warning_prefix: &'static str,
    unknown_description: &'static str,
    definition: fn(u16) -> Option<(&'static str, &'static str)>,
}

fn parse_panasonic_makernote(
    bytes: &[u8],
    data_offset: u64,
    metadata: &mut Metadata,
    limits: ParseLimits,
) {
    if bytes.len() < 14 {
        metadata.add_warning(
            Warning::new(
                "truncated-panasonic-makernote",
                "Panasonic MakerNote does not contain a complete IFD count",
            )
            .at(data_offset),
        );
        return;
    }
    parse_vendor_little_ifd(
        bytes,
        12,
        data_offset,
        metadata,
        limits,
        VendorIfdConfig {
            group: "Panasonic",
            name_prefix: "Panasonic:",
            source: "EXIF/MakerNote/Panasonic",
            warning_prefix: "panasonic-makernote",
            unknown_description: "Unknown Panasonic MakerNote tag",
            definition: panasonic_tag_definition,
        },
    );
}

fn parse_olympus_makernote(
    bytes: &[u8],
    data_offset: u64,
    metadata: &mut Metadata,
    limits: ParseLimits,
) {
    let ifd_offset = if bytes.get(8..10) == Some(b"II")
        && read_u16(bytes, 10, Endian::Little) == Some(3)
    {
        Some(12)
    } else if bytes.get(0..6) == Some(b"OLYMP\0") && read_u16(bytes, 6, Endian::Little) == Some(2) {
        Some(8)
    } else {
        None
    };
    let Some(ifd_offset) = ifd_offset else {
        return;
    };
    parse_vendor_little_ifd(
        bytes,
        ifd_offset,
        data_offset,
        metadata,
        limits,
        VendorIfdConfig {
            group: "Olympus",
            name_prefix: "Olympus:",
            source: "EXIF/MakerNote/Olympus",
            warning_prefix: "olympus-makernote",
            unknown_description: "Unknown Olympus MakerNote tag",
            definition: olympus_tag_definition,
        },
    );
}

fn parse_vendor_little_ifd(
    bytes: &[u8],
    offset: usize,
    data_offset: u64,
    metadata: &mut Metadata,
    limits: ParseLimits,
    config: VendorIfdConfig,
) {
    let Some(count) = read_u16(bytes, offset, Endian::Little).map(usize::from) else {
        metadata.add_warning(
            Warning::new(
                format!("truncated-{}", config.warning_prefix),
                format!("{} MakerNote IFD count is missing", config.group),
            )
            .at(data_offset.saturating_add(offset as u64)),
        );
        return;
    };
    let count_to_read = count.min(limits.max_ifd_entries);
    if count > count_to_read {
        metadata.add_warning(
            Warning::new(
                format!("{}-entry-limit", config.warning_prefix),
                format!(
                    "{} MakerNote declares {count} entries; reading only {count_to_read}",
                    config.group
                ),
            )
            .at(data_offset.saturating_add(offset as u64)),
        );
    }
    let Some(entries_start) = offset.checked_add(2) else {
        return;
    };
    for index in 0..count_to_read {
        let Some(entry_offset) = index
            .checked_mul(12)
            .and_then(|delta| entries_start.checked_add(delta))
        else {
            metadata.add_warning(
                Warning::new(
                    format!("invalid-{}-size", config.warning_prefix),
                    format!("{} MakerNote IFD entry offset overflows", config.group),
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
                    format!("truncated-{}", config.warning_prefix),
                    format!(
                        "{} MakerNote entry extends beyond its payload",
                        config.group
                    ),
                )
                .at(data_offset.saturating_add(entry_offset as u64)),
            );
            return;
        };
        parse_vendor_little_entry(
            entry,
            entry_offset,
            data_offset,
            bytes,
            metadata,
            limits,
            config,
        );
    }
}

fn parse_vendor_little_entry(
    entry: &[u8],
    entry_offset: usize,
    data_offset: u64,
    bytes: &[u8],
    metadata: &mut Metadata,
    limits: ParseLimits,
    config: VendorIfdConfig,
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
                format!("invalid-{}-size", config.warning_prefix),
                format!(
                    "{} MakerNote tag 0x{id:04X} value size overflows",
                    config.group
                ),
            )
            .at(data_offset.saturating_add(entry_offset as u64)),
        );
        return;
    };
    if total_size > limits.max_value_bytes {
        metadata.add_warning(
            Warning::new(
                format!("{}-value-limit", config.warning_prefix),
                format!(
                    "{} MakerNote tag 0x{id:04X} exceeds the value budget",
                    config.group
                ),
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
                format!("invalid-{}-offset", config.warning_prefix),
                format!(
                    "{} MakerNote tag 0x{id:04X} is outside its payload",
                    config.group
                ),
            )
            .at(data_offset.saturating_add(value_offset as u64)),
        );
        return;
    };
    let Some(value) = decode_value(type_id, count, value_bytes, Endian::Little) else {
        return;
    };
    let (name, description) = (config.definition)(id)
        .map(|(name, description)| (name.to_owned(), description.to_owned()))
        .unwrap_or_else(|| {
            (
                format!("{}Tag0x{id:04X}", config.name_prefix),
                config.unknown_description.to_owned(),
            )
        });
    metadata.add_tag(Tag {
        namespace: "MakerNotes".to_owned(),
        group: config.group.to_owned(),
        id: Some(u32::from(id)),
        name,
        description: Some(description),
        raw_value: Some(value_bytes.to_vec()),
        value_type: value_type(&value),
        value,
        source: Source::new(
            config.source,
            Some(data_offset.saturating_add(value_offset as u64)),
            Some(total_size as u64),
        ),
        writable: false,
    });
}

fn panasonic_tag_definition(id: u16) -> Option<(&'static str, &'static str)> {
    Some(match id {
        0x0001 => ("Panasonic:ImageQuality", "Panasonic image quality"),
        0x0002 => ("Panasonic:FirmwareVersion", "Panasonic firmware version"),
        0x0003 => ("Panasonic:WhiteBalance", "Panasonic white balance"),
        0x0007 => ("Panasonic:FocusMode", "Panasonic focus mode"),
        0x000F => ("Panasonic:AFAreaMode", "Panasonic autofocus area mode"),
        0x001A => (
            "Panasonic:ImageStabilization",
            "Panasonic image stabilization",
        ),
        0x001C => ("Panasonic:MacroMode", "Panasonic macro mode"),
        0x001F => ("Panasonic:ShootingMode", "Panasonic shooting mode"),
        0x0020 => ("Panasonic:Audio", "Panasonic audio mode"),
        0x0021 => ("Panasonic:DataDump", "Panasonic opaque data dump"),
        0x0023 => ("Panasonic:WhiteBalanceBias", "Panasonic white-balance bias"),
        0x0024 => ("Panasonic:FlashBias", "Panasonic flash bias"),
        0x0025 => (
            "Panasonic:InternalSerialNumber",
            "Panasonic internal serial number",
        ),
        0x0026 => (
            "Panasonic:PanasonicExifVersion",
            "Panasonic MakerNote EXIF version",
        ),
        0x0027 => ("Panasonic:VideoFrameRate", "Panasonic video frame rate"),
        0x0028 => ("Panasonic:ColorEffect", "Panasonic color effect"),
        0x0029 => (
            "Panasonic:TimeSincePowerOn",
            "Panasonic time since power on",
        ),
        _ => return None,
    })
}

fn olympus_tag_definition(id: u16) -> Option<(&'static str, &'static str)> {
    Some(match id {
        0x0200 => ("Olympus:SpecialMode", "Olympus special mode"),
        0x0201 => ("Olympus:Quality", "Olympus image quality"),
        0x0202 => ("Olympus:Macro", "Olympus macro mode"),
        0x0203 => ("Olympus:BWMode", "Olympus black-and-white mode"),
        0x0204 => ("Olympus:DigitalZoom", "Olympus digital zoom"),
        0x0205 => ("Olympus:FocalPlaneDiagonal", "Olympus focal-plane diagonal"),
        0x0206 => (
            "Olympus:LensDistortionParams",
            "Olympus lens distortion parameters",
        ),
        0x0207 => ("Olympus:CameraType", "Olympus camera type"),
        0x0209 => ("Olympus:CameraID", "Olympus camera identifier"),
        0x0300 => (
            "Olympus:PreCaptureFrames",
            "Olympus pre-capture frame count",
        ),
        0x0404 => ("Olympus:SerialNumber", "Olympus serial number"),
        0x1010 => ("Olympus:FlashChargeLevel", "Olympus flash charge level"),
        0x1023 => (
            "Olympus:FlashExposureComp",
            "Olympus flash exposure compensation",
        ),
        0x1029 => ("Olympus:Contrast", "Olympus contrast"),
        0x102A => ("Olympus:SharpnessFactor", "Olympus sharpness factor"),
        0x102B => ("Olympus:ColorControl", "Olympus color control"),
        0x102C => ("Olympus:ValidBits", "Olympus valid bits"),
        0x1030 => ("Olympus:SceneDetect", "Olympus scene detection"),
        _ => return None,
    })
}

fn nikon_type1_tag_definition(id: u16) -> Option<(&'static str, &'static str)> {
    Some(match id {
        0x0003 => ("Nikon:Quality", "Nikon Type 1 image quality"),
        0x0004 => ("Nikon:ColorMode", "Nikon Type 1 color mode"),
        0x0005 => ("Nikon:ImageAdjustment", "Nikon Type 1 image adjustment"),
        0x0006 => ("Nikon:CCDSensitivity", "Nikon Type 1 CCD sensitivity"),
        0x0007 => ("Nikon:WhiteBalance", "Nikon Type 1 white balance"),
        0x0008 => ("Nikon:Focus", "Nikon Type 1 focus mode"),
        0x000A => ("Nikon:DigitalZoom", "Nikon Type 1 digital zoom"),
        0x000B => ("Nikon:Converter", "Nikon Type 1 converter"),
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
        TagValue::Date { .. } => ValueType::Date,
        TagValue::Time { .. } => ValueType::Time,
        TagValue::DateTime { .. } => ValueType::DateTime,
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
        let format = match bytes.get(6) {
            Some(2) => "Nikon Type 2",
            Some(1) => "Nikon Type 1",
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
    fn reads_bounded_nikon_type_one_ifd_values() {
        let mut maker_note = b"Nikon\0\x01\0".to_vec();
        maker_note.extend_from_slice(&2_u16.to_le_bytes());
        maker_note.extend_from_slice(&[3, 0, 2, 0]);
        maker_note.extend_from_slice(&5_u32.to_le_bytes());
        maker_note.extend_from_slice(&40_u32.to_le_bytes());
        maker_note.extend_from_slice(&[7, 0, 3, 0]);
        maker_note.extend_from_slice(&1_u32.to_le_bytes());
        maker_note.extend_from_slice(&1_u16.to_le_bytes());
        maker_note.extend_from_slice(&[0, 0]);
        maker_note.extend_from_slice(&[0, 0, 0, 0]);
        maker_note.resize(40, 0);
        maker_note.extend_from_slice(b"FINE\0");

        let mut metadata = Metadata::new(FileInfo::new(
            "nikon-type1.jpg".into(),
            maker_note.len() as u64,
            FileFormat::Jpeg,
        ));
        inspect_maker_note(&maker_note, 2_100, &mut metadata, ParseLimits::default());

        assert_eq!(
            metadata.find("MakerNotes:Nikon:Quality").unwrap().value,
            TagValue::String("FINE".to_owned())
        );
        assert_eq!(
            metadata
                .find("MakerNotes:Nikon:WhiteBalance")
                .unwrap()
                .value,
            TagValue::Unsigned(1)
        );
        assert_eq!(
            metadata
                .find("MakerNotes:Nikon:Quality")
                .unwrap()
                .source
                .offset,
            Some(2_140)
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

    #[test]
    fn reads_bounded_sony_ifd_values_and_unknown_bytes() {
        let mut maker_note = b"SONY DSC ".to_vec();
        maker_note.extend_from_slice(&[0, 0, 0]);
        maker_note.extend_from_slice(&3_u16.to_le_bytes());
        maker_note.extend_from_slice(&[0x02, 0x20, 4, 0]);
        maker_note.extend_from_slice(&1_u32.to_le_bytes());
        maker_note.extend_from_slice(&4_u32.to_le_bytes());
        maker_note.extend_from_slice(&[0x07, 0x20, 9, 0]);
        maker_note.extend_from_slice(&1_u32.to_le_bytes());
        maker_note.extend_from_slice(&(-2_i32).to_le_bytes());
        maker_note.extend_from_slice(&[0x01, 0x90, 7, 0]);
        maker_note.extend_from_slice(&3_u32.to_le_bytes());
        maker_note.extend_from_slice(&[1, 2, 3, 0]);
        maker_note.extend_from_slice(&[0, 0, 0, 0]);

        let mut metadata = Metadata::new(FileInfo::new(
            "sony.jpg".into(),
            maker_note.len() as u64,
            FileFormat::Jpeg,
        ));
        inspect_maker_note(&maker_note, 700, &mut metadata, ParseLimits::default());

        assert_eq!(
            metadata.find("MakerNotes:Sony:Rating").unwrap().value,
            TagValue::Unsigned(4)
        );
        assert_eq!(
            metadata.find("MakerNotes:Sony:Brightness").unwrap().value,
            TagValue::Signed(-2)
        );
        let unknown = metadata
            .find("MakerNotes:Sony:Tag0x9001")
            .expect("unknown Sony tag should be retained");
        assert_eq!(unknown.value, TagValue::Bytes(vec![1, 2, 3]));
        assert_eq!(unknown.raw_value, Some(vec![1, 2, 3]));
        assert_eq!(unknown.id, Some(0x9001));
    }

    #[test]
    fn reads_sony_out_of_line_values_relative_to_makernote_start() {
        let mut maker_note = b"SONY DSC ".to_vec();
        maker_note.extend_from_slice(&[0, 0, 0]);
        maker_note.extend_from_slice(&1_u16.to_le_bytes());
        maker_note.extend_from_slice(&[0x08, 0x20, 4, 0]);
        maker_note.extend_from_slice(&2_u32.to_le_bytes());
        maker_note.extend_from_slice(&60_u32.to_le_bytes());
        maker_note.extend_from_slice(&[0, 0, 0, 0]);
        maker_note.resize(60, 0);
        maker_note.extend_from_slice(&1_u32.to_le_bytes());
        maker_note.extend_from_slice(&65_537_u32.to_le_bytes());

        let mut metadata = Metadata::new(FileInfo::new(
            "sony.jpg".into(),
            maker_note.len() as u64,
            FileFormat::Jpeg,
        ));
        inspect_maker_note(&maker_note, 900, &mut metadata, ParseLimits::default());

        let tag = metadata
            .find("MakerNotes:Sony:LongExposureNoiseReduction")
            .unwrap();
        assert_eq!(
            tag.value,
            TagValue::Array(vec![TagValue::Unsigned(1), TagValue::Unsigned(65_537)])
        );
        assert_eq!(tag.source.offset, Some(960));
        assert_eq!(tag.source.length, Some(8));
    }

    #[test]
    fn reads_bounded_fujifilm_ifd_values_and_unknown_bytes() {
        let mut maker_note = b"FUJIFILM".to_vec();
        maker_note.extend_from_slice(&12_u32.to_le_bytes());
        maker_note.extend_from_slice(&3_u16.to_le_bytes());
        maker_note.extend_from_slice(&[0, 0, 7, 0]);
        maker_note.extend_from_slice(&4_u32.to_le_bytes());
        maker_note.extend_from_slice(b"0130");
        maker_note.extend_from_slice(&[0, 0x10, 2, 0]);
        maker_note.extend_from_slice(&8_u32.to_le_bytes());
        maker_note.extend_from_slice(&60_u32.to_le_bytes());
        maker_note.extend_from_slice(&[1, 0x10, 3, 0]);
        maker_note.extend_from_slice(&1_u32.to_le_bytes());
        maker_note.extend_from_slice(&3_u16.to_le_bytes());
        maker_note.extend_from_slice(&[0, 0, 0, 0]);
        maker_note.resize(60, 0);
        maker_note.extend_from_slice(b"NORMAL \0");

        let mut metadata = Metadata::new(FileInfo::new(
            "fujifilm.jpg".into(),
            maker_note.len() as u64,
            FileFormat::Jpeg,
        ));
        inspect_maker_note(&maker_note, 1_100, &mut metadata, ParseLimits::default());

        assert_eq!(
            metadata.find("MakerNotes:FujiFilm:Version").unwrap().value,
            TagValue::Bytes(b"0130".to_vec())
        );
        assert_eq!(
            metadata.find("MakerNotes:FujiFilm:Quality").unwrap().value,
            TagValue::String("NORMAL ".to_owned())
        );
        assert_eq!(
            metadata
                .find("MakerNotes:FujiFilm:Sharpness")
                .unwrap()
                .value,
            TagValue::Unsigned(3)
        );
    }

    #[test]
    fn reads_bounded_panasonic_ifd_values() {
        let mut maker_note = b"Panasonic\0\0\0".to_vec();
        maker_note.extend_from_slice(&3_u16.to_le_bytes());
        maker_note.extend_from_slice(&[1, 0, 3, 0]);
        maker_note.extend_from_slice(&1_u32.to_le_bytes());
        maker_note.extend_from_slice(&2_u16.to_le_bytes());
        maker_note.extend_from_slice(&[0, 0]);
        maker_note.extend_from_slice(&[2, 0, 7, 0]);
        maker_note.extend_from_slice(&4_u32.to_le_bytes());
        maker_note.extend_from_slice(b"0100");
        maker_note.extend_from_slice(&[0x21, 0, 7, 0]);
        maker_note.extend_from_slice(&3_u32.to_le_bytes());
        maker_note.extend_from_slice(&[1, 2, 3, 0]);
        maker_note.extend_from_slice(&[0, 0, 0, 0]);

        let mut metadata = Metadata::new(FileInfo::new(
            "panasonic.jpg".into(),
            maker_note.len() as u64,
            FileFormat::Jpeg,
        ));
        inspect_maker_note(&maker_note, 1_300, &mut metadata, ParseLimits::default());

        assert_eq!(
            metadata
                .find("MakerNotes:Panasonic:ImageQuality")
                .unwrap()
                .value,
            TagValue::Unsigned(2)
        );
        assert_eq!(
            metadata
                .find("MakerNotes:Panasonic:FirmwareVersion")
                .unwrap()
                .value,
            TagValue::Bytes(b"0100".to_vec())
        );
        let data_dump = metadata
            .find("MakerNotes:Panasonic:DataDump")
            .expect("Panasonic data dump should be retained");
        assert_eq!(data_dump.value, TagValue::Bytes(vec![1, 2, 3]));
    }

    #[test]
    fn reads_modern_and_legacy_olympus_ifd_headers() {
        let mut modern = b"OLYMPUS\0II\x03\0".to_vec();
        modern.extend_from_slice(&2_u16.to_le_bytes());
        modern.extend_from_slice(&[0, 2, 4, 0]);
        modern.extend_from_slice(&1_u32.to_le_bytes());
        modern.extend_from_slice(&1_u32.to_le_bytes());
        modern.extend_from_slice(&[1, 2, 3, 0]);
        modern.extend_from_slice(&1_u32.to_le_bytes());
        modern.extend_from_slice(&[1, 2, 3, 0]);
        modern.extend_from_slice(&[0, 0, 0, 0]);

        let mut modern_metadata = Metadata::new(FileInfo::new(
            "olympus.jpg".into(),
            modern.len() as u64,
            FileFormat::Jpeg,
        ));
        inspect_maker_note(&modern, 1_500, &mut modern_metadata, ParseLimits::default());
        assert_eq!(
            modern_metadata
                .find("MakerNotes:Olympus:SpecialMode")
                .unwrap()
                .value,
            TagValue::Unsigned(1)
        );

        let mut legacy = b"OLYMP\0\x02\0".to_vec();
        legacy.extend_from_slice(&1_u16.to_le_bytes());
        legacy.extend_from_slice(&[1, 2, 3, 0]);
        legacy.extend_from_slice(&1_u32.to_le_bytes());
        legacy.extend_from_slice(&3_u16.to_le_bytes());
        legacy.extend_from_slice(&[0, 0, 0, 0]);
        let mut legacy_metadata = Metadata::new(FileInfo::new(
            "olympus-e1.jpg".into(),
            legacy.len() as u64,
            FileFormat::Jpeg,
        ));
        inspect_maker_note(&legacy, 1_700, &mut legacy_metadata, ParseLimits::default());
        assert_eq!(
            legacy_metadata
                .find("MakerNotes:Olympus:Quality")
                .unwrap()
                .value,
            TagValue::Unsigned(3)
        );
    }

    #[test]
    fn retains_unknown_nikon_values_with_stable_fallback_names() {
        let mut tiff = vec![b'I', b'I', 42, 0, 8, 0, 0, 0, 1, 0];
        tiff.extend_from_slice(&[0x34, 0x12, 7, 0, 3, 0, 0, 0, 1, 2, 3, 0]);
        tiff.extend_from_slice(&[0, 0, 0, 0]);
        let mut maker_note = b"Nikon\0\x02\0\0\0".to_vec();
        maker_note.extend_from_slice(&tiff);

        let mut metadata = Metadata::new(FileInfo::new(
            "nikon.jpg".into(),
            maker_note.len() as u64,
            FileFormat::Jpeg,
        ));
        inspect_maker_note(&maker_note, 500, &mut metadata, ParseLimits::default());

        let tag = metadata
            .find("MakerNotes:Nikon:Tag0x1234")
            .expect("unknown Nikon tag should be retained");
        assert_eq!(tag.value, TagValue::Bytes(vec![1, 2, 3]));
        assert_eq!(tag.id, Some(0x1234));
    }
}
