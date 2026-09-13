use std::io::{Cursor, Read, Seek, SeekFrom};
use std::path::Path;

use metra_core::{
    FileFormat, FileInfo, Metadata, MetraError, ParseLimits, Result, Source, Tag, TagValue,
    ValueType, Warning,
};

#[derive(Debug, Clone, Copy)]
struct BoxHeader {
    data_start: u64,
    end: u64,
    kind: [u8; 4],
}

pub fn read_isobmff<R: Read + Seek>(
    reader: &mut R,
    file_info: FileInfo,
    limits: ParseLimits,
) -> Result<Metadata> {
    let path = file_info.path.clone();
    let file_length = file_info.size;
    let mut metadata = Metadata::new(file_info);
    if file_length < 8 {
        return Err(MetraError::InvalidHeader {
            context: "ISO-BMFF".to_owned(),
            message: "file is shorter than a box header".to_owned(),
        });
    }
    let mut parser = BoxParser {
        reader,
        path: &path,
        file_length,
        limits,
        bytes_read: 0,
        boxes_read: 0,
    };
    parser.parse_region(0, file_length, 0, &mut metadata)?;
    if metadata.find("ISOBMFF:MajorBrand").is_none() {
        return Err(MetraError::InvalidHeader {
            context: "ISO-BMFF".to_owned(),
            message: "missing ftyp box".to_owned(),
        });
    }
    metadata.sort_tags();
    Ok(metadata)
}

struct BoxParser<'a, R> {
    reader: &'a mut R,
    path: &'a Path,
    file_length: u64,
    limits: ParseLimits,
    bytes_read: usize,
    boxes_read: usize,
}

impl<R: Read + Seek> BoxParser<'_, R> {
    fn parse_region(
        &mut self,
        start: u64,
        end: u64,
        depth: usize,
        metadata: &mut Metadata,
    ) -> Result<()> {
        if depth > self.limits.max_recursion_depth {
            metadata.add_warning(
                Warning::new("isobmff-depth-limit", "ISO-BMFF box nesting limit reached").at(start),
            );
            return Ok(());
        }
        let mut cursor = start;
        while cursor < end {
            if self.boxes_read >= self.limits.max_jpeg_segments {
                metadata.add_warning(
                    Warning::new("isobmff-box-limit", "ISO-BMFF box limit reached").at(cursor),
                );
                return Ok(());
            }
            if end.saturating_sub(cursor) < 8 {
                metadata.add_warning(
                    Warning::new(
                        "truncated-isobmff",
                        "trailing bytes are shorter than a box header",
                    )
                    .at(cursor),
                );
                return Ok(());
            }
            let header = self.read_box_header(cursor, end)?;
            self.boxes_read += 1;
            let kind_name = fourcc(&header.kind);
            if &header.kind == b"ftyp" {
                self.parse_ftyp(&header, metadata)?;
            } else if is_container(&header.kind) {
                let nested_start = if &header.kind == b"meta" {
                    header
                        .data_start
                        .checked_add(4)
                        .ok_or(MetraError::InvalidOffset {
                            context: "ISO-BMFF meta header".to_owned(),
                            offset: header.data_start,
                        })?
                } else {
                    header.data_start
                };
                if nested_start <= header.end {
                    self.parse_region(nested_start, header.end, depth + 1, metadata)?;
                }
            } else if is_text_item(&header.kind) {
                self.parse_text_item(&header, metadata)?;
            } else if &header.kind == b"ispe" {
                self.parse_ispe(&header, metadata)?;
            } else if &header.kind == b"pixi" {
                self.parse_pixi(&header, metadata)?;
            } else if &header.kind == b"irot" {
                self.parse_irot(&header, metadata)?;
            } else if &header.kind == b"imir" {
                self.parse_imir(&header, metadata)?;
            } else if &header.kind == b"pasp" {
                self.parse_pasp(&header, metadata)?;
            } else if &header.kind == b"colr" {
                self.parse_colr(&header, metadata)?;
            } else if &header.kind == b"auxC" {
                self.parse_auxc(&header, metadata)?;
            } else if &header.kind == b"pitm" {
                self.parse_pitm(&header, metadata)?;
            } else if &header.kind == b"hdlr" {
                self.parse_hdlr(&header, metadata)?;
            } else if &header.kind == b"infe" {
                self.parse_infe(&header, metadata)?;
            } else if &header.kind == b"xml " {
                self.parse_xmp(&header, metadata)?;
            } else if &header.kind == b"Exif" {
                self.parse_exif(&header, metadata)?;
            } else if &header.kind == b"uuid" {
                metadata.add_warning(
                    Warning::new(
                        "isobmff-embedded-metadata",
                        format!("ISO-BMFF {kind_name} metadata block is detected but not decoded"),
                    )
                    .at(header.data_start),
                );
            }
            if header.end <= cursor {
                return Err(MetraError::InvalidOffset {
                    context: "ISO-BMFF box progress".to_owned(),
                    offset: cursor,
                });
            }
            cursor = header.end;
        }
        Ok(())
    }

    fn read_box_header(&mut self, start: u64, region_end: u64) -> Result<BoxHeader> {
        let fixed = self.read_at(start, 8, "ISO-BMFF box header")?;
        let size = u64::from(u32::from_be_bytes(fixed[..4].try_into().expect("box size")));
        let kind: [u8; 4] = fixed[4..8].try_into().expect("box kind");
        let (header_size, box_size) = if size == 1 {
            let extended_start = start.checked_add(8).ok_or(MetraError::InvalidOffset {
                context: "ISO-BMFF extended box size".to_owned(),
                offset: start,
            })?;
            let extended = self.read_at(extended_start, 8, "ISO-BMFF extended box size")?;
            (
                16_u64,
                u64::from_be_bytes(extended.try_into().expect("extended size")),
            )
        } else if size == 0 {
            (8_u64, region_end.saturating_sub(start))
        } else {
            (8_u64, size)
        };
        if box_size < header_size {
            return Err(MetraError::InvalidTag {
                context: format!("ISO-BMFF {}", fourcc(&kind)),
                message: "box size is smaller than its header".to_owned(),
            });
        }
        let end = start
            .checked_add(box_size)
            .ok_or(MetraError::InvalidOffset {
                context: format!("ISO-BMFF {} end", fourcc(&kind)),
                offset: start,
            })?;
        if end > region_end || end > self.file_length {
            return Err(MetraError::UnexpectedEof {
                context: format!("ISO-BMFF {} box", fourcc(&kind)),
            });
        }
        Ok(BoxHeader {
            data_start: start + header_size,
            end,
            kind,
        })
    }

    fn parse_ftyp(&mut self, header: &BoxHeader, metadata: &mut Metadata) -> Result<()> {
        let data = self.read_payload(header, "ISO-BMFF ftyp")?;
        if data.len() < 8 {
            return Err(MetraError::InvalidHeader {
                context: "ISO-BMFF ftyp".to_owned(),
                message: "ftyp payload is shorter than major brand and version".to_owned(),
            });
        }
        add_tag(
            metadata,
            "MajorBrand",
            TagValue::String(fourcc(data[..4].try_into().expect("major brand"))),
            header.data_start,
            4,
        );
        add_tag(
            metadata,
            "MinorVersion",
            TagValue::Unsigned(u64::from(u32::from_be_bytes(
                data[4..8].try_into().expect("minor version"),
            ))),
            header.data_start + 4,
            4,
        );
        let compatible = data[8..]
            .chunks_exact(4)
            .map(|brand| TagValue::String(fourcc(brand.try_into().expect("compatible brand"))))
            .collect::<Vec<_>>();
        add_tag(
            metadata,
            "CompatibleBrands",
            TagValue::Array(compatible),
            header.data_start + 8,
            (data.len() - 8) as u64,
        );
        if data[8..].len() % 4 != 0 {
            metadata.add_warning(Warning::new(
                "isobmff-brand-table",
                "ftyp compatible-brand table has trailing bytes",
            ));
        }
        Ok(())
    }

    fn parse_text_item(&mut self, header: &BoxHeader, metadata: &mut Metadata) -> Result<()> {
        let data = self.read_payload(header, "ISO-BMFF metadata item")?;
        let (value, offset) = if data.len() >= 16 && &data[4..8] == b"data" {
            (data[16..].to_vec(), header.data_start + 16)
        } else {
            (data, header.data_start)
        };
        if value.is_empty() {
            return Ok(());
        }
        let name = match &header.kind {
            b"\xA9nam" => "Title",
            b"\xA9ART" => "Artist",
            b"\xA9alb" => "Album",
            b"\xA9day" => "Year",
            b"\xA9cmt" => "Comment",
            b"aART" => "AlbumArtist",
            b"desc" => "Description",
            b"purd" => "PurchaseDate",
            b"too " => "Encoder",
            _ => "Text",
        };
        add_tag(
            metadata,
            name,
            TagValue::String(
                String::from_utf8_lossy(&value)
                    .trim_end_matches('\0')
                    .to_owned(),
            ),
            offset,
            value.len() as u64,
        );
        Ok(())
    }

    fn parse_ispe(&mut self, header: &BoxHeader, metadata: &mut Metadata) -> Result<()> {
        let data = self.read_payload(header, "ISO-BMFF ispe")?;
        if data.len() < 12 {
            metadata.add_warning(
                Warning::new(
                    "truncated-ispe",
                    "ispe box is shorter than its fixed fields",
                )
                .at(header.data_start),
            );
            return Ok(());
        }
        add_tag(
            metadata,
            "ImageWidth",
            TagValue::Unsigned(u64::from(u32::from_be_bytes(
                data[4..8].try_into().expect("ispe width"),
            ))),
            header.data_start + 4,
            4,
        );
        add_tag(
            metadata,
            "ImageHeight",
            TagValue::Unsigned(u64::from(u32::from_be_bytes(
                data[8..12].try_into().expect("ispe height"),
            ))),
            header.data_start + 8,
            4,
        );
        Ok(())
    }

    fn parse_pixi(&mut self, header: &BoxHeader, metadata: &mut Metadata) -> Result<()> {
        let data = self.read_payload(header, "ISO-BMFF pixi")?;
        if data.len() < 5 {
            metadata.add_warning(
                Warning::new(
                    "truncated-pixi",
                    "pixi box is shorter than its channel count",
                )
                .at(header.data_start),
            );
            return Ok(());
        }
        add_tag(
            metadata,
            "ChannelCount",
            TagValue::Unsigned(u64::from(data[4])),
            header.data_start + 4,
            1,
        );
        let bits = data[5..]
            .iter()
            .map(|value| TagValue::Unsigned(u64::from(*value)))
            .collect::<Vec<_>>();
        add_tag(
            metadata,
            "BitsPerChannel",
            TagValue::Array(bits),
            header.data_start + 5,
            (data.len() - 5) as u64,
        );
        Ok(())
    }

    fn parse_irot(&mut self, header: &BoxHeader, metadata: &mut Metadata) -> Result<()> {
        let data = self.read_payload(header, "ISO-BMFF irot")?;
        let Some(value) = data.first() else {
            metadata.add_warning(
                Warning::new("truncated-irot", "irot box has no rotation value")
                    .at(header.data_start),
            );
            return Ok(());
        };
        add_tag(
            metadata,
            "RotationDegrees",
            TagValue::Unsigned(u64::from(value & 0x03) * 90),
            header.data_start,
            1,
        );
        Ok(())
    }

    fn parse_imir(&mut self, header: &BoxHeader, metadata: &mut Metadata) -> Result<()> {
        let data = self.read_payload(header, "ISO-BMFF imir")?;
        let Some(value) = data.first() else {
            metadata.add_warning(
                Warning::new("truncated-imir", "imir box has no mirror axis").at(header.data_start),
            );
            return Ok(());
        };
        let axis = if value & 0x01 == 0 {
            "vertical"
        } else {
            "horizontal"
        };
        add_tag(
            metadata,
            "MirrorAxis",
            TagValue::String(axis.to_owned()),
            header.data_start,
            1,
        );
        Ok(())
    }

    fn parse_pasp(&mut self, header: &BoxHeader, metadata: &mut Metadata) -> Result<()> {
        let data = self.read_payload(header, "ISO-BMFF pasp")?;
        if data.len() < 8 {
            metadata.add_warning(
                Warning::new(
                    "truncated-pasp",
                    "pasp box is shorter than its horizontal and vertical spacing",
                )
                .at(header.data_start),
            );
            return Ok(());
        }
        let horizontal = u64::from(u32::from_be_bytes(
            data[..4].try_into().expect("pasp horizontal"),
        ));
        let vertical = u64::from(u32::from_be_bytes(
            data[4..8].try_into().expect("pasp vertical"),
        ));
        if vertical == 0 {
            metadata.add_warning(
                Warning::new("invalid-pasp", "pasp vertical spacing must not be zero")
                    .at(header.data_start + 4),
            );
            return Ok(());
        }
        add_tag(
            metadata,
            "PixelAspectHorizontal",
            TagValue::Unsigned(horizontal),
            header.data_start,
            4,
        );
        add_tag(
            metadata,
            "PixelAspectVertical",
            TagValue::Unsigned(vertical),
            header.data_start + 4,
            4,
        );
        add_tag(
            metadata,
            "PixelAspectRatio",
            TagValue::Float(horizontal as f64 / vertical as f64),
            header.data_start,
            8,
        );
        Ok(())
    }

    fn parse_colr(&mut self, header: &BoxHeader, metadata: &mut Metadata) -> Result<()> {
        let data = self.read_payload(header, "ISO-BMFF colr")?;
        if data.len() < 4 {
            metadata.add_warning(
                Warning::new("truncated-colr", "colr box is shorter than its color type")
                    .at(header.data_start),
            );
            return Ok(());
        }
        let color_type = fourcc(data[..4].try_into().expect("color type"));
        add_tag(
            metadata,
            "ColorType",
            TagValue::String(color_type.clone()),
            header.data_start,
            4,
        );
        if color_type == "nclx" {
            if data.len() < 11 {
                metadata.add_warning(
                    Warning::new("truncated-colr", "nclx color profile is truncated")
                        .at(header.data_start + 4),
                );
                return Ok(());
            }
            for (name, offset) in [
                ("ColorPrimaries", 4_u64),
                ("TransferCharacteristics", 6_u64),
                ("MatrixCoefficients", 8_u64),
            ] {
                let start = usize::try_from(offset).expect("small colr offset");
                add_tag(
                    metadata,
                    name,
                    TagValue::Unsigned(u64::from(u16::from_be_bytes(
                        data[start..start + 2].try_into().expect("nclx field"),
                    ))),
                    header.data_start + offset,
                    2,
                );
            }
            add_tag(
                metadata,
                "FullRange",
                TagValue::Unsigned(u64::from(data[10] >> 7)),
                header.data_start + 10,
                1,
            );
        }
        Ok(())
    }

    fn parse_auxc(&mut self, header: &BoxHeader, metadata: &mut Metadata) -> Result<()> {
        let data = self.read_payload(header, "ISO-BMFF auxC")?;
        if data.len() <= 4 {
            metadata.add_warning(
                Warning::new("truncated-auxc", "auxC box has no auxiliary type")
                    .at(header.data_start),
            );
            return Ok(());
        }
        let auxiliary_type = String::from_utf8_lossy(&data[4..])
            .trim_end_matches('\0')
            .to_owned();
        add_tag(
            metadata,
            "AuxiliaryType",
            TagValue::String(auxiliary_type),
            header.data_start + 4,
            (data.len() - 4) as u64,
        );
        Ok(())
    }

    fn parse_pitm(&mut self, header: &BoxHeader, metadata: &mut Metadata) -> Result<()> {
        let data = self.read_payload(header, "ISO-BMFF pitm")?;
        if data.len() < 6 {
            metadata.add_warning(
                Warning::new(
                    "truncated-pitm",
                    "pitm box is shorter than its item identifier",
                )
                .at(header.data_start),
            );
            return Ok(());
        }
        let item_id = if data[0] == 0 {
            u64::from(u16::from_be_bytes(
                data[4..6].try_into().expect("pitm item id"),
            ))
        } else if data.len() >= 8 {
            u64::from(u32::from_be_bytes(
                data[4..8].try_into().expect("pitm item id"),
            ))
        } else {
            metadata.add_warning(
                Warning::new(
                    "truncated-pitm",
                    "versioned pitm item identifier is truncated",
                )
                .at(header.data_start),
            );
            return Ok(());
        };
        add_tag(
            metadata,
            "PrimaryItemId",
            TagValue::Unsigned(item_id),
            header.data_start + 4,
            if data[0] == 0 { 2 } else { 4 },
        );
        Ok(())
    }

    fn parse_hdlr(&mut self, header: &BoxHeader, metadata: &mut Metadata) -> Result<()> {
        let data = self.read_payload(header, "ISO-BMFF hdlr")?;
        if data.len() < 12 {
            metadata.add_warning(
                Warning::new(
                    "truncated-hdlr",
                    "hdlr box is shorter than its handler type",
                )
                .at(header.data_start),
            );
            return Ok(());
        }
        add_tag(
            metadata,
            "HandlerType",
            TagValue::String(fourcc(data[8..12].try_into().expect("handler type"))),
            header.data_start + 8,
            4,
        );
        Ok(())
    }

    fn parse_infe(&mut self, header: &BoxHeader, metadata: &mut Metadata) -> Result<()> {
        let data = self.read_payload(header, "ISO-BMFF infe")?;
        if data.len() < 12 {
            metadata.add_warning(
                Warning::new("truncated-infe", "infe box is shorter than its item type")
                    .at(header.data_start),
            );
            return Ok(());
        }
        let version = data[0];
        let (item_id_offset, item_id_length, item_type_offset) = if version == 0 {
            (4_usize, 2_usize, 8_usize)
        } else {
            (4_usize, 4_usize, 12_usize)
        };
        if data.len() < item_type_offset + 4 {
            metadata.add_warning(
                Warning::new("truncated-infe", "infe item type is truncated").at(header.data_start),
            );
            return Ok(());
        }
        let item_id = if item_id_length == 2 {
            u64::from(u16::from_be_bytes(
                data[item_id_offset..item_id_offset + 2]
                    .try_into()
                    .expect("infe item id"),
            ))
        } else {
            u64::from(u32::from_be_bytes(
                data[item_id_offset..item_id_offset + 4]
                    .try_into()
                    .expect("infe item id"),
            ))
        };
        add_tag(
            metadata,
            "ItemId",
            TagValue::Unsigned(item_id),
            header.data_start + item_id_offset as u64,
            item_id_length as u64,
        );
        add_tag(
            metadata,
            "ItemType",
            TagValue::String(fourcc(
                data[item_type_offset..item_type_offset + 4]
                    .try_into()
                    .expect("infe item type"),
            )),
            header.data_start + item_type_offset as u64,
            4,
        );
        Ok(())
    }

    fn parse_xmp(&mut self, header: &BoxHeader, metadata: &mut Metadata) -> Result<()> {
        let data = self.read_payload(header, "ISO-BMFF XMP")?;
        if let Err(error) = crate::xmp::parse_xmp(
            &data,
            header.data_start,
            "ISO-BMFF/xml",
            metadata,
            self.limits,
        ) {
            metadata.add_warning(
                Warning::new("invalid-isobmff-xmp", error.to_string()).at(header.data_start),
            );
        }
        Ok(())
    }

    fn parse_exif(&mut self, header: &BoxHeader, metadata: &mut Metadata) -> Result<()> {
        let data = self.read_payload(header, "ISO-BMFF EXIF")?;
        let Some(tiff_offset) = find_tiff_offset(&data) else {
            metadata.add_warning(
                Warning::new(
                    "invalid-isobmff-exif",
                    "ISO-BMFF Exif box does not contain a TIFF header",
                )
                .at(header.data_start),
            );
            return Ok(());
        };
        let tiff_data = data[tiff_offset..].to_vec();
        let info = FileInfo::new(
            self.path.to_path_buf(),
            tiff_data.len() as u64,
            FileFormat::Tiff,
        );
        match crate::tiff::read_tiff(&mut Cursor::new(tiff_data), info, self.limits) {
            Ok(mut embedded) => {
                for mut tag in embedded.tags.drain(..) {
                    tag.source.container = "ISO-BMFF/Exif".to_owned();
                    tag.source.offset = tag
                        .source
                        .offset
                        .map(|offset| header.data_start + tiff_offset as u64 + offset);
                    metadata.add_tag(tag);
                }
                for mut warning in embedded.warnings.drain(..) {
                    warning.offset = warning
                        .offset
                        .map(|offset| header.data_start + tiff_offset as u64 + offset);
                    metadata.add_warning(warning);
                }
            }
            Err(error) => metadata.add_warning(
                Warning::new("invalid-isobmff-exif", error.to_string()).at(header.data_start),
            ),
        }
        Ok(())
    }

    fn read_payload(&mut self, header: &BoxHeader, context: &str) -> Result<Vec<u8>> {
        let length = header.end.saturating_sub(header.data_start);
        let length = usize::try_from(length).map_err(|_| MetraError::ResourceLimitExceeded {
            resource: context.to_owned(),
            limit: self.limits.max_value_bytes,
        })?;
        if length > self.limits.max_value_bytes {
            return Err(MetraError::ResourceLimitExceeded {
                resource: context.to_owned(),
                limit: self.limits.max_value_bytes,
            });
        }
        self.read_at(header.data_start, length, context)
    }

    fn read_at(&mut self, offset: u64, length: usize, context: &str) -> Result<Vec<u8>> {
        self.bytes_read =
            self.bytes_read
                .checked_add(length)
                .ok_or(MetraError::ResourceLimitExceeded {
                    resource: "ISO-BMFF reads".to_owned(),
                    limit: self.limits.max_metadata_bytes,
                })?;
        if self.bytes_read > self.limits.max_metadata_bytes {
            return Err(MetraError::ResourceLimitExceeded {
                resource: "ISO-BMFF reads".to_owned(),
                limit: self.limits.max_metadata_bytes,
            });
        }
        self.reader
            .seek(SeekFrom::Start(offset))
            .map_err(|source| io_error(self.path, source))?;
        let mut bytes = vec![0_u8; length];
        self.reader
            .read_exact(&mut bytes)
            .map_err(|source| match source.kind() {
                std::io::ErrorKind::UnexpectedEof => MetraError::UnexpectedEof {
                    context: context.to_owned(),
                },
                _ => io_error(self.path, source),
            })?;
        Ok(bytes)
    }
}

fn is_container(kind: &[u8; 4]) -> bool {
    matches!(
        kind,
        b"moov"
            | b"trak"
            | b"mdia"
            | b"minf"
            | b"stbl"
            | b"edts"
            | b"dinf"
            | b"udta"
            | b"meta"
            | b"ilst"
            | b"meco"
            | b"hnti"
            | b"tref"
            | b"iprp"
            | b"ipco"
            | b"iref"
    )
}

fn is_text_item(kind: &[u8; 4]) -> bool {
    matches!(
        kind,
        b"\xA9nam"
            | b"\xA9ART"
            | b"\xA9alb"
            | b"\xA9day"
            | b"\xA9cmt"
            | b"aART"
            | b"desc"
            | b"purd"
            | b"too "
    )
}

fn fourcc(bytes: &[u8; 4]) -> String {
    if bytes
        .iter()
        .all(|byte| byte.is_ascii_graphic() || *byte == b' ')
    {
        String::from_utf8_lossy(bytes).to_string()
    } else {
        format!(
            "0x{:02X}{:02X}{:02X}{:02X}",
            bytes[0], bytes[1], bytes[2], bytes[3]
        )
    }
}

fn find_tiff_offset(data: &[u8]) -> Option<usize> {
    if data.starts_with(b"Exif\0\0") && data.len() > 6 {
        return Some(6);
    }
    if data.len() >= 4 {
        let declared = u32::from_be_bytes(data[..4].try_into().expect("Exif item offset")) as usize;
        let candidate = 4_usize.checked_add(declared)?;
        if is_tiff_header(data.get(candidate..)?) {
            return Some(candidate);
        }
    }
    data.windows(4).position(is_tiff_header)
}

fn is_tiff_header(bytes: &[u8]) -> bool {
    crate::raw::is_tiff_header(bytes)
}

fn add_tag(metadata: &mut Metadata, name: &str, value: TagValue, offset: u64, length: u64) {
    let value_type = match &value {
        TagValue::String(_) => ValueType::String,
        TagValue::Unsigned(_) => ValueType::UnsignedInteger,
        TagValue::Float(_) => ValueType::Float,
        TagValue::UnsignedRational { .. } => ValueType::UnsignedRational,
        TagValue::Array(_) => ValueType::Array,
        _ => ValueType::Unknown,
    };
    metadata.add_tag(Tag {
        namespace: "ISOBMFF".to_owned(),
        group: "Container".to_owned(),
        id: None,
        name: name.to_owned(),
        description: Some("ISO-BMFF container property".to_owned()),
        raw_value: None,
        value,
        value_type,
        source: Source::new("ISO-BMFF", Some(offset), Some(length)),
        writable: false,
    });
}

fn io_error(path: &Path, source: std::io::Error) -> MetraError {
    if source.kind() == std::io::ErrorKind::UnexpectedEof {
        MetraError::UnexpectedEof {
            context: path.display().to_string(),
        }
    } else {
        MetraError::Io {
            path: path.to_path_buf(),
            source,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;
    use metra_core::{FileFormat, FileInfo};

    fn box_with_kind(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let size = u32::try_from(data.len() + 8).expect("test box fits");
        let mut bytes = size.to_be_bytes().to_vec();
        bytes.extend_from_slice(kind);
        bytes.extend_from_slice(data);
        bytes
    }

    #[test]
    fn reads_brand_and_quicktime_title_without_media_payloads() {
        let ftyp = box_with_kind(b"ftyp", b"isom\0\0\0\0mp42");
        let data = box_with_kind(
            b"data",
            &[0, 0, 0, 1, 0, 0, 0, 0, b'M', b'e', b't', b'r', b'a'],
        );
        let title = box_with_kind(b"\xA9nam", &data);
        let ilst = box_with_kind(b"ilst", &title);
        let udta = box_with_kind(b"udta", &ilst);
        let moov = box_with_kind(b"moov", &udta);
        let ispe = box_with_kind(b"ispe", &[0, 0, 0, 0, 0, 0, 3, 0, 0, 0, 2, 0]);
        let pixi = box_with_kind(b"pixi", &[0, 0, 0, 0, 3, 8, 10, 12]);
        let irot = box_with_kind(b"irot", &[2]);
        let imir = box_with_kind(b"imir", &[1]);
        let pasp = box_with_kind(b"pasp", &[0, 0, 0, 4, 0, 0, 0, 3]);
        let colr = box_with_kind(b"colr", b"nclx\0\x01\0\x02\0\x03\x80");
        let mut auxc_data = vec![0, 0, 0, 0];
        auxc_data.extend_from_slice(b"urn:mpeg:avc:auxiliary:alpha\0");
        let auxc = box_with_kind(b"auxC", &auxc_data);
        let mut properties = pixi;
        properties.extend_from_slice(&irot);
        properties.extend_from_slice(&imir);
        properties.extend_from_slice(&pasp);
        properties.extend_from_slice(&colr);
        properties.extend_from_slice(&auxc);
        let iprp = box_with_kind(b"iprp", &box_with_kind(b"ipco", &properties));
        let xmp = box_with_kind(
            b"xml ",
            br#"<x:xmpmeta><rdf:RDF><rdf:Description dc:format="image/heic" xmlns:dc="urn:dc"/></rdf:RDF></x:xmpmeta>"#,
        );
        let mut bytes = ftyp;
        bytes.extend_from_slice(&moov);
        bytes.extend_from_slice(&ispe);
        bytes.extend_from_slice(&iprp);
        bytes.extend_from_slice(&xmp);
        let info = FileInfo::new("movie.mp4".into(), bytes.len() as u64, FileFormat::Mp4);
        let metadata = read_isobmff(&mut Cursor::new(bytes), info, ParseLimits::default()).unwrap();
        assert_eq!(
            metadata.find("ISOBMFF:MajorBrand").unwrap().display_value(),
            "isom"
        );
        assert_eq!(
            metadata.find("ISOBMFF:Title").unwrap().display_value(),
            "Metra"
        );
        assert_eq!(
            metadata.find("ISOBMFF:ImageWidth").unwrap().display_value(),
            "768"
        );
        assert_eq!(
            metadata.find("ISOBMFF:ChannelCount").unwrap().value,
            TagValue::Unsigned(3)
        );
        assert_eq!(
            metadata.find("ISOBMFF:RotationDegrees").unwrap().value,
            TagValue::Unsigned(180)
        );
        assert_eq!(
            metadata.find("ISOBMFF:MirrorAxis").unwrap().display_value(),
            "horizontal"
        );
        assert_eq!(
            metadata.find("ISOBMFF:PixelAspectRatio").unwrap().value,
            TagValue::Float(4.0 / 3.0)
        );
        assert_eq!(
            metadata.find("ISOBMFF:ColorPrimaries").unwrap().value,
            TagValue::Unsigned(1)
        );
        assert_eq!(
            metadata
                .find("ISOBMFF:AuxiliaryType")
                .unwrap()
                .display_value(),
            "urn:mpeg:avc:auxiliary:alpha"
        );
        assert_eq!(
            metadata.find("XMP:dc:format").unwrap().display_value(),
            "image/heic"
        );
    }
}
