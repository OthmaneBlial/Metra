use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use metra_core::{
    FileInfo, Metadata, MetraError, ParseLimits, Result, Source, Tag, TagValue, ValueType, Warning,
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
            } else if &header.kind == b"Exif" || &header.kind == b"xml " {
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

fn add_tag(metadata: &mut Metadata, name: &str, value: TagValue, offset: u64, length: u64) {
    let value_type = match &value {
        TagValue::String(_) => ValueType::String,
        TagValue::Unsigned(_) => ValueType::UnsignedInteger,
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
        let mut bytes = ftyp;
        bytes.extend_from_slice(&moov);
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
    }
}
