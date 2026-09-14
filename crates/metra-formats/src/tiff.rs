use std::collections::HashSet;
use std::io::{Read, Seek, SeekFrom};

use metra_core::{
    FileInfo, Metadata, MetraError, ParseLimits, Result, Source, Tag, TagValue, ValueType, Warning,
};

use crate::makers::{inspect_maker_note, inspect_maker_note_with_context};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Endian {
    Little,
    Big,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TiffVariant {
    Classic,
    Big,
}

impl TiffVariant {
    const fn count_size(self) -> usize {
        match self {
            Self::Classic => 2,
            Self::Big => 8,
        }
    }

    const fn entry_size(self) -> usize {
        match self {
            Self::Classic => 12,
            Self::Big => 20,
        }
    }

    const fn inline_value_size(self) -> usize {
        match self {
            Self::Classic => 4,
            Self::Big => 8,
        }
    }
}

impl Endian {
    fn u16(self, bytes: &[u8]) -> u16 {
        let bytes: [u8; 2] = bytes.try_into().expect("caller validates length");
        match self {
            Self::Little => u16::from_le_bytes(bytes),
            Self::Big => u16::from_be_bytes(bytes),
        }
    }

    fn i16(self, bytes: &[u8]) -> i16 {
        let bytes: [u8; 2] = bytes.try_into().expect("caller validates length");
        match self {
            Self::Little => i16::from_le_bytes(bytes),
            Self::Big => i16::from_be_bytes(bytes),
        }
    }

    fn u32(self, bytes: &[u8]) -> u32 {
        let bytes: [u8; 4] = bytes.try_into().expect("caller validates length");
        match self {
            Self::Little => u32::from_le_bytes(bytes),
            Self::Big => u32::from_be_bytes(bytes),
        }
    }

    fn i32(self, bytes: &[u8]) -> i32 {
        let bytes: [u8; 4] = bytes.try_into().expect("caller validates length");
        match self {
            Self::Little => i32::from_le_bytes(bytes),
            Self::Big => i32::from_be_bytes(bytes),
        }
    }

    fn u64(self, bytes: &[u8]) -> u64 {
        let bytes: [u8; 8] = bytes.try_into().expect("caller validates length");
        match self {
            Self::Little => u64::from_le_bytes(bytes),
            Self::Big => u64::from_be_bytes(bytes),
        }
    }

    fn i64(self, bytes: &[u8]) -> i64 {
        let bytes: [u8; 8] = bytes.try_into().expect("caller validates length");
        match self {
            Self::Little => i64::from_le_bytes(bytes),
            Self::Big => i64::from_be_bytes(bytes),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct TagDefinition {
    namespace: &'static str,
    name: &'static str,
    description: &'static str,
}

fn tag_definition(namespace: &str, id: u16) -> TagDefinition {
    let definition = metra_core::tag_definition(namespace, u32::from(id));
    let definition = if namespace == "EXIF" && definition.name == "Unknown" {
        let dng_definition = metra_core::tag_definition("DNG", u32::from(id));
        if dng_definition.name != "Unknown" {
            dng_definition
        } else {
            definition
        }
    } else {
        definition
    };
    TagDefinition {
        namespace: definition.namespace,
        name: definition.name,
        description: definition.description,
    }
}

pub fn read_tiff<R: Read + Seek>(
    reader: &mut R,
    file_info: FileInfo,
    limits: ParseLimits,
) -> Result<Metadata> {
    let length = file_info.size;
    let mut metadata = Metadata::new(file_info);
    parse_tiff_from_reader(reader, 0, length, 0, &mut metadata, limits)?;
    metadata.sort_tags();
    Ok(metadata)
}

pub(crate) fn parse_tiff_from_reader<R: Read + Seek>(
    reader: &mut R,
    seek_start: u64,
    length: u64,
    absolute_start: u64,
    metadata: &mut Metadata,
    limits: ParseLimits,
) -> Result<()> {
    if length < 8 {
        return Err(MetraError::InvalidHeader {
            context: "TIFF".to_owned(),
            message: "header is shorter than 8 bytes".to_owned(),
        });
    }

    let mut parser = TiffParser {
        reader,
        seek_start,
        length,
        absolute_start,
        limits,
        endian: Endian::Little,
        variant: TiffVariant::Classic,
        bytes_read: 0,
        visited_ifds: HashSet::new(),
    };
    parser.parse(metadata)
}

struct TiffParser<'a, R> {
    reader: &'a mut R,
    seek_start: u64,
    length: u64,
    absolute_start: u64,
    limits: ParseLimits,
    endian: Endian,
    variant: TiffVariant,
    bytes_read: usize,
    visited_ifds: HashSet<u64>,
}

impl<R: Read + Seek> TiffParser<'_, R> {
    fn parse(&mut self, metadata: &mut Metadata) -> Result<()> {
        let header = self.read_at(0, 8, "TIFF header")?;
        self.endian = match &header[..2] {
            b"II" => Endian::Little,
            b"MM" => Endian::Big,
            _ => {
                return Err(MetraError::InvalidHeader {
                    context: "TIFF".to_owned(),
                    message: "byte order must be II or MM".to_owned(),
                });
            }
        };
        let magic = self.endian.u16(&header[2..4]);
        let (variant, first_ifd) = match magic {
            42 | 85 => (
                TiffVariant::Classic,
                u64::from(self.endian.u32(&header[4..8])),
            ),
            43 => {
                let big_header = self.read_at(4, 12, "BigTIFF header")?;
                let offset_size = self.endian.u16(&big_header[..2]);
                let reserved = self.endian.u16(&big_header[2..4]);
                if offset_size != 8 || reserved != 0 {
                    return Err(MetraError::InvalidHeader {
                        context: "BigTIFF".to_owned(),
                        message: format!(
                            "expected 8-byte offsets and zero reserved field, got {offset_size} and {reserved}"
                        ),
                    });
                }
                (TiffVariant::Big, self.endian.u64(&big_header[4..12]))
            }
            _ => {
                return Err(MetraError::InvalidHeader {
                    context: "TIFF".to_owned(),
                    message: format!(
                        "expected classic TIFF/RW2 magic 42 or 85, or BigTIFF magic 43, got {magic}"
                    ),
                });
            }
        };
        self.variant = variant;
        if first_ifd != 0 {
            self.parse_ifd(first_ifd, "EXIF", "IFD0", 0, metadata)?;
        } else {
            metadata.add_warning(Warning::new(
                "missing-ifd",
                "TIFF header contains no first image directory",
            ));
        }
        add_gps_decimal_tags(metadata);
        validate_thumbnail_reference(metadata, self.length, self.absolute_start);
        Ok(())
    }

    fn parse_ifd(
        &mut self,
        offset: u64,
        namespace: &str,
        group: &str,
        depth: usize,
        metadata: &mut Metadata,
    ) -> Result<()> {
        if depth > self.limits.max_recursion_depth {
            metadata.add_warning(
                Warning::new(
                    "recursion-limit",
                    format!("stopped nested IFD traversal at depth {}", depth),
                )
                .at(self.absolute_start.saturating_add(offset)),
            );
            return Ok(());
        }
        if !self.visited_ifds.insert(offset) {
            metadata.add_warning(
                Warning::new(
                    "cyclic-ifd",
                    format!("IFD offset {offset} was already visited"),
                )
                .at(self.absolute_start.saturating_add(offset)),
            );
            return Ok(());
        }

        let count_bytes = self.read_at(offset, self.variant.count_size(), "IFD entry count")?;
        let count = match self.variant {
            TiffVariant::Classic => u64::from(self.endian.u16(&count_bytes)),
            TiffVariant::Big => self.endian.u64(&count_bytes),
        };
        let max_entries = u64::try_from(self.limits.max_ifd_entries).unwrap_or(u64::MAX);
        let count_to_read = usize::try_from(count.min(max_entries)).unwrap_or(usize::MAX);
        if count > count_to_read as u64 {
            metadata.add_warning(
                Warning::new(
                    "ifd-entry-limit",
                    format!("IFD declares {count} entries; reading only {count_to_read}"),
                )
                .at(self.absolute_start.saturating_add(offset)),
            );
        }

        let entries_start = offset.checked_add(self.variant.count_size() as u64).ok_or(
            MetraError::InvalidOffset {
                context: "IFD entries".to_owned(),
                offset,
            },
        )?;
        for index in 0..count_to_read {
            let index_offset = u64::try_from(index).map_err(|_| MetraError::InvalidOffset {
                context: "IFD entry index".to_owned(),
                offset: index as u64,
            })?;
            let entry_offset = entries_start
                .checked_add(
                    index_offset
                        .checked_mul(self.variant.entry_size() as u64)
                        .ok_or(MetraError::InvalidOffset {
                            context: "IFD entry offset".to_owned(),
                            offset: index_offset,
                        })?,
                )
                .ok_or(MetraError::InvalidOffset {
                    context: "IFD entry offset".to_owned(),
                    offset: entries_start,
                })?;
            let entry = self.read_at(entry_offset, self.variant.entry_size(), "IFD entry")?;
            self.parse_entry(&entry, entry_offset, namespace, group, depth, metadata)?;
        }

        let next_offset_position = entries_start
            .checked_add(
                u64::try_from(count_to_read)
                    .map_err(|_| MetraError::InvalidOffset {
                        context: "next IFD pointer".to_owned(),
                        offset,
                    })?
                    .checked_mul(self.variant.entry_size() as u64)
                    .ok_or(MetraError::InvalidOffset {
                        context: "next IFD pointer".to_owned(),
                        offset,
                    })?,
            )
            .ok_or(MetraError::InvalidOffset {
                context: "next IFD pointer".to_owned(),
                offset,
            })?;
        if count == count_to_read as u64 {
            let next = self.read_at(
                next_offset_position,
                self.variant.inline_value_size(),
                "next IFD pointer",
            )?;
            let next = match self.variant {
                TiffVariant::Classic => u64::from(self.endian.u32(&next)),
                TiffVariant::Big => self.endian.u64(&next),
            };
            if next != 0 {
                let next_group = next_ifd_group(namespace, group);
                self.parse_ifd(next, namespace, &next_group, depth, metadata)?;
            }
        }
        Ok(())
    }

    fn parse_entry(
        &mut self,
        entry: &[u8],
        entry_offset: u64,
        namespace: &str,
        group: &str,
        depth: usize,
        metadata: &mut Metadata,
    ) -> Result<()> {
        let id = self.endian.u16(&entry[..2]);
        let type_id = self.endian.u16(&entry[2..4]);
        let count = match self.variant {
            TiffVariant::Classic => u64::from(self.endian.u32(&entry[4..8])),
            TiffVariant::Big => self.endian.u64(&entry[4..12]),
        };
        let value_offset_start = match self.variant {
            TiffVariant::Classic => 8,
            TiffVariant::Big => 12,
        };
        let inline_value_size = self.variant.inline_value_size();
        let entry_size = self.variant.entry_size();
        let definition = tag_definition(namespace, id);
        let Some(item_size) = type_size(type_id) else {
            metadata.add_warning(
                Warning::new(
                    "unsupported-tiff-type",
                    format!(
                        "{}:0x{id:04X} uses unsupported TIFF type {type_id}; value omitted",
                        definition.namespace
                    ),
                )
                .at(self.absolute_start.saturating_add(entry_offset)),
            );
            let name = if definition.name == "Unknown" {
                format!("Tag0x{id:04X}")
            } else {
                definition.name.to_owned()
            };
            metadata.add_tag(Tag {
                namespace: definition.namespace.to_owned(),
                group: group.to_owned(),
                id: Some(u32::from(id)),
                name,
                description: Some(definition.description.to_owned()),
                raw_value: None,
                value: TagValue::Unknown {
                    type_id,
                    bytes: Vec::new(),
                },
                value_type: ValueType::Unknown,
                source: Source::new(
                    format!("TIFF/{group}"),
                    Some(self.absolute_start.saturating_add(entry_offset)),
                    Some(entry_size as u64),
                ),
                writable: false,
            });
            return Ok(());
        };
        let total_size = count
            .checked_mul(
                u64::try_from(item_size).map_err(|_| MetraError::InvalidOffset {
                    context: "tag value size".to_owned(),
                    offset: item_size as u64,
                })?,
            )
            .ok_or(MetraError::InvalidOffset {
                context: format!("{group}/0x{id:04X} value size"),
                offset: count,
            })?;

        let (value, raw_value, value_type) =
            if total_size > u64::try_from(self.limits.max_value_bytes).unwrap_or(u64::MAX) {
                metadata.add_warning(
                    Warning::new(
                        "value-limit",
                        format!(
                            "{}:0x{id:04X} contains {total_size} bytes; value omitted",
                            definition.namespace
                        ),
                    )
                    .at(self.absolute_start.saturating_add(entry_offset)),
                );
                (
                    TagValue::Unknown {
                        type_id,
                        bytes: Vec::new(),
                    },
                    None,
                    ValueType::Unknown,
                )
            } else {
                let total_size_usize =
                    usize::try_from(total_size).map_err(|_| MetraError::ResourceLimitExceeded {
                        resource: "tag value".to_owned(),
                        limit: self.limits.max_value_bytes,
                    })?;
                let value_bytes = if total_size_usize <= inline_value_size {
                    entry[value_offset_start..value_offset_start + total_size_usize].to_vec()
                } else {
                    let value_offset = match self.variant {
                        TiffVariant::Classic => u64::from(
                            self.endian
                                .u32(&entry[value_offset_start..value_offset_start + 4]),
                        ),
                        TiffVariant::Big => self.endian.u64(
                            &entry[value_offset_start..value_offset_start + inline_value_size],
                        ),
                    };
                    self.read_at(value_offset, total_size_usize, "tag value")?
                };
                let value = decode_value(type_id, count, &value_bytes, self.endian)?;
                let value = decode_special_value(namespace, id, type_id, &value_bytes, value);
                let value_type = value_type(&value);
                (value, Some(value_bytes), value_type)
            };

        let name = if definition.name == "Unknown" {
            format!("Tag0x{id:04X}")
        } else {
            definition.name.to_owned()
        };
        let maker_note = if namespace == "EXIF" && id == 0x927C {
            match &value {
                TagValue::Bytes(bytes) => {
                    let value_offset = if total_size <= inline_value_size as u64 {
                        entry_offset.saturating_add(value_offset_start as u64)
                    } else {
                        match self.variant {
                            TiffVariant::Classic => u64::from(self.endian.u32(
                                &entry[value_offset_start..value_offset_start + inline_value_size],
                            )),
                            TiffVariant::Big => self.endian.u64(
                                &entry[value_offset_start..value_offset_start + inline_value_size],
                            ),
                        }
                    };
                    Some((
                        bytes.clone(),
                        self.absolute_start.saturating_add(value_offset),
                    ))
                }
                _ => None,
            }
        } else {
            None
        };
        let sub_ifd_offsets = if namespace == "EXIF" && id == 0x014A {
            match &value {
                TagValue::Unsigned(offset) => vec![*offset],
                TagValue::Array(values) => values
                    .iter()
                    .filter_map(|value| match value {
                        TagValue::Unsigned(offset) => Some(*offset),
                        _ => None,
                    })
                    .collect(),
                _ => Vec::new(),
            }
        } else {
            Vec::new()
        };
        let is_empty_string = matches!(&value, TagValue::String(value) if value.is_empty());
        let is_zeroed_gps_coordinate = namespace == "GPS"
            && matches!(id, 0x0002 | 0x0004)
            && matches!(
                &value,
                TagValue::Array(values)
                    if values.len() == 3
                        && values.iter().all(|value| matches!(
                            value,
                            TagValue::UnsignedRational {
                                numerator: 0,
                                denominator: 0,
                            }
                        ))
            );
        let is_zeroed_gps_scalar = namespace == "GPS"
            && matches!(id, 0x0006 | 0x000D | 0x0011)
            && matches!(
                &value,
                TagValue::UnsignedRational {
                    numerator: 0,
                    denominator: 0,
                }
            );
        if !is_empty_string && !is_zeroed_gps_coordinate && !is_zeroed_gps_scalar {
            metadata.add_tag(Tag {
                namespace: definition.namespace.to_owned(),
                group: group.to_owned(),
                id: Some(u32::from(id)),
                name,
                description: Some(definition.description.to_owned()),
                raw_value,
                value,
                value_type,
                source: Source::new(
                    format!("TIFF/{group}"),
                    Some(self.absolute_start.saturating_add(entry_offset)),
                    Some(entry_size as u64),
                ),
                writable: false,
            });
        }

        if let Some((bytes, maker_note_offset)) = maker_note {
            let make = metadata.find("EXIF:Make").and_then(|tag| match &tag.value {
                TagValue::String(value) => Some(value.clone()),
                _ => None,
            });
            if let Some(make) = make.as_deref() {
                inspect_maker_note_with_context(
                    &bytes,
                    maker_note_offset,
                    metadata,
                    self.limits,
                    Some(make),
                    Some(self.absolute_start),
                );
            } else {
                inspect_maker_note(&bytes, maker_note_offset, metadata, self.limits);
            }
        }

        if count == 1 && matches!(id, 0x8769 | 0x8825 | 0xA005) {
            let pointer = match metadata.tags.last().map(|tag| &tag.value) {
                Some(TagValue::Unsigned(value)) => *value,
                _ => 0,
            };
            if pointer != 0 {
                let nested = match id {
                    0x8769 => Some(("EXIF", "ExifIFD")),
                    0x8825 => Some(("GPS", "GPS")),
                    0xA005 => Some(("Interop", "InteropIFD")),
                    _ => None,
                };
                if let Some((nested_namespace, nested_group)) = nested {
                    self.parse_ifd(pointer, nested_namespace, nested_group, depth + 1, metadata)?;
                }
            }
        }
        for (index, offset) in sub_ifd_offsets.into_iter().enumerate() {
            if offset == 0 {
                continue;
            }
            let group = format!("SubIFD{}", index.saturating_add(1));
            self.parse_ifd(offset, namespace, &group, depth + 1, metadata)?;
        }
        Ok(())
    }

    fn read_at(&mut self, offset: u64, length: usize, context: &str) -> Result<Vec<u8>> {
        let length_u64 = u64::try_from(length).map_err(|_| MetraError::InvalidOffset {
            context: context.to_owned(),
            offset,
        })?;
        let end = offset
            .checked_add(length_u64)
            .ok_or(MetraError::InvalidOffset {
                context: context.to_owned(),
                offset,
            })?;
        if end > self.length {
            return Err(MetraError::UnexpectedEof {
                context: format!("{context} at TIFF offset {offset}"),
            });
        }
        self.bytes_read =
            self.bytes_read
                .checked_add(length)
                .ok_or(MetraError::ResourceLimitExceeded {
                    resource: "metadata reads".to_owned(),
                    limit: self.limits.max_metadata_bytes,
                })?;
        if self.bytes_read > self.limits.max_metadata_bytes {
            return Err(MetraError::ResourceLimitExceeded {
                resource: "metadata reads".to_owned(),
                limit: self.limits.max_metadata_bytes,
            });
        }
        let absolute_seek =
            self.seek_start
                .checked_add(offset)
                .ok_or(MetraError::InvalidOffset {
                    context: context.to_owned(),
                    offset,
                })?;
        self.reader
            .seek(SeekFrom::Start(absolute_seek))
            .map_err(|source| MetraError::Io {
                path: std::path::PathBuf::from("<reader>"),
                source,
            })?;
        let mut buffer = vec![0_u8; length];
        self.reader
            .read_exact(&mut buffer)
            .map_err(|source| match source.kind() {
                std::io::ErrorKind::UnexpectedEof => MetraError::UnexpectedEof {
                    context: context.to_owned(),
                },
                _ => MetraError::Io {
                    path: std::path::PathBuf::from("<reader>"),
                    source,
                },
            })?;
        Ok(buffer)
    }
}

fn type_size(type_id: u16) -> Option<usize> {
    match type_id {
        1 | 2 | 6 | 7 => Some(1),
        3 | 8 => Some(2),
        4 | 9 | 11 | 13 => Some(4),
        5 | 10 | 12 | 16 | 17 | 18 => Some(8),
        _ => None,
    }
}

fn decode_value(type_id: u16, count: u64, bytes: &[u8], endian: Endian) -> Result<TagValue> {
    let count = usize::try_from(count).map_err(|_| MetraError::InvalidTag {
        context: "TIFF value".to_owned(),
        message: "element count does not fit in memory".to_owned(),
    })?;
    let values = match type_id {
        1 => bytes
            .iter()
            .map(|byte| TagValue::Unsigned(u64::from(*byte)))
            .collect::<Vec<_>>(),
        2 => {
            let text = String::from_utf8_lossy(bytes)
                .trim_end_matches('\0')
                .to_owned();
            return Ok(TagValue::String(text));
        }
        3 => bytes
            .chunks_exact(2)
            .map(|chunk| TagValue::Unsigned(u64::from(endian.u16(chunk))))
            .collect::<Vec<_>>(),
        4 | 13 => bytes
            .chunks_exact(4)
            .map(|chunk| TagValue::Unsigned(u64::from(endian.u32(chunk))))
            .collect::<Vec<_>>(),
        16 | 18 => bytes
            .chunks_exact(8)
            .map(|chunk| TagValue::Unsigned(endian.u64(chunk)))
            .collect::<Vec<_>>(),
        5 => bytes
            .chunks_exact(8)
            .map(|chunk| TagValue::UnsignedRational {
                numerator: u64::from(endian.u32(&chunk[..4])),
                denominator: u64::from(endian.u32(&chunk[4..8])),
            })
            .collect::<Vec<_>>(),
        6 => bytes
            .iter()
            .map(|byte| TagValue::Signed(i64::from(i8::from_ne_bytes([*byte]))))
            .collect::<Vec<_>>(),
        7 => return Ok(TagValue::Bytes(bytes.to_vec())),
        8 => bytes
            .chunks_exact(2)
            .map(|chunk| TagValue::Signed(i64::from(endian.i16(chunk))))
            .collect::<Vec<_>>(),
        9 => bytes
            .chunks_exact(4)
            .map(|chunk| TagValue::Signed(i64::from(endian.i32(chunk))))
            .collect::<Vec<_>>(),
        10 => bytes
            .chunks_exact(8)
            .map(|chunk| TagValue::Rational {
                numerator: i64::from(endian.i32(&chunk[..4])),
                denominator: i64::from(endian.i32(&chunk[4..8])),
            })
            .collect::<Vec<_>>(),
        17 => bytes
            .chunks_exact(8)
            .map(|chunk| TagValue::Signed(endian.i64(chunk)))
            .collect::<Vec<_>>(),
        11 => bytes
            .chunks_exact(4)
            .map(|chunk| TagValue::Float(f32::from_bits(endian.u32(chunk)) as f64))
            .collect::<Vec<_>>(),
        12 => bytes
            .chunks_exact(8)
            .map(|chunk| TagValue::Float(f64::from_bits(endian.u64(chunk))))
            .collect::<Vec<_>>(),
        _ => {
            return Ok(TagValue::Unknown {
                type_id,
                bytes: bytes.to_vec(),
            });
        }
    };

    if values.len() != count {
        return Err(MetraError::InvalidTag {
            context: "TIFF value".to_owned(),
            message: format!("expected {count} values, decoded {}", values.len()),
        });
    }
    if values
        .iter()
        .any(|value| matches!(value, TagValue::Float(value) if !value.is_finite()))
    {
        return Ok(TagValue::Unknown {
            type_id,
            bytes: bytes.to_vec(),
        });
    }
    if values.len() == 1 {
        Ok(values.into_iter().next().expect("length checked"))
    } else {
        Ok(TagValue::Array(values))
    }
}

fn decode_special_value(
    namespace: &str,
    id: u16,
    type_id: u16,
    bytes: &[u8],
    value: TagValue,
) -> TagValue {
    if namespace == "EXIF" && id == 0x9286 && type_id == 7 {
        return decode_user_comment(bytes).map_or(value, TagValue::String);
    }
    if namespace == "EXIF" && type_id == 2 && matches!(id, 0x0132 | 0x9003 | 0x9004) {
        return parse_exif_datetime(&value).unwrap_or(value);
    }
    if namespace == "GPS" {
        if id == 0x001D && type_id == 2 {
            return parse_gps_date(&value).unwrap_or(value);
        }
        if id == 0x0007 && type_id == 5 {
            return parse_gps_time(&value).unwrap_or(value);
        }
    }
    value
}

fn parse_exif_datetime(value: &TagValue) -> Option<TagValue> {
    let TagValue::String(value) = value else {
        return None;
    };
    let mut parts = value.split([':', ' ', '-']);
    let year = parts.next()?.parse().ok()?;
    let month = parts.next()?.parse().ok()?;
    let day = parts.next()?.parse().ok()?;
    let hour = parts.next()?.parse().ok()?;
    let minute = parts.next()?.parse().ok()?;
    let second = parts.next()?.parse().ok()?;
    if parts.next().is_some() || !valid_date(year, month, day) || !valid_time(hour, minute, second)
    {
        return None;
    }
    Some(TagValue::DateTime {
        year,
        month,
        day,
        hour,
        minute,
        second,
        nanosecond: 0,
        offset_minutes: None,
    })
}

fn parse_gps_date(value: &TagValue) -> Option<TagValue> {
    let TagValue::String(value) = value else {
        return None;
    };
    let mut parts = value.split(':');
    let year = parts.next()?.parse().ok()?;
    let month = parts.next()?.parse().ok()?;
    let day = parts.next()?.parse().ok()?;
    if parts.next().is_some() || !valid_date(year, month, day) {
        return None;
    }
    Some(TagValue::Date { year, month, day })
}

fn parse_gps_time(value: &TagValue) -> Option<TagValue> {
    let values = match value {
        TagValue::Array(values) => values,
        _ => return None,
    };
    if values.len() != 3 {
        return None;
    }
    let hour = rational_u8(&values[0], 24)?;
    let minute = rational_u8(&values[1], 60)?;
    let (second, nanosecond) = rational_second(&values[2])?;
    if hour >= 24 || minute >= 60 || second >= 60 {
        return None;
    }
    Some(TagValue::Time {
        hour,
        minute,
        second,
        nanosecond,
    })
}

fn rational_u8(value: &TagValue, exclusive_maximum: u8) -> Option<u8> {
    let TagValue::UnsignedRational {
        numerator,
        denominator,
    } = value
    else {
        return None;
    };
    if *denominator == 0 || numerator % denominator != 0 {
        return None;
    }
    let value = numerator / denominator;
    (value < u64::from(exclusive_maximum))
        .then(|| u8::try_from(value).ok())
        .flatten()
}

fn rational_second(value: &TagValue) -> Option<(u8, u32)> {
    let TagValue::UnsignedRational {
        numerator,
        denominator,
    } = value
    else {
        return None;
    };
    if *denominator == 0 {
        return None;
    }
    let second = numerator / denominator;
    if second >= 60 {
        return None;
    }
    let remainder = numerator % denominator;
    let nanosecond = remainder.checked_mul(1_000_000_000)? / denominator;
    Some((u8::try_from(second).ok()?, u32::try_from(nanosecond).ok()?))
}

fn valid_date(year: u16, month: u8, day: u8) -> bool {
    (1..=12).contains(&month) && (1..=days_in_month(year, month)).contains(&day)
}

fn days_in_month(year: u16, month: u8) -> u8 {
    match month {
        2 if year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400)) => {
            29
        }
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

fn valid_time(hour: u8, minute: u8, second: u8) -> bool {
    hour < 24 && minute < 60 && second < 60
}

fn decode_user_comment(bytes: &[u8]) -> Option<String> {
    if bytes.len() < 8 {
        return None;
    }
    let (encoding, payload) = bytes.split_at(8);
    let mut value = match encoding {
        b"ASCII\0\0\0" => String::from_utf8(payload.to_vec()).ok()?,
        b"UNICODE\0" => decode_utf16(payload, true)?,
        b"JIS\0\0\0\0\0" => return None,
        _ => return None,
    };
    while value.ends_with('\0') {
        value.pop();
    }
    Some(value)
}

fn decode_utf16(bytes: &[u8], big_endian: bool) -> Option<String> {
    let chunks = bytes.chunks_exact(2);
    if !chunks.remainder().is_empty() {
        return None;
    }
    let units = chunks
        .map(|chunk| {
            if big_endian {
                u16::from_be_bytes([chunk[0], chunk[1]])
            } else {
                u16::from_le_bytes([chunk[0], chunk[1]])
            }
        })
        .collect::<Vec<_>>();
    String::from_utf16(&units).ok()
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

fn next_ifd_group(namespace: &str, group: &str) -> String {
    if namespace == "EXIF"
        && let Some(index) = group
            .strip_prefix("IFD")
            .and_then(|value| value.parse::<u32>().ok())
    {
        return format!("IFD{}", index.saturating_add(1));
    }
    "IFD-next".to_owned()
}

fn validate_thumbnail_reference(metadata: &mut Metadata, length: u64, absolute_start: u64) {
    let offset = metadata
        .tags
        .iter()
        .find(|tag| tag.id == Some(0x0201))
        .and_then(|tag| match tag.value {
            TagValue::Unsigned(value) => Some(value),
            _ => None,
        });
    let thumbnail_length = metadata
        .tags
        .iter()
        .find(|tag| tag.id == Some(0x0202))
        .and_then(|tag| match tag.value {
            TagValue::Unsigned(value) => Some(value),
            _ => None,
        });
    if let (Some(offset), Some(thumbnail_length)) = (offset, thumbnail_length) {
        let end = offset.saturating_add(thumbnail_length);
        if offset >= length || end > length {
            metadata.add_warning(
                Warning::new(
                    "thumbnail-offset",
                    format!(
                        "thumbnail range {offset}..{end} is outside the TIFF payload of {length} bytes"
                    ),
                )
                .at(absolute_start.saturating_add(offset)),
            );
        }
    }
}

fn add_gps_decimal_tags(metadata: &mut Metadata) {
    let latitude = gps_decimal(metadata, "GPSLatitude", "GPSLatitudeRef");
    let longitude = gps_decimal(metadata, "GPSLongitude", "GPSLongitudeRef");
    if let Some((value, source)) = latitude {
        metadata.add_tag(Tag {
            namespace: "GPS".to_owned(),
            group: "Derived".to_owned(),
            id: None,
            name: "LatitudeDecimal".to_owned(),
            description: Some("Latitude converted to decimal degrees".to_owned()),
            raw_value: None,
            value: TagValue::Float(value),
            value_type: ValueType::Float,
            source,
            writable: false,
        });
    }
    if let Some((value, source)) = longitude {
        metadata.add_tag(Tag {
            namespace: "GPS".to_owned(),
            group: "Derived".to_owned(),
            id: None,
            name: "LongitudeDecimal".to_owned(),
            description: Some("Longitude converted to decimal degrees".to_owned()),
            raw_value: None,
            value: TagValue::Float(value),
            value_type: ValueType::Float,
            source,
            writable: false,
        });
    }
    if let Some(value) = gps_altitude_meters(metadata) {
        add_derived_gps_tag(
            metadata,
            "AltitudeMeters",
            "Altitude converted to meters",
            TagValue::Float(value),
            ValueType::Float,
        );
    }
    if let Some(value) = gps_direction_degrees(metadata) {
        add_derived_gps_tag(
            metadata,
            "ImageDirectionDegrees",
            "Image direction in degrees",
            TagValue::Float(value),
            ValueType::Float,
        );
    }
    if let Some(value) = gps_time_seconds(metadata) {
        add_derived_gps_tag(
            metadata,
            "TimeOfDaySeconds",
            "GPS time converted to seconds since midnight",
            TagValue::Float(value),
            ValueType::Float,
        );
    }
    if let Some(value) = gps_speed_meters_per_second(metadata) {
        add_derived_gps_tag(
            metadata,
            "SpeedMetersPerSecond",
            "GPS speed converted to meters per second",
            TagValue::Float(value),
            ValueType::Float,
        );
    }
}

fn add_derived_gps_tag(
    metadata: &mut Metadata,
    name: &str,
    description: &str,
    value: TagValue,
    value_type: ValueType,
) {
    metadata.add_tag(Tag {
        namespace: "GPS".to_owned(),
        group: "Derived".to_owned(),
        id: None,
        name: name.to_owned(),
        description: Some(description.to_owned()),
        raw_value: None,
        value,
        value_type,
        source: Source::new("derived/GPS", None, None),
        writable: false,
    });
}

fn gps_decimal(
    metadata: &Metadata,
    coordinate_name: &str,
    reference_name: &str,
) -> Option<(f64, Source)> {
    let coordinate = metadata
        .tags
        .iter()
        .find(|tag| tag.namespace == "GPS" && tag.name == coordinate_name)?;
    let reference = metadata
        .tags
        .iter()
        .find(|tag| tag.namespace == "GPS" && tag.name == reference_name)
        .and_then(|tag| match &tag.value {
            TagValue::String(value) => value.chars().next(),
            _ => None,
        })?;
    let values = match &coordinate.value {
        TagValue::Array(values) => values,
        _ => return None,
    };
    if values.len() != 3 {
        return None;
    }
    let mut parts = [0.0_f64; 3];
    for (index, value) in values.iter().enumerate() {
        parts[index] = match value {
            TagValue::UnsignedRational {
                numerator,
                denominator,
            } if *denominator != 0 => *numerator as f64 / *denominator as f64,
            TagValue::Rational {
                numerator,
                denominator,
            } if *denominator != 0 => *numerator as f64 / *denominator as f64,
            _ => return None,
        };
    }
    if parts.iter().any(|part| *part < 0.0) || parts[1] >= 60.0 || parts[2] >= 60.0 {
        return None;
    }
    let sign = match reference {
        'N' | 'n' | 'E' | 'e' => 1.0,
        'S' | 's' | 'W' | 'w' => -1.0,
        _ => return None,
    };
    let decimal = sign * (parts[0] + parts[1] / 60.0 + parts[2] / 3_600.0);
    let maximum = if matches!(reference, 'N' | 'n' | 'S' | 's') {
        90.0
    } else {
        180.0
    };
    if decimal.abs() > maximum {
        return None;
    }
    Some((decimal, Source::new("derived/GPS", None, None)))
}

fn gps_altitude_meters(metadata: &Metadata) -> Option<f64> {
    let altitude = metadata
        .tags
        .iter()
        .find(|tag| tag.namespace == "GPS" && tag.name == "GPSAltitude")
        .and_then(|tag| scalar_number(&tag.value))?;
    let reference = metadata
        .tags
        .iter()
        .find(|tag| tag.namespace == "GPS" && tag.name == "GPSAltitudeRef")
        .and_then(|tag| match &tag.value {
            TagValue::Unsigned(value) if *value <= 1 => Some(*value),
            _ => None,
        })?;
    if altitude < 0.0 {
        return None;
    }
    Some(if reference == 1 { -altitude } else { altitude })
}

fn gps_direction_degrees(metadata: &Metadata) -> Option<f64> {
    let direction = metadata
        .tags
        .iter()
        .find(|tag| tag.namespace == "GPS" && tag.name == "GPSImgDirection")
        .and_then(|tag| scalar_number(&tag.value))?;
    (0.0..=360.0).contains(&direction).then_some(direction)
}

fn gps_time_seconds(metadata: &Metadata) -> Option<f64> {
    let timestamp = metadata
        .tags
        .iter()
        .find(|tag| tag.namespace == "GPS" && tag.name == "GPSTimeStamp")?;
    let TagValue::Array(values) = &timestamp.value else {
        return None;
    };
    if values.len() != 3 {
        return None;
    }
    let hours = scalar_number(&values[0])?;
    let minutes = scalar_number(&values[1])?;
    let seconds = scalar_number(&values[2])?;
    if !(0.0..24.0).contains(&hours)
        || !(0.0..60.0).contains(&minutes)
        || !(0.0..60.0).contains(&seconds)
    {
        return None;
    }
    Some(hours * 3_600.0 + minutes * 60.0 + seconds)
}

fn gps_speed_meters_per_second(metadata: &Metadata) -> Option<f64> {
    let speed = metadata
        .tags
        .iter()
        .find(|tag| tag.namespace == "GPS" && tag.name == "GPSSpeed")
        .and_then(|tag| scalar_number(&tag.value))?;
    if speed < 0.0 {
        return None;
    }
    let unit = metadata
        .tags
        .iter()
        .find(|tag| tag.namespace == "GPS" && tag.name == "GPSSpeedRef")
        .and_then(|tag| match &tag.value {
            TagValue::String(value) => value.chars().next(),
            _ => None,
        })?;
    let multiplier = match unit {
        'K' | 'k' => 1_000.0 / 3_600.0,
        'M' | 'm' => 1.0,
        'N' | 'n' => 1_852.0 / 3_600.0,
        _ => return None,
    };
    Some(speed * multiplier)
}

fn scalar_number(value: &TagValue) -> Option<f64> {
    let number = match value {
        TagValue::Unsigned(value) => *value as f64,
        TagValue::Signed(value) => *value as f64,
        TagValue::Float(value) => *value,
        TagValue::Rational {
            numerator,
            denominator,
        } if *denominator != 0 => *numerator as f64 / *denominator as f64,
        TagValue::UnsignedRational {
            numerator,
            denominator,
        } if *denominator != 0 => *numerator as f64 / *denominator as f64,
        _ => return None,
    };
    number.is_finite().then_some(number)
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;
    use metra_core::{FileFormat, FileInfo, Tag, TagValue, ValueType};

    fn gps_tag(name: &str, value: TagValue, value_type: ValueType) -> Tag {
        Tag {
            namespace: "GPS".to_owned(),
            group: "GPS".to_owned(),
            id: None,
            name: name.to_owned(),
            description: None,
            raw_value: None,
            value,
            value_type,
            source: Source::default(),
            writable: false,
        }
    }

    fn little_endian_tiff() -> Vec<u8> {
        let mut bytes = vec![
            b'I', b'I', 42, 0, 8, 0, 0, 0, // header and IFD0 offset
            3, 0, // three entries
            0x0F, 0x01, 2, 0, 6, 0, 0, 0, 50, 0, 0, 0, // Make -> offset 50
            0x12, 0x01, 3, 0, 1, 0, 0, 0, 1, 0, 0, 0, // Orientation = 1
            0x69, 0x87, 4, 0, 1, 0, 0, 0, 56, 0, 0, 0, // ExifIFD -> 56
            0, 0, 0, 0, // next IFD
        ];
        bytes.resize(50, 0);
        bytes.extend_from_slice(b"Canon\0");
        bytes.extend_from_slice(&[1, 0]); // Exif IFD count = 1 at offset 56
        bytes.extend_from_slice(&[
            0x03, 0x90, 2, 0, 20, 0, 0, 0, 74, 0, 0, 0, // DateTimeOriginal
        ]);
        bytes.extend_from_slice(&[0, 0, 0, 0]);
        bytes.resize(74, 0);
        bytes.extend_from_slice(b"2026:09:13 12:34:56\0");
        bytes
    }

    fn little_endian_big_tiff() -> Vec<u8> {
        let mut bytes = vec![b'I', b'I', 43, 0, 8, 0, 0, 0, 16, 0, 0, 0, 0, 0, 0, 0];
        bytes.extend_from_slice(&1_u64.to_le_bytes());
        bytes.extend_from_slice(&0x010F_u16.to_le_bytes());
        bytes.extend_from_slice(&2_u16.to_le_bytes());
        bytes.extend_from_slice(&5_u64.to_le_bytes());
        bytes.extend_from_slice(b"Sony\0\0\0\0");
        bytes.extend_from_slice(&0_u64.to_le_bytes());
        bytes
    }

    fn big_endian_big_tiff() -> Vec<u8> {
        let mut bytes = vec![b'M', b'M', 0, 43, 0, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 16];
        bytes.extend_from_slice(&1_u64.to_be_bytes());
        bytes.extend_from_slice(&0x0100_u16.to_be_bytes());
        bytes.extend_from_slice(&16_u16.to_be_bytes());
        bytes.extend_from_slice(&1_u64.to_be_bytes());
        bytes.extend_from_slice(&640_u64.to_be_bytes());
        bytes.extend_from_slice(&0_u64.to_be_bytes());
        bytes
    }

    fn dng_tiff() -> Vec<u8> {
        vec![
            b'I', b'I', 42, 0, 8, 0, 0, 0, 1, 0, // one IFD0 entry
            0x12, 0xC6, 1, 0, 4, 0, 0, 0, 1, 4, 0, 0, // DNGVersion = 1.4.0.0
            0, 0, 0, 0, // no next IFD
        ]
    }

    #[test]
    fn parses_nested_ifd_and_typed_values() {
        let bytes = little_endian_tiff();
        let info = FileInfo::new("fixture.tif".into(), bytes.len() as u64, FileFormat::Tiff);
        let length = bytes.len() as u64;
        let mut metadata = Metadata::new(info);
        parse_tiff_from_reader(
            &mut Cursor::new(bytes),
            0,
            length,
            0,
            &mut metadata,
            ParseLimits::default(),
        )
        .expect("fixture should parse");
        assert_eq!(
            metadata.find("EXIF:Make").unwrap().value,
            TagValue::String("Canon".into())
        );
        assert_eq!(
            metadata.find("EXIF:Orientation").unwrap().value,
            TagValue::Unsigned(1)
        );
        assert_eq!(
            metadata.find("EXIF:DateTimeOriginal").unwrap().value,
            TagValue::DateTime {
                year: 2026,
                month: 9,
                day: 13,
                hour: 12,
                minute: 34,
                second: 56,
                nanosecond: 0,
                offset_minutes: None,
            }
        );
        assert_eq!(tag_definition("EXIF", 0x0100).name, "ImageWidth");
        assert_eq!(tag_definition("EXIF", 0x0102).name, "BitsPerSample");
        assert_eq!(
            tag_definition("EXIF", 0x8831).name,
            "StandardOutputSensitivity"
        );
        assert_eq!(tag_definition("EXIF", 0xA432).name, "LensSpecification");
        assert_eq!(tag_definition("EXIF", 0xA434).name, "LensModel");
    }

    #[test]
    fn parses_thumbnail_ifd_chain_without_decoding_thumbnail_pixels() {
        let mut bytes = vec![
            b'I', b'I', 42, 0, 8, 0, 0, 0, // classic TIFF header, IFD0 at 8
            0, 0, // IFD0 has no entries
            14, 0, 0, 0, // next IFD is IFD1
            2, 0, // IFD1 has width and height
            0x00, 0x01, 4, 0, 1, 0, 0, 0, 160, 0, 0, 0, // ImageWidth = 160
            0x01, 0x01, 4, 0, 1, 0, 0, 0, 120, 0, 0, 0, // ImageLength = 120
            0, 0, 0, 0, // no further IFD
        ];
        bytes.extend_from_slice(&[0xFF, 0xD8, 0xFF, 0xD9]);

        let info = FileInfo::new("thumbnail.tif".into(), bytes.len() as u64, FileFormat::Tiff);
        let metadata = read_tiff(&mut Cursor::new(bytes), info, ParseLimits::default())
            .expect("thumbnail IFD should parse");
        let width = metadata
            .tags
            .iter()
            .find(|tag| tag.group == "IFD1" && tag.name == "ImageWidth")
            .expect("IFD1 width should be present");
        assert_eq!(width.value, TagValue::Unsigned(160));
        assert_eq!(
            metadata
                .tags
                .iter()
                .find(|tag| tag.group == "IFD1" && tag.name == "ImageLength")
                .unwrap()
                .value,
            TagValue::Unsigned(120)
        );
    }

    #[test]
    fn parses_multiple_subifd_offsets_as_separate_groups() {
        let mut bytes = vec![
            b'I', b'I', 42, 0, 8, 0, 0, 0, // classic TIFF header, IFD0 at 8
            1, 0, // one IFD0 entry
            0x4A, 0x01, 4, 0, 2, 0, 0, 0, 26, 0, 0, 0, // SubIFDs -> offset array at 26
            0, 0, 0, 0, // no next IFD
        ];
        bytes.extend_from_slice(&50_u32.to_le_bytes());
        bytes.extend_from_slice(&80_u32.to_le_bytes());
        bytes.resize(50, 0);
        bytes.extend_from_slice(&[1, 0, 0x00, 0x01, 4, 0, 1, 0, 0, 0]);
        bytes.extend_from_slice(&320_u32.to_le_bytes()); // ImageWidth = 320
        bytes.extend_from_slice(&[0, 0, 0, 0]);
        bytes.resize(80, 0);
        bytes.extend_from_slice(&[
            1, 0, // SubIFD2 has one entry
            0x01, 0x01, 4, 0, 1, 0, 0, 0, 240, 0, 0, 0, // ImageLength = 240
            0, 0, 0, 0,
        ]);

        let info = FileInfo::new("subifds.tif".into(), bytes.len() as u64, FileFormat::Tiff);
        let metadata = read_tiff(&mut Cursor::new(bytes), info, ParseLimits::default())
            .expect("SubIFD offsets should parse");
        assert_eq!(
            metadata
                .tags
                .iter()
                .find(|tag| tag.group == "SubIFD1" && tag.name == "ImageWidth")
                .unwrap()
                .value,
            TagValue::Unsigned(320)
        );
        assert_eq!(
            metadata
                .tags
                .iter()
                .find(|tag| tag.group == "SubIFD2" && tag.name == "ImageLength")
                .unwrap()
                .value,
            TagValue::Unsigned(240)
        );
    }

    #[test]
    fn parses_little_endian_bigtiff_ascii_values() {
        let bytes = little_endian_big_tiff();
        let info = FileInfo::new(
            "fixture.bigtiff".into(),
            bytes.len() as u64,
            FileFormat::Tiff,
        );
        let metadata = read_tiff(&mut Cursor::new(bytes), info, ParseLimits::default())
            .expect("little-endian BigTIFF should parse");
        let make = metadata
            .find("EXIF:Make")
            .expect("BigTIFF Make should be present");
        assert_eq!(make.value, TagValue::String("Sony".into()));
        assert_eq!(make.raw_value.as_deref(), Some(b"Sony\0".as_slice()));
        assert_eq!(make.source.length, Some(20));
    }

    #[test]
    fn parses_big_endian_bigtiff_64_bit_values() {
        let bytes = big_endian_big_tiff();
        let info = FileInfo::new(
            "fixture.bigtiff".into(),
            bytes.len() as u64,
            FileFormat::Tiff,
        );
        let metadata = read_tiff(&mut Cursor::new(bytes), info, ParseLimits::default())
            .expect("big-endian BigTIFF should parse");
        assert_eq!(
            metadata.find("EXIF:ImageWidth").unwrap().value,
            TagValue::Unsigned(640)
        );
    }

    #[test]
    fn resolves_common_dng_tag_namespace_and_identifier() {
        let bytes = dng_tiff();
        let info = FileInfo::new("capture.dng".into(), bytes.len() as u64, FileFormat::Tiff);
        let metadata = read_tiff(&mut Cursor::new(bytes), info, ParseLimits::default())
            .expect("DNG TIFF should parse");
        let version = metadata
            .find("DNG:DNGVersion")
            .expect("DNGVersion should be catalogued");
        assert_eq!(version.id, Some(0xC612));
        assert_eq!(
            version.value,
            TagValue::Array(vec![
                TagValue::Unsigned(1),
                TagValue::Unsigned(4),
                TagValue::Unsigned(0),
                TagValue::Unsigned(0),
            ])
        );
    }

    #[test]
    fn discovers_nikon_maker_note_through_exif_ifd() {
        let mut embedded_tiff = vec![b'I', b'I', 42, 0, 8, 0, 0, 0, 1, 0];
        embedded_tiff.extend_from_slice(&[2, 0, 3, 0, 1, 0, 0, 0, 100, 0, 0, 0]);
        embedded_tiff.extend_from_slice(&[0, 0, 0, 0]);
        let mut maker_note = b"Nikon\0\x02\0\0\0".to_vec();
        maker_note.extend_from_slice(&embedded_tiff);

        let value_offset = 64_u32;
        let mut bytes = vec![b'I', b'I', 42, 0, 8, 0, 0, 0, 1, 0];
        bytes.extend_from_slice(&0x927C_u16.to_le_bytes());
        bytes.extend_from_slice(&7_u16.to_le_bytes());
        bytes.extend_from_slice(&(maker_note.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&value_offset.to_le_bytes());
        bytes.extend_from_slice(&[0, 0, 0, 0]);
        bytes.resize(value_offset as usize, 0);
        bytes.extend_from_slice(&maker_note);

        let info = FileInfo::new("nikon.tif".into(), bytes.len() as u64, FileFormat::Tiff);
        let mut metadata = Metadata::new(info);
        parse_tiff_from_reader(
            &mut Cursor::new(bytes),
            0,
            metadata.file_info.size,
            0,
            &mut metadata,
            ParseLimits::default(),
        )
        .expect("Nikon MakerNote fixture should parse");

        let tag = metadata
            .find("MakerNotes:Nikon:ISO")
            .expect("Nikon ISO tag");
        assert_eq!(tag.value, TagValue::Unsigned(100));
        assert_eq!(tag.source.offset, Some(u64::from(value_offset) + 10 + 18));
    }

    #[test]
    fn decodes_ascii_and_unicode_user_comments() {
        let ascii = [b"ASCII\0\0\0".as_slice(), b"reviewed\0".as_slice()].concat();
        assert_eq!(decode_user_comment(&ascii).as_deref(), Some("reviewed"));

        let mut unicode = b"UNICODE\0".to_vec();
        unicode.extend("Métra".encode_utf16().flat_map(u16::to_be_bytes));
        assert_eq!(decode_user_comment(&unicode).as_deref(), Some("Métra"));

        assert_eq!(decode_user_comment(b"JIS\0\0\0\0\0text"), None);
    }

    #[test]
    fn decodes_valid_exif_and_gps_temporal_values() {
        let date_time = decode_special_value(
            "EXIF",
            0x9003,
            2,
            b"2026:09:13 12:34:56\0",
            TagValue::String("2026:09:13 12:34:56".to_owned()),
        );
        assert_eq!(
            date_time,
            TagValue::DateTime {
                year: 2026,
                month: 9,
                day: 13,
                hour: 12,
                minute: 34,
                second: 56,
                nanosecond: 0,
                offset_minutes: None,
            }
        );

        let date = decode_special_value(
            "GPS",
            0x001D,
            2,
            b"2026:09:13\0",
            TagValue::String("2026:09:13".to_owned()),
        );
        assert_eq!(
            date,
            TagValue::Date {
                year: 2026,
                month: 9,
                day: 13,
            }
        );

        let time = decode_special_value(
            "GPS",
            0x0007,
            5,
            &[],
            TagValue::Array(vec![
                TagValue::UnsignedRational {
                    numerator: 12,
                    denominator: 1,
                },
                TagValue::UnsignedRational {
                    numerator: 34,
                    denominator: 1,
                },
                TagValue::UnsignedRational {
                    numerator: 56_125,
                    denominator: 1_000,
                },
            ]),
        );
        assert_eq!(
            time,
            TagValue::Time {
                hour: 12,
                minute: 34,
                second: 56,
                nanosecond: 125_000_000,
            }
        );
    }

    #[test]
    fn parses_user_comment_as_text_and_keeps_raw_value() {
        let comment = [b"ASCII\0\0\0".as_slice(), b"from camera\0".as_slice()].concat();
        let value_offset = 26_u32;
        let mut bytes = vec![b'I', b'I', 42, 0, 8, 0, 0, 0, 1, 0];
        bytes.extend_from_slice(&0x9286_u16.to_le_bytes());
        bytes.extend_from_slice(&7_u16.to_le_bytes());
        bytes.extend_from_slice(&(comment.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&value_offset.to_le_bytes());
        bytes.extend_from_slice(&[0, 0, 0, 0]);
        bytes.extend_from_slice(&comment);

        let mut metadata = Metadata::new(FileInfo::new(
            "user-comment.tif".into(),
            bytes.len() as u64,
            FileFormat::Tiff,
        ));
        parse_tiff_from_reader(
            &mut Cursor::new(bytes),
            0,
            metadata.file_info.size,
            0,
            &mut metadata,
            ParseLimits::default(),
        )
        .expect("UserComment fixture should parse");
        let tag = metadata.find("EXIF:UserComment").expect("UserComment tag");
        assert_eq!(tag.value, TagValue::String("from camera".to_owned()));
        assert_eq!(tag.raw_value, Some(comment));
    }

    #[test]
    fn derives_gps_altitude_direction_and_time_without_losing_signs() {
        let mut metadata = Metadata::new(FileInfo::new("gps.tif".into(), 0, FileFormat::Tiff));
        metadata.add_tag(gps_tag(
            "GPSLatitude",
            TagValue::Array(vec![
                TagValue::UnsignedRational {
                    numerator: 48,
                    denominator: 1,
                },
                TagValue::UnsignedRational {
                    numerator: 51,
                    denominator: 1,
                },
                TagValue::UnsignedRational {
                    numerator: 24,
                    denominator: 1,
                },
            ]),
            ValueType::Array,
        ));
        metadata.add_tag(gps_tag(
            "GPSLatitudeRef",
            TagValue::String("S".to_owned()),
            ValueType::String,
        ));
        metadata.add_tag(gps_tag(
            "GPSLongitude",
            TagValue::Array(vec![
                TagValue::UnsignedRational {
                    numerator: 2,
                    denominator: 1,
                },
                TagValue::UnsignedRational {
                    numerator: 20,
                    denominator: 1,
                },
                TagValue::UnsignedRational {
                    numerator: 0,
                    denominator: 1,
                },
            ]),
            ValueType::Array,
        ));
        metadata.add_tag(gps_tag(
            "GPSLongitudeRef",
            TagValue::String("E".to_owned()),
            ValueType::String,
        ));
        metadata.add_tag(gps_tag(
            "GPSAltitude",
            TagValue::UnsignedRational {
                numerator: 125,
                denominator: 1,
            },
            ValueType::UnsignedRational,
        ));
        metadata.add_tag(gps_tag(
            "GPSAltitudeRef",
            TagValue::Unsigned(1),
            ValueType::UnsignedInteger,
        ));
        metadata.add_tag(gps_tag(
            "GPSImgDirection",
            TagValue::UnsignedRational {
                numerator: 270,
                denominator: 1,
            },
            ValueType::UnsignedRational,
        ));
        metadata.add_tag(gps_tag(
            "GPSTimeStamp",
            TagValue::Array(vec![
                TagValue::UnsignedRational {
                    numerator: 12,
                    denominator: 1,
                },
                TagValue::UnsignedRational {
                    numerator: 34,
                    denominator: 1,
                },
                TagValue::UnsignedRational {
                    numerator: 56,
                    denominator: 1,
                },
            ]),
            ValueType::Array,
        ));
        metadata.add_tag(gps_tag(
            "GPSSpeed",
            TagValue::UnsignedRational {
                numerator: 36,
                denominator: 1,
            },
            ValueType::UnsignedRational,
        ));
        metadata.add_tag(gps_tag(
            "GPSSpeedRef",
            TagValue::String("K".to_owned()),
            ValueType::String,
        ));

        add_gps_decimal_tags(&mut metadata);

        assert!(
            (metadata
                .find("GPS:LatitudeDecimal")
                .unwrap()
                .display_value()
                .parse::<f64>()
                .unwrap()
                + 48.8566666667)
                .abs()
                < 0.000001
        );
        assert_eq!(
            metadata
                .find("GPS:LongitudeDecimal")
                .unwrap()
                .display_value(),
            "2.3333333333333335"
        );
        assert_eq!(
            metadata.find("GPS:AltitudeMeters").unwrap().display_value(),
            "-125"
        );
        assert_eq!(
            metadata
                .find("GPS:ImageDirectionDegrees")
                .unwrap()
                .display_value(),
            "270"
        );
        assert_eq!(
            metadata
                .find("GPS:TimeOfDaySeconds")
                .unwrap()
                .display_value(),
            "45296"
        );
        assert_eq!(
            metadata
                .find("GPS:SpeedMetersPerSecond")
                .unwrap()
                .display_value(),
            "10"
        );
    }

    #[test]
    fn rejects_offset_past_payload_without_allocating() {
        let bytes = vec![
            b'I', b'I', 42, 0, 8, 0, 0, 0, 1, 0, 0x0F, 0x01, 2, 0, 10, 0, 0, 0, 250, 0, 0, 0,
        ];
        let info = FileInfo::new("bad.tif".into(), bytes.len() as u64, FileFormat::Tiff);
        let mut metadata = Metadata::new(info);
        let result = parse_tiff_from_reader(
            &mut Cursor::new(bytes),
            0,
            21,
            0,
            &mut metadata,
            ParseLimits::default(),
        );
        assert!(matches!(result, Err(MetraError::UnexpectedEof { .. })));
    }
}
