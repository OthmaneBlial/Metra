use std::collections::BTreeMap;

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
    inspect_maker_note_with_context(bytes, data_offset, metadata, limits, None, None);
}

pub(crate) fn inspect_maker_note_with_context(
    bytes: &[u8],
    data_offset: u64,
    metadata: &mut Metadata,
    limits: ParseLimits,
    make: Option<&str>,
    tiff_base: Option<u64>,
) {
    let Some(identity) = identify(bytes, make) else {
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
    } else if identity.format == "Apple MakerNote" {
        parse_apple_makernote(bytes, data_offset, metadata, limits);
    } else if identity.format == "Pentax MakerNote" {
        parse_pentax_makernote(bytes, data_offset, metadata, limits, tiff_base);
    } else if identity.format == "Samsung STMN MakerNote" {
        parse_samsung_stmn(bytes, data_offset, metadata, limits);
    } else if identity.format == "DJI MakerNote" {
        parse_dji_makernote(bytes, data_offset, metadata, limits);
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
            endian: Endian::Little,
            value_offset_base: None,
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
    endian: Endian,
    value_offset_base: Option<u64>,
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
            endian: Endian::Little,
            value_offset_base: None,
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
            endian: Endian::Little,
            value_offset_base: None,
        },
    );
}

fn parse_apple_makernote(
    bytes: &[u8],
    data_offset: u64,
    metadata: &mut Metadata,
    limits: ParseLimits,
) {
    if bytes.len() < 16 {
        metadata.add_warning(
            Warning::new(
                "truncated-apple-makernote",
                "Apple MakerNote does not contain a complete IFD header",
            )
            .at(data_offset),
        );
        return;
    }
    if bytes.get(12..14) != Some(b"MM") {
        metadata.add_warning(
            Warning::new(
                "invalid-apple-makernote",
                "Apple MakerNote does not contain the expected Big Endian marker",
            )
            .at(data_offset + 12),
        );
        return;
    }
    parse_vendor_ifd(
        bytes,
        14,
        data_offset,
        metadata,
        limits,
        VendorIfdConfig {
            group: "Apple",
            name_prefix: "Apple:",
            source: "EXIF/MakerNote/Apple",
            warning_prefix: "apple-makernote",
            unknown_description: "Unknown Apple MakerNote tag",
            definition: apple_tag_definition,
            endian: Endian::Big,
            value_offset_base: None,
        },
    );
}

fn parse_pentax_makernote(
    bytes: &[u8],
    data_offset: u64,
    metadata: &mut Metadata,
    limits: ParseLimits,
    tiff_base: Option<u64>,
) {
    if bytes.len() < 8 {
        metadata.add_warning(
            Warning::new(
                "truncated-pentax-makernote",
                "Pentax MakerNote does not contain a complete Big Endian IFD header",
            )
            .at(data_offset),
        );
        return;
    }
    parse_vendor_ifd(
        bytes,
        6,
        data_offset,
        metadata,
        limits,
        VendorIfdConfig {
            group: "Pentax",
            name_prefix: "Pentax:",
            source: "EXIF/MakerNote/Pentax",
            warning_prefix: "pentax-makernote",
            unknown_description: "Unknown Pentax MakerNote tag",
            definition: pentax_tag_definition,
            endian: Endian::Big,
            value_offset_base: tiff_base,
        },
    );
}

fn parse_samsung_stmn(
    bytes: &[u8],
    data_offset: u64,
    metadata: &mut Metadata,
    limits: ParseLimits,
) {
    if bytes.len() < 8 {
        metadata.add_warning(
            Warning::new(
                "truncated-samsung-stmn",
                "Samsung STMN MakerNote version field is truncated",
            )
            .at(data_offset),
        );
        return;
    }

    let version = bytes[..8].to_vec();
    add_maker_value(
        metadata,
        MakerValueSpec {
            group: "Samsung",
            id: 0,
            name: "Samsung:MakerNoteVersion",
            description: "Samsung STMN MakerNote version bytes",
            source: "EXIF/MakerNote/Samsung/STMN",
            value: TagValue::Bytes(version.clone()),
            raw_value: version,
            data_offset,
            value_offset: 0,
        },
    );

    for (id, name, description, offset) in [
        (
            2,
            "Samsung:PreviewImageStart",
            "Samsung preview-image start offset",
            12,
        ),
        (
            3,
            "Samsung:PreviewImageLength",
            "Samsung preview-image byte length",
            16,
        ),
    ] {
        let Some(value) = read_u32(bytes, offset, Endian::Little) else {
            metadata.add_warning(
                Warning::new(
                    "truncated-samsung-stmn",
                    "Samsung STMN preview-image fields are truncated",
                )
                .at(data_offset.saturating_add(offset as u64)),
            );
            break;
        };
        let raw_value = bytes[offset..offset + 4].to_vec();
        add_maker_value(
            metadata,
            MakerValueSpec {
                group: "Samsung",
                id,
                name,
                description,
                source: "EXIF/MakerNote/Samsung/STMN",
                value: TagValue::Unsigned(u64::from(value)),
                raw_value,
                data_offset,
                value_offset: offset,
            },
        );
    }

    let Some(ifd_payload) = bytes.get(48..) else {
        return;
    };
    if ifd_payload.len() < 4 || ifd_payload[0] == 0 || ifd_payload.get(1..4) != Some(&[0, 0, 0]) {
        return;
    }
    let value_length = ifd_payload.len().min(limits.max_value_bytes);
    if value_length < ifd_payload.len() {
        metadata.add_warning(
            Warning::new(
                "samsung-stmn-value-limit",
                "Samsung STMN nested IFD payload was truncated to the value budget",
            )
            .at(data_offset.saturating_add(48)),
        );
    }
    let value = ifd_payload[..value_length].to_vec();
    add_maker_value(
        metadata,
        MakerValueSpec {
            group: "Samsung",
            id: 11,
            name: "Samsung:SamsungIFD",
            description: "Samsung STMN nested IFD payload",
            source: "EXIF/MakerNote/Samsung/STMN",
            value: TagValue::Bytes(value.clone()),
            raw_value: value,
            data_offset,
            value_offset: 48,
        },
    );
}

fn parse_dji_makernote(
    bytes: &[u8],
    data_offset: u64,
    metadata: &mut Metadata,
    limits: ParseLimits,
) {
    let Some(endian) = dji_ifd_endian(bytes, limits) else {
        metadata.add_warning(
            Warning::new(
                "invalid-dji-makernote",
                "DJI MakerNote does not contain a bounded IFD in a supported byte order",
            )
            .at(data_offset),
        );
        return;
    };
    parse_vendor_ifd(
        bytes,
        0,
        data_offset,
        metadata,
        limits,
        VendorIfdConfig {
            group: "DJI",
            name_prefix: "DJI:",
            source: "EXIF/MakerNote/DJI",
            warning_prefix: "dji-makernote",
            unknown_description: "Unknown DJI MakerNote tag",
            definition: dji_tag_definition,
            endian,
            value_offset_base: None,
        },
    );
}

fn dji_ifd_endian(bytes: &[u8], limits: ParseLimits) -> Option<Endian> {
    let little = dji_ifd_candidate(bytes, Endian::Little, limits);
    let big = dji_ifd_candidate(bytes, Endian::Big, limits);
    match (little, big) {
        (Some(little_score), Some(big_score)) => (big_score > little_score)
            .then_some(Endian::Big)
            .or(Some(Endian::Little)),
        (Some(_), None) => Some(Endian::Little),
        (None, Some(_)) => Some(Endian::Big),
        (None, None) => None,
    }
}

fn dji_ifd_candidate(bytes: &[u8], endian: Endian, limits: ParseLimits) -> Option<usize> {
    let count = usize::from(read_u16(bytes, 0, endian)?);
    let count_to_read = count.min(limits.max_ifd_entries);
    let entries_end = 2usize.checked_add(count_to_read.checked_mul(12)?)?;
    if entries_end > bytes.len() {
        return None;
    }
    let mut score = 0;
    for index in 0..count_to_read {
        let entry_offset = 2 + index * 12;
        let type_id = read_u16(bytes, entry_offset + 2, endian)?;
        if type_size(type_id).is_some() {
            score += 1;
        }
    }
    Some(score)
}

fn parse_vendor_little_ifd(
    bytes: &[u8],
    offset: usize,
    data_offset: u64,
    metadata: &mut Metadata,
    limits: ParseLimits,
    config: VendorIfdConfig,
) {
    parse_vendor_ifd(bytes, offset, data_offset, metadata, limits, config);
}

fn parse_vendor_ifd(
    bytes: &[u8],
    offset: usize,
    data_offset: u64,
    metadata: &mut Metadata,
    limits: ParseLimits,
    config: VendorIfdConfig,
) {
    let endian = config.endian;
    let Some(count) = read_u16(bytes, offset, endian).map(usize::from) else {
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
    let endian = config.endian;
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
    let (value_offset, value_source_offset) = if total_size <= 4 {
        let value_offset = entry_offset.saturating_add(8);
        (
            value_offset,
            data_offset.saturating_add(value_offset as u64),
        )
    } else {
        let Some(raw_offset) = read_u32(entry, 8, endian).map(u64::from) else {
            return;
        };
        if let Some(base) = config.value_offset_base {
            let Some(value_source_offset) = base.checked_add(raw_offset) else {
                return;
            };
            let Some(relative_offset) = value_source_offset.checked_sub(data_offset) else {
                return;
            };
            let Some(value_offset) = usize::try_from(relative_offset).ok() else {
                return;
            };
            (value_offset, value_source_offset)
        } else {
            let Some(value_offset) = usize::try_from(raw_offset).ok() else {
                return;
            };
            (
                value_offset,
                data_offset.saturating_add(value_offset as u64),
            )
        }
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
            .at(value_source_offset),
        );
        return;
    };
    let Some(decoded_value) = decode_value(type_id, count, value_bytes, endian) else {
        return;
    };
    let value = if config.group == "Apple" && id == 0x0003 {
        match &decoded_value {
            TagValue::Bytes(bytes) => parse_apple_runtime_plist(bytes, limits)
                .map(TagValue::Structure)
                .unwrap_or(decoded_value),
            _ => decoded_value,
        }
    } else {
        decoded_value
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
            Some(value_source_offset),
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

fn apple_tag_definition(id: u16) -> Option<(&'static str, &'static str)> {
    Some(match id {
        0x0001 => ("Apple:MakerNoteVersion", "Apple MakerNote version"),
        0x0002 => ("Apple:AEMatrix", "Apple auto-exposure matrix"),
        0x0003 => ("Apple:RunTime", "Apple runtime property list"),
        0x0004 => ("Apple:AEStable", "Apple auto-exposure stability"),
        0x0005 => ("Apple:AETarget", "Apple auto-exposure target"),
        0x0006 => ("Apple:AEAverage", "Apple auto-exposure average"),
        0x0007 => ("Apple:AFStable", "Apple autofocus stability"),
        0x0008 => ("Apple:AccelerationVector", "Apple acceleration vector"),
        0x000C => ("Apple:FocusDistanceRange", "Apple focus distance range"),
        0x000D => ("Apple:Apple_0x000d", "Unknown Apple MakerNote field 0x000D"),
        0x000E => ("Apple:Apple_0x000e", "Unknown Apple MakerNote field 0x000E"),
        0x000F => ("Apple:OISMode", "Apple optical image stabilization mode"),
        0x0010 => ("Apple:Apple_0x0010", "Unknown Apple MakerNote field 0x0010"),
        0x0011 => ("Apple:ContentIdentifier", "Apple content identifier"),
        0x0014 => ("Apple:ImageCaptureType", "Apple image capture type"),
        0x0015 => ("Apple:ImageUniqueID", "Apple image unique identifier"),
        0x0017 => ("Apple:LivePhotoVideoIndex", "Apple Live Photo video index"),
        0x0019 => ("Apple:ImageProcessingFlags", "Apple image processing flags"),
        0x001A => ("Apple:QualityHint", "Apple photo quality hint"),
        0x001D => (
            "Apple:LuminanceNoiseAmplitude",
            "Apple luminance noise amplitude",
        ),
        0x0020 => (
            "Apple:ImageCaptureRequestID",
            "Apple image capture request identifier",
        ),
        0x0023 => ("Apple:AFPerformance", "Apple autofocus performance"),
        0x002B => ("Apple:PhotoIdentifier", "Apple photo identifier"),
        0x002D => ("Apple:ColorTemperature", "Apple color temperature"),
        0x002E => ("Apple:CameraType", "Apple camera type"),
        0x002F => ("Apple:FocusPosition", "Apple focus position"),
        0x0030 => ("Apple:HDRGain", "Apple HDR gain"),
        0x0038 => ("Apple:AFMeasuredDepth", "Apple measured autofocus depth"),
        0x003C => ("Apple:AFConfidence", "Apple autofocus confidence"),
        _ => return None,
    })
}

fn dji_tag_definition(id: u16) -> Option<(&'static str, &'static str)> {
    Some(match id {
        0x0001 => ("DJI:Make", "DJI MakerNote manufacturer"),
        0x0002 => ("DJI:Flags", "DJI MakerNote flags"),
        0x0003 => ("DJI:SpeedX", "DJI horizontal speed X"),
        0x0004 => ("DJI:SpeedY", "DJI horizontal speed Y"),
        0x0005 => ("DJI:SpeedZ", "DJI vertical speed Z"),
        0x0006 => ("DJI:Pitch", "DJI aircraft pitch"),
        0x0007 => ("DJI:Yaw", "DJI aircraft yaw"),
        0x0008 => ("DJI:Roll", "DJI aircraft roll"),
        0x0009 => ("DJI:CameraPitch", "DJI camera pitch"),
        0x000A => ("DJI:CameraYaw", "DJI camera yaw"),
        0x000B => ("DJI:CameraRoll", "DJI camera roll"),
        _ => return None,
    })
}

fn pentax_tag_definition(id: u16) -> Option<(&'static str, &'static str)> {
    Some(match id {
        0x0000 => ("Pentax:PentaxVersion", "Pentax MakerNote version"),
        0x0001 => ("Pentax:PentaxModelType", "Pentax model type"),
        0x0002 => ("Pentax:PreviewImageSize", "Pentax preview-image dimensions"),
        0x0003 => (
            "Pentax:PreviewImageLength",
            "Pentax preview-image byte length",
        ),
        0x0004 => (
            "Pentax:PreviewImageStart",
            "Pentax preview-image start offset",
        ),
        0x0005 => ("Pentax:PentaxModelID", "Pentax model identifier"),
        0x0006 => ("Pentax:Date", "Pentax capture date bytes"),
        0x0007 => ("Pentax:Time", "Pentax capture time bytes"),
        0x0008 => ("Pentax:Quality", "Pentax image quality"),
        0x000C => ("Pentax:FlashMode", "Pentax flash mode"),
        0x000D => ("Pentax:FocusMode", "Pentax focus mode"),
        0x000E => ("Pentax:AFPointSelected", "Pentax selected autofocus point"),
        0x0012 => ("Pentax:ExposureTime", "Pentax exposure time"),
        0x0013 => ("Pentax:FNumber", "Pentax f-number"),
        0x0014 => ("Pentax:ISO", "Pentax ISO setting"),
        0x0016 => (
            "Pentax:ExposureCompensation",
            "Pentax exposure compensation",
        ),
        0x0017 => ("Pentax:MeteringMode", "Pentax metering mode"),
        0x0018 => ("Pentax:AutoBracketing", "Pentax auto-bracketing settings"),
        0x0019 => ("Pentax:WhiteBalance", "Pentax white balance"),
        0x001A => ("Pentax:WhiteBalanceMode", "Pentax white-balance mode"),
        0x001D => ("Pentax:FocalLength", "Pentax focal length"),
        0x001F => ("Pentax:Saturation", "Pentax saturation"),
        0x0020 => ("Pentax:Contrast", "Pentax contrast"),
        0x0021 => ("Pentax:Sharpness", "Pentax sharpness"),
        0x0022 => ("Pentax:WorldTimeLocation", "Pentax world-time location"),
        0x0023 => ("Pentax:HometownCity", "Pentax hometown city"),
        0x0024 => ("Pentax:DestinationCity", "Pentax destination city"),
        0x0025 => ("Pentax:HometownDST", "Pentax hometown daylight-saving flag"),
        0x0026 => (
            "Pentax:DestinationDST",
            "Pentax destination daylight-saving flag",
        ),
        0x0027 => (
            "Pentax:DSPFirmwareVersion",
            "Pentax DSP firmware version bytes",
        ),
        0x0028 => (
            "Pentax:CPUFirmwareVersion",
            "Pentax CPU firmware version bytes",
        ),
        0x002D => ("Pentax:EffectiveLV", "Pentax effective light value"),
        0x0032 => ("Pentax:ImageEditing", "Pentax image-editing flags"),
        0x0033 => ("Pentax:PictureMode", "Pentax picture mode"),
        0x0034 => ("Pentax:DriveMode", "Pentax drive mode"),
        0x0037 => ("Pentax:ColorSpace", "Pentax color space"),
        0x003D => ("Pentax:DataScaling", "Pentax data scaling"),
        0x003E => ("Pentax:PreviewImageBorders", "Pentax preview-image borders"),
        0x003F => ("Pentax:LensType", "Pentax lens type"),
        0x0040 => ("Pentax:SensitivityAdjust", "Pentax sensitivity adjustment"),
        0x0041 => ("Pentax:ImageEditCount", "Pentax image-edit count"),
        0x0042 => ("Pentax:CameraTemperature", "Pentax camera temperature"),
        0x0043 => ("Pentax:AELock", "Pentax auto-exposure lock"),
        0x0044 => ("Pentax:NoiseReduction", "Pentax noise reduction"),
        0x0045 => (
            "Pentax:FlashExposureComp",
            "Pentax flash exposure compensation",
        ),
        0x0046 => ("Pentax:ImageTone", "Pentax image tone"),
        0x0047 => ("Pentax:SRResult", "Pentax shake-reduction result"),
        0x0048 => ("Pentax:ShakeReduction", "Pentax shake-reduction mode"),
        0x0049 => (
            "Pentax:SRHalfPressTime",
            "Pentax shake-reduction half-press time",
        ),
        0x004A => (
            "Pentax:SRFocalLength",
            "Pentax shake-reduction focal length",
        ),
        0x004B => ("Pentax:ShutterCount", "Pentax shutter count"),
        0x004C => (
            "Pentax:RawDevelopmentProcess",
            "Pentax raw-development process",
        ),
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

fn parse_apple_runtime_plist(
    bytes: &[u8],
    limits: ParseLimits,
) -> Option<BTreeMap<String, TagValue>> {
    const TRAILER_SIZE: usize = 32;
    if !bytes.starts_with(b"bplist00") || bytes.len() < 8 + TRAILER_SIZE {
        return None;
    }
    let trailer_start = bytes.len().checked_sub(TRAILER_SIZE)?;
    let offset_size = usize::from(*bytes.get(trailer_start + 6)?);
    let object_ref_size = usize::from(*bytes.get(trailer_start + 7)?);
    if !matches!(offset_size, 1 | 2 | 4 | 8) || !matches!(object_ref_size, 1 | 2 | 4 | 8) {
        return None;
    }
    let object_count = usize::try_from(read_be_sized(bytes, trailer_start + 8, 8)?).ok()?;
    if object_count == 0 || object_count > limits.max_ifd_entries {
        return None;
    }
    let top_object = usize::try_from(read_be_sized(bytes, trailer_start + 16, 8)?).ok()?;
    let offset_table = usize::try_from(read_be_sized(bytes, trailer_start + 24, 8)?).ok()?;
    let table_length = object_count.checked_mul(offset_size)?;
    let table_end = offset_table.checked_add(table_length)?;
    if table_end > trailer_start || top_object >= object_count {
        return None;
    }

    let mut offsets = Vec::with_capacity(object_count);
    for index in 0..object_count {
        let entry_offset = offset_table.checked_add(index.checked_mul(offset_size)?)?;
        let object_offset =
            usize::try_from(read_be_sized(bytes, entry_offset, offset_size)?).ok()?;
        if object_offset >= trailer_start {
            return None;
        }
        offsets.push(object_offset);
    }

    let object = bplist_object_slice(bytes, &offsets, top_object, trailer_start)?;
    let (count, refs_start) = bplist_count(object, 0)?;
    if object.first().copied()? >> 4 != 0xD || count > limits.max_ifd_entries {
        return None;
    }
    let refs_length = count.checked_mul(2)?.checked_mul(object_ref_size)?;
    if refs_start.checked_add(refs_length)? > object.len() {
        return None;
    }

    let mut fields = BTreeMap::new();
    for index in 0..count {
        let key_offset = refs_start.checked_add(index.checked_mul(object_ref_size)?)?;
        let value_offset = refs_start
            .checked_add(count.checked_mul(object_ref_size)?)?
            .checked_add(index.checked_mul(object_ref_size)?)?;
        let key_ref = usize::try_from(read_be_sized(object, key_offset, object_ref_size)?).ok()?;
        let value_ref =
            usize::try_from(read_be_sized(object, value_offset, object_ref_size)?).ok()?;
        let key = bplist_object_slice(bytes, &offsets, key_ref, trailer_start)
            .and_then(parse_bplist_string)?;
        let Some(value) = bplist_object_slice(bytes, &offsets, value_ref, trailer_start)
            .and_then(parse_bplist_integer)
        else {
            continue;
        };
        fields.insert(key, value);
    }
    (!fields.is_empty()).then_some(fields)
}

fn bplist_object_slice<'a>(
    bytes: &'a [u8],
    offsets: &[usize],
    index: usize,
    trailer_start: usize,
) -> Option<&'a [u8]> {
    let start = *offsets.get(index)?;
    let end = offsets
        .iter()
        .copied()
        .filter(|offset| *offset > start)
        .min()
        .unwrap_or(trailer_start);
    (start < end && end <= trailer_start).then(|| bytes.get(start..end))?
}

fn bplist_count(bytes: &[u8], offset: usize) -> Option<(usize, usize)> {
    let marker = *bytes.get(offset)?;
    let info = marker & 0x0F;
    if info != 0x0F {
        return Some((usize::from(info), offset + 1));
    }
    let integer_marker = *bytes.get(offset.checked_add(1)?)?;
    if integer_marker >> 4 != 0x1 {
        return None;
    }
    let integer_size = 1usize.checked_shl(u32::from(integer_marker & 0x0F))?;
    let count =
        usize::try_from(read_be_sized(bytes, offset.checked_add(2)?, integer_size)?).ok()?;
    Some((count, offset.checked_add(2)?.checked_add(integer_size)?))
}

fn parse_bplist_string(bytes: &[u8]) -> Option<String> {
    let marker = *bytes.first()?;
    let (count, content_offset) = bplist_count(bytes, 0)?;
    let content = bytes.get(
        content_offset..content_offset.checked_add(match marker >> 4 {
            0x5 => count,
            0x6 => count.checked_mul(2)?,
            _ => return None,
        })?,
    )?;
    match marker >> 4 {
        0x5 => String::from_utf8(content.to_vec()).ok(),
        0x6 => {
            let units = content
                .chunks_exact(2)
                .map(|chunk| u16::from_be_bytes([chunk[0], chunk[1]]))
                .collect::<Vec<_>>();
            String::from_utf16(&units).ok()
        }
        _ => None,
    }
}

fn parse_bplist_integer(bytes: &[u8]) -> Option<TagValue> {
    let marker = *bytes.first()?;
    if marker >> 4 != 0x1 {
        return None;
    }
    let integer_size = 1usize.checked_shl(u32::from(marker & 0x0F))?;
    (integer_size <= 8).then(|| {
        TagValue::Unsigned(read_be_sized(bytes, 1, integer_size).expect("bounded integer"))
    })
}

fn read_be_sized(bytes: &[u8], offset: usize, width: usize) -> Option<u64> {
    if !matches!(width, 1 | 2 | 4 | 8) {
        return None;
    }
    let bytes = bytes.get(offset..offset.checked_add(width)?)?;
    Some(bytes.iter().fold(0_u64, |value, byte| {
        value.wrapping_shl(8) | u64::from(*byte)
    }))
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

fn identify(bytes: &[u8], make: Option<&str>) -> Option<MakerNoteIdentity> {
    if bytes.starts_with(b"Apple iOS\0") {
        return Some(MakerNoteIdentity {
            vendor: "Apple",
            format: "Apple MakerNote",
        });
    }
    if bytes.starts_with(b"AOC\0MM") {
        return Some(MakerNoteIdentity {
            vendor: "Pentax",
            format: "Pentax MakerNote",
        });
    }
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
    if bytes.starts_with(b"STMN") {
        return Some(MakerNoteIdentity {
            vendor: "Samsung",
            format: "Samsung STMN MakerNote",
        });
    }
    if bytes.starts_with(b"[ae_dbg_info:") {
        return Some(MakerNoteIdentity {
            vendor: "DJI",
            format: "DJI Debug MakerNote",
        });
    }
    if bytes.starts_with(b"DJI") {
        return Some(MakerNoteIdentity {
            vendor: "DJI",
            format: "DJI MakerNote",
        });
    }
    if make.is_some_and(|value| value.trim().eq_ignore_ascii_case("GoPro")) {
        return Some(MakerNoteIdentity {
            vendor: "GoPro",
            format: "GoPro MakerNote",
        });
    }
    if make.is_some_and(|value| value.trim().eq_ignore_ascii_case("DJI")) {
        return Some(MakerNoteIdentity {
            vendor: "DJI",
            format: "DJI MakerNote",
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

struct MakerValueSpec {
    group: &'static str,
    id: u16,
    name: &'static str,
    description: &'static str,
    source: &'static str,
    value: TagValue,
    raw_value: Vec<u8>,
    data_offset: u64,
    value_offset: usize,
}

fn add_maker_value(metadata: &mut Metadata, spec: MakerValueSpec) {
    let value_length = spec.raw_value.len();
    metadata.add_tag(Tag {
        namespace: "MakerNotes".to_owned(),
        group: spec.group.to_owned(),
        id: Some(u32::from(spec.id)),
        name: spec.name.to_owned(),
        description: Some(spec.description.to_owned()),
        raw_value: Some(spec.raw_value),
        value_type: value_type(&spec.value),
        value: spec.value,
        source: Source::new(
            spec.source,
            Some(spec.data_offset.saturating_add(spec.value_offset as u64)),
            Some(value_length as u64),
        ),
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
    fn reads_bounded_pentax_big_endian_ifd_values() {
        let mut maker_note = b"AOC\0MM".to_vec();
        maker_note.extend_from_slice(&3_u16.to_be_bytes());
        maker_note.extend_from_slice(&[
            0x00, 0x00, 0x00, 0x07, 0x00, 0x00, 0x00, 0x04, b'3', 0, 0, 0,
        ]);
        maker_note.extend_from_slice(&[
            0x00, 0x08, 0x00, 0x03, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0, 0,
        ]);
        maker_note.extend_from_slice(&[
            0x00, 0x27, 0x00, 0x07, 0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x68,
        ]);
        maker_note.extend_from_slice(&[0, 0, 0, 0]);
        maker_note.resize(52, 0);
        maker_note.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);

        let mut metadata = Metadata::new(FileInfo::new(
            "pentax.jpg".into(),
            maker_note.len() as u64,
            FileFormat::Jpeg,
        ));
        inspect_maker_note_with_context(
            &maker_note,
            5_000,
            &mut metadata,
            ParseLimits::default(),
            Some("PENTAX"),
            Some(4_948),
        );

        assert_eq!(
            metadata
                .find("MakerNotes:Pentax:PentaxVersion")
                .unwrap()
                .value,
            TagValue::Bytes(vec![b'3', 0, 0, 0])
        );
        assert_eq!(
            metadata.find("MakerNotes:Pentax:Quality").unwrap().value,
            TagValue::Unsigned(1)
        );
        assert_eq!(
            metadata
                .find("MakerNotes:Pentax:Quality")
                .unwrap()
                .source
                .offset,
            Some(5_028)
        );
        assert_eq!(
            metadata
                .find("MakerNotes:Pentax:DSPFirmwareVersion")
                .unwrap()
                .value,
            TagValue::Bytes(vec![1, 2, 3, 4, 5, 6, 7, 8])
        );
        assert_eq!(
            metadata
                .find("MakerNotes:Pentax:DSPFirmwareVersion")
                .unwrap()
                .source
                .offset,
            Some(5_052)
        );
    }

    #[test]
    fn reads_bounded_apple_big_endian_ifd_values() {
        let mut maker_note = b"Apple iOS\0\0\x01MM".to_vec();
        maker_note.extend_from_slice(&2_u16.to_be_bytes());
        maker_note.extend_from_slice(&[
            0x00, 0x01, 0x00, 0x09, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x04,
        ]);
        maker_note.extend_from_slice(&[
            0x00, 0x08, 0x00, 0x0A, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x2C,
        ]);
        maker_note.extend_from_slice(&[0, 0, 0, 0]);
        maker_note.extend_from_slice(&(-1_i32).to_be_bytes());
        maker_note.extend_from_slice(&2_i32.to_be_bytes());

        let mut metadata = Metadata::new(FileInfo::new(
            "apple.jpg".into(),
            maker_note.len() as u64,
            FileFormat::Jpeg,
        ));
        inspect_maker_note(&maker_note, 2_000, &mut metadata, ParseLimits::default());

        assert_eq!(
            metadata
                .find("MakerNotes:Apple:MakerNoteVersion")
                .unwrap()
                .value,
            TagValue::Signed(4)
        );
        assert_eq!(
            metadata
                .find("MakerNotes:Apple:AccelerationVector")
                .unwrap()
                .value,
            TagValue::Rational {
                numerator: -1,
                denominator: 2,
            }
        );
        assert_eq!(
            metadata
                .find("MakerNotes:Apple:AccelerationVector")
                .unwrap()
                .source
                .offset,
            Some(2_044)
        );
    }

    #[test]
    fn decodes_bounded_apple_runtime_binary_plist_integers() {
        let mut plist = b"bplist00".to_vec();
        plist.extend_from_slice(&[0xD2, 1, 2, 3, 4]);
        plist.extend_from_slice(b"Uflags");
        plist.extend_from_slice(b"Uvalue");
        plist.extend_from_slice(&[0x10, 1, 0x10, 42]);
        plist.extend_from_slice(&[8, 13, 19, 25, 27]);
        plist.extend_from_slice(&[0, 0, 0, 0, 0, 0, 1, 1]);
        plist.extend_from_slice(&5_u64.to_be_bytes());
        plist.extend_from_slice(&0_u64.to_be_bytes());
        plist.extend_from_slice(&29_u64.to_be_bytes());

        let fields = parse_apple_runtime_plist(&plist, ParseLimits::default()).unwrap();
        assert_eq!(fields.get("flags"), Some(&TagValue::Unsigned(1)));
        assert_eq!(fields.get("value"), Some(&TagValue::Unsigned(42)));
    }

    #[test]
    fn identifies_samsung_dji_and_gopro_maker_note_families() {
        assert_eq!(
            identify(b"STMN001\0", None),
            Some(MakerNoteIdentity {
                vendor: "Samsung",
                format: "Samsung STMN MakerNote",
            })
        );
        assert_eq!(
            identify(b"[ae_dbg_info:sample", None),
            Some(MakerNoteIdentity {
                vendor: "DJI",
                format: "DJI Debug MakerNote",
            })
        );
        assert_eq!(
            identify(b"opaque proprietary payload", Some("GoPro")),
            Some(MakerNoteIdentity {
                vendor: "GoPro",
                format: "GoPro MakerNote",
            })
        );
    }

    #[test]
    fn reads_bounded_samsung_stmn_fields_without_materializing_preview_data() {
        let mut maker_note = b"STMN001X\0\0\0\0".to_vec();
        maker_note.extend_from_slice(&500_u32.to_le_bytes());
        maker_note.extend_from_slice(&80_u32.to_le_bytes());
        maker_note.resize(48, 0);
        maker_note.extend_from_slice(&[1, 0, 0, 0, 0xAA, 0xBB, 0xCC]);

        let mut metadata = Metadata::new(FileInfo::new(
            "samsung.jpg".into(),
            maker_note.len() as u64,
            FileFormat::Jpeg,
        ));
        inspect_maker_note(&maker_note, 3_000, &mut metadata, ParseLimits::default());

        assert_eq!(
            metadata
                .find("MakerNotes:Samsung:MakerNoteVersion")
                .unwrap()
                .value,
            TagValue::Bytes(b"STMN001X".to_vec())
        );
        assert_eq!(
            metadata
                .find("MakerNotes:Samsung:PreviewImageStart")
                .unwrap()
                .value,
            TagValue::Unsigned(500)
        );
        assert_eq!(
            metadata
                .find("MakerNotes:Samsung:PreviewImageLength")
                .unwrap()
                .value,
            TagValue::Unsigned(80)
        );
        assert_eq!(
            metadata
                .find("MakerNotes:Samsung:SamsungIFD")
                .unwrap()
                .value,
            TagValue::Bytes(vec![1, 0, 0, 0, 0xAA, 0xBB, 0xCC])
        );
        assert_eq!(
            metadata
                .find("MakerNotes:Samsung:SamsungIFD")
                .unwrap()
                .source
                .offset,
            Some(3_048)
        );
    }

    #[test]
    fn reads_bounded_dji_ifd_values_with_manufacturer_context() {
        let mut maker_note = vec![2, 0];
        maker_note.extend_from_slice(&[1, 0, 2, 0, 4, 0, 0, 0, b'D', b'J', b'I', 0]);
        maker_note.extend_from_slice(&[6, 0, 11, 0, 1, 0, 0, 0]);
        maker_note.extend_from_slice(&1.5_f32.to_le_bytes());
        maker_note.extend_from_slice(&[0, 0, 0, 0]);

        let mut metadata = Metadata::new(FileInfo::new(
            "dji.jpg".into(),
            maker_note.len() as u64,
            FileFormat::Jpeg,
        ));
        inspect_maker_note_with_context(
            &maker_note,
            4_000,
            &mut metadata,
            ParseLimits::default(),
            Some("DJI"),
            None,
        );

        assert_eq!(
            metadata.find("MakerNotes:DJI:Make").unwrap().value,
            TagValue::String("DJI".to_owned())
        );
        assert_eq!(
            metadata.find("MakerNotes:DJI:Pitch").unwrap().value,
            TagValue::Float(1.5)
        );
        assert_eq!(
            metadata.find("MakerNotes:DJI:Pitch").unwrap().source.offset,
            Some(4_022)
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
