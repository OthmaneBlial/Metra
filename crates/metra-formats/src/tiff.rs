use std::collections::HashSet;
use std::io::{Read, Seek, SeekFrom};

use metra_core::{
    FileInfo, Metadata, MetraError, ParseLimits, Result, Source, Tag, TagValue, ValueType, Warning,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Endian {
    Little,
    Big,
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
}

#[derive(Debug, Clone, Copy)]
struct TagDefinition {
    namespace: &'static str,
    name: &'static str,
    description: &'static str,
}

fn tag_definition(namespace: &str, id: u16) -> TagDefinition {
    match (namespace, id) {
        ("EXIF", 0x010F) => TagDefinition {
            namespace: "EXIF",
            name: "Make",
            description: "Camera manufacturer",
        },
        ("EXIF", 0x0110) => TagDefinition {
            namespace: "EXIF",
            name: "Model",
            description: "Camera model",
        },
        ("EXIF", 0x0112) => TagDefinition {
            namespace: "EXIF",
            name: "Orientation",
            description: "Image orientation",
        },
        ("EXIF", 0x011A) => TagDefinition {
            namespace: "EXIF",
            name: "XResolution",
            description: "Horizontal resolution",
        },
        ("EXIF", 0x011B) => TagDefinition {
            namespace: "EXIF",
            name: "YResolution",
            description: "Vertical resolution",
        },
        ("EXIF", 0x0128) => TagDefinition {
            namespace: "EXIF",
            name: "ResolutionUnit",
            description: "Resolution unit",
        },
        ("EXIF", 0x0131) => TagDefinition {
            namespace: "EXIF",
            name: "Software",
            description: "Software used to create the file",
        },
        ("EXIF", 0x0132) => TagDefinition {
            namespace: "EXIF",
            name: "ModifyDate",
            description: "File modification date from EXIF",
        },
        ("EXIF", 0x013B) => TagDefinition {
            namespace: "EXIF",
            name: "Artist",
            description: "Person who created the image",
        },
        ("EXIF", 0x0201) => TagDefinition {
            namespace: "EXIF",
            name: "JPEGInterchangeFormat",
            description: "Thumbnail offset",
        },
        ("EXIF", 0x0202) => TagDefinition {
            namespace: "EXIF",
            name: "JPEGInterchangeFormatLength",
            description: "Thumbnail length",
        },
        ("EXIF", 0x8298) => TagDefinition {
            namespace: "EXIF",
            name: "Copyright",
            description: "Copyright notice",
        },
        ("EXIF", 0x8769) => TagDefinition {
            namespace: "EXIF",
            name: "ExifIFDPointer",
            description: "Offset to the EXIF IFD",
        },
        ("EXIF", 0x8825) => TagDefinition {
            namespace: "EXIF",
            name: "GPSInfoIFDPointer",
            description: "Offset to the GPS IFD",
        },
        ("EXIF", 0x8827) => TagDefinition {
            namespace: "EXIF",
            name: "ISO",
            description: "ISO speed rating",
        },
        ("EXIF", 0x9000) => TagDefinition {
            namespace: "EXIF",
            name: "ExifVersion",
            description: "EXIF specification version",
        },
        ("EXIF", 0x9003) => TagDefinition {
            namespace: "EXIF",
            name: "DateTimeOriginal",
            description: "Original capture date and time",
        },
        ("EXIF", 0x9004) => TagDefinition {
            namespace: "EXIF",
            name: "CreateDate",
            description: "Digitized date and time",
        },
        ("EXIF", 0x9201) => TagDefinition {
            namespace: "EXIF",
            name: "ShutterSpeedValue",
            description: "Shutter speed value",
        },
        ("EXIF", 0x9202) => TagDefinition {
            namespace: "EXIF",
            name: "ApertureValue",
            description: "Aperture value",
        },
        ("EXIF", 0x9204) => TagDefinition {
            namespace: "EXIF",
            name: "ExposureCompensation",
            description: "Exposure bias value",
        },
        ("EXIF", 0x9207) => TagDefinition {
            namespace: "EXIF",
            name: "MeteringMode",
            description: "Metering mode",
        },
        ("EXIF", 0x9209) => TagDefinition {
            namespace: "EXIF",
            name: "Flash",
            description: "Flash status",
        },
        ("EXIF", 0x920A) => TagDefinition {
            namespace: "EXIF",
            name: "FocalLength",
            description: "Lens focal length",
        },
        ("EXIF", 0x927C) => TagDefinition {
            namespace: "EXIF",
            name: "MakerNote",
            description: "Manufacturer-specific metadata block",
        },
        ("EXIF", 0x9291) => TagDefinition {
            namespace: "EXIF",
            name: "SubSecTimeOriginal",
            description: "Sub-second capture time",
        },
        ("EXIF", 0xA002) => TagDefinition {
            namespace: "EXIF",
            name: "PixelXDimension",
            description: "Image width",
        },
        ("EXIF", 0xA003) => TagDefinition {
            namespace: "EXIF",
            name: "PixelYDimension",
            description: "Image height",
        },
        ("GPS", 0x0000) => TagDefinition {
            namespace: "GPS",
            name: "GPSVersionID",
            description: "GPS metadata version",
        },
        ("GPS", 0x0001) => TagDefinition {
            namespace: "GPS",
            name: "GPSLatitudeRef",
            description: "North or south latitude reference",
        },
        ("GPS", 0x0002) => TagDefinition {
            namespace: "GPS",
            name: "GPSLatitude",
            description: "Latitude in degrees, minutes, seconds",
        },
        ("GPS", 0x0003) => TagDefinition {
            namespace: "GPS",
            name: "GPSLongitudeRef",
            description: "East or west longitude reference",
        },
        ("GPS", 0x0004) => TagDefinition {
            namespace: "GPS",
            name: "GPSLongitude",
            description: "Longitude in degrees, minutes, seconds",
        },
        ("GPS", 0x0005) => TagDefinition {
            namespace: "GPS",
            name: "GPSAltitudeRef",
            description: "Altitude reference",
        },
        ("GPS", 0x0006) => TagDefinition {
            namespace: "GPS",
            name: "GPSAltitude",
            description: "Altitude",
        },
        ("GPS", 0x0007) => TagDefinition {
            namespace: "GPS",
            name: "GPSTimeStamp",
            description: "GPS time of day",
        },
        ("GPS", 0x0010) => TagDefinition {
            namespace: "GPS",
            name: "GPSImgDirectionRef",
            description: "Image direction reference",
        },
        ("GPS", 0x0011) => TagDefinition {
            namespace: "GPS",
            name: "GPSImgDirection",
            description: "Image direction",
        },
        ("GPS", 0x001D) => TagDefinition {
            namespace: "GPS",
            name: "GPSDateStamp",
            description: "GPS date",
        },
        ("Interop", 0x0001) => TagDefinition {
            namespace: "Interop",
            name: "InteroperabilityIndex",
            description: "Interoperability identifier",
        },
        _ => TagDefinition {
            namespace: match namespace {
                "GPS" => "GPS",
                "Interop" => "Interop",
                _ => "EXIF",
            },
            name: "Unknown",
            description: "Unknown TIFF tag",
        },
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
        if magic != 42 {
            return Err(MetraError::InvalidHeader {
                context: "TIFF".to_owned(),
                message: format!("expected magic 42, got {magic}"),
            });
        }
        let first_ifd = u64::from(self.endian.u32(&header[4..8]));
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

        let count_bytes = self.read_at(offset, 2, "IFD entry count")?;
        let count = usize::from(self.endian.u16(&count_bytes));
        let count_to_read = count.min(self.limits.max_ifd_entries);
        if count > count_to_read {
            metadata.add_warning(
                Warning::new(
                    "ifd-entry-limit",
                    format!("IFD declares {count} entries; reading only {count_to_read}"),
                )
                .at(self.absolute_start.saturating_add(offset)),
            );
        }

        let entries_start = offset.checked_add(2).ok_or(MetraError::InvalidOffset {
            context: "IFD entries".to_owned(),
            offset,
        })?;
        for index in 0..count_to_read {
            let index_offset = u64::try_from(index).map_err(|_| MetraError::InvalidOffset {
                context: "IFD entry index".to_owned(),
                offset: index as u64,
            })?;
            let entry_offset = entries_start
                .checked_add(
                    index_offset
                        .checked_mul(12)
                        .ok_or(MetraError::InvalidOffset {
                            context: "IFD entry offset".to_owned(),
                            offset: index_offset,
                        })?,
                )
                .ok_or(MetraError::InvalidOffset {
                    context: "IFD entry offset".to_owned(),
                    offset: entries_start,
                })?;
            let entry = self.read_at(entry_offset, 12, "IFD entry")?;
            self.parse_entry(&entry, entry_offset, namespace, group, depth, metadata)?;
        }

        let next_offset_position = entries_start
            .checked_add(
                u64::try_from(count_to_read)
                    .map_err(|_| MetraError::InvalidOffset {
                        context: "next IFD pointer".to_owned(),
                        offset,
                    })?
                    .checked_mul(12)
                    .ok_or(MetraError::InvalidOffset {
                        context: "next IFD pointer".to_owned(),
                        offset,
                    })?,
            )
            .ok_or(MetraError::InvalidOffset {
                context: "next IFD pointer".to_owned(),
                offset,
            })?;
        if count == count_to_read {
            let next = self.read_at(next_offset_position, 4, "next IFD pointer")?;
            let next = u64::from(self.endian.u32(&next));
            if next != 0 {
                self.parse_ifd(next, namespace, "IFD-next", depth, metadata)?;
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
        let count = u64::from(self.endian.u32(&entry[4..8]));
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
                    Some(12),
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
                let value_bytes = if total_size_usize <= 4 {
                    entry[8..8 + total_size_usize].to_vec()
                } else {
                    let value_offset = u64::from(self.endian.u32(&entry[8..12]));
                    self.read_at(value_offset, total_size_usize, "tag value")?
                };
                let value = decode_value(type_id, count, &value_bytes, self.endian)?;
                let value_type = value_type(&value);
                (value, Some(value_bytes), value_type)
            };

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
            raw_value,
            value,
            value_type,
            source: Source::new(
                format!("TIFF/{group}"),
                Some(self.absolute_start.saturating_add(entry_offset)),
                Some(12),
            ),
            writable: false,
        });

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
        5 | 10 | 12 => Some(8),
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
    let mut decimal = parts[0] + parts[1] / 60.0 + parts[2] / 3_600.0;
    if matches!(reference, 'S' | 'W' | 's' | 'w') {
        decimal = -decimal;
    }
    Some((decimal, Source::new("derived/GPS", None, None)))
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;
    use metra_core::{FileFormat, FileInfo, TagValue};

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
            TagValue::String("2026:09:13 12:34:56".into())
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
