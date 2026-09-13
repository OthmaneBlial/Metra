use std::collections::HashSet;
use std::io::{Read, Seek, SeekFrom};

use metra_core::{
    FileInfo, Metadata, MetraError, ParseLimits, Result, Source, Tag, TagValue, ValueType, Warning,
};

use crate::makers::inspect_maker_note;

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
    match (namespace, id) {
        ("EXIF", 0x0100) => TagDefinition {
            namespace: "EXIF",
            name: "ImageWidth",
            description: "Image width in pixels",
        },
        ("EXIF", 0x0101) => TagDefinition {
            namespace: "EXIF",
            name: "ImageLength",
            description: "Image height in pixels",
        },
        ("EXIF", 0x0103) => TagDefinition {
            namespace: "EXIF",
            name: "Compression",
            description: "Image compression scheme",
        },
        ("EXIF", 0x0106) => TagDefinition {
            namespace: "EXIF",
            name: "PhotometricInterpretation",
            description: "Pixel color interpretation",
        },
        ("EXIF", 0x010E) => TagDefinition {
            namespace: "EXIF",
            name: "ImageDescription",
            description: "Image description",
        },
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
        ("EXIF", 0x829A) => TagDefinition {
            namespace: "EXIF",
            name: "ExposureTime",
            description: "Exposure time",
        },
        ("EXIF", 0x829D) => TagDefinition {
            namespace: "EXIF",
            name: "FNumber",
            description: "F-number",
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
        ("EXIF", 0x8822) => TagDefinition {
            namespace: "EXIF",
            name: "ExposureProgram",
            description: "Exposure program",
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
        ("EXIF", 0x9286) => TagDefinition {
            namespace: "EXIF",
            name: "UserComment",
            description: "User comment with an EXIF character-code prefix",
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
        ("EXIF", 0xA001) => TagDefinition {
            namespace: "EXIF",
            name: "ColorSpace",
            description: "Color space information",
        },
        ("EXIF", 0xA405) => TagDefinition {
            namespace: "EXIF",
            name: "FocalLengthIn35mmFormat",
            description: "Equivalent focal length in 35mm film",
        },
        ("EXIF", 0xA433) => TagDefinition {
            namespace: "EXIF",
            name: "LensMake",
            description: "Lens manufacturer",
        },
        ("EXIF", 0xA434) => TagDefinition {
            namespace: "EXIF",
            name: "LensModel",
            description: "Lens model",
        },
        ("EXIF", 0xA435) => TagDefinition {
            namespace: "EXIF",
            name: "LensSerialNumber",
            description: "Lens serial number",
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
        ("GPS", 0x000C) => TagDefinition {
            namespace: "GPS",
            name: "GPSSpeedRef",
            description: "GPS speed unit",
        },
        ("GPS", 0x000D) => TagDefinition {
            namespace: "GPS",
            name: "GPSSpeed",
            description: "GPS speed",
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
            42 => (
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
                    message: format!("expected magic 42 or BigTIFF magic 43, got {magic}"),
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

        if let Some((bytes, maker_note_offset)) = maker_note {
            inspect_maker_note(&bytes, maker_note_offset, metadata, self.limits);
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
    value
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
        assert_eq!(tag_definition("EXIF", 0x0100).name, "ImageWidth");
        assert_eq!(tag_definition("EXIF", 0xA434).name, "LensModel");
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
