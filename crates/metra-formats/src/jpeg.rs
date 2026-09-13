use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use metra_core::{
    FileInfo, Metadata, MetraError, ParseLimits, Result, Source, Tag, TagValue, ValueType, Warning,
};

use crate::tiff::parse_tiff_from_reader;
use crate::xmp::parse_xmp;

pub fn read_jpeg<R: Read + Seek>(
    reader: &mut R,
    file_info: FileInfo,
    limits: ParseLimits,
) -> Result<Metadata> {
    let path = file_info.path.clone();
    let mut metadata = Metadata::new(file_info);
    let mut soi = [0_u8; 2];
    read_exact(reader, &mut soi, &path)?;
    if soi != [0xFF, 0xD8] {
        return Err(MetraError::InvalidHeader {
            context: "JPEG".to_owned(),
            message: "missing SOI marker".to_owned(),
        });
    }

    let mut offset = 2_u64;
    let mut segments = 0_usize;
    let mut consumed_metadata = 0_usize;
    loop {
        if segments >= limits.max_jpeg_segments {
            metadata.add_warning(
                Warning::new(
                    "jpeg-segment-limit",
                    format!("stopped after {} JPEG segments", limits.max_jpeg_segments),
                )
                .at(offset),
            );
            break;
        }
        let marker = match read_marker(reader, &mut offset, &path)? {
            Some(marker) => marker,
            None => {
                metadata.add_warning(Warning::new(
                    "truncated-jpeg",
                    "JPEG ended before an EOI or SOS marker",
                ));
                break;
            }
        };
        if marker == 0xD9 || marker == 0xDA {
            break;
        }
        if (0xD0..=0xD7).contains(&marker) || marker == 0x01 {
            segments += 1;
            continue;
        }

        let mut length_bytes = [0_u8; 2];
        read_exact(reader, &mut length_bytes, &path)?;
        offset = offset.checked_add(2).ok_or(MetraError::InvalidOffset {
            context: "JPEG segment length".to_owned(),
            offset,
        })?;
        let segment_length = usize::from(u16::from_be_bytes(length_bytes));
        if segment_length < 2 {
            return Err(MetraError::InvalidTag {
                context: format!("JPEG marker 0xFF{marker:02X}"),
                message: "segment length must include its two length bytes".to_owned(),
            });
        }
        let data_length = segment_length - 2;
        let data_offset = offset;
        let remaining_budget = limits.max_metadata_bytes.saturating_sub(consumed_metadata);
        if data_length > remaining_budget {
            metadata.add_warning(
                Warning::new(
                    "jpeg-metadata-limit",
                    format!("skipped {data_length}-byte segment after metadata budget was reached"),
                )
                .at(data_offset),
            );
            seek_forward(reader, data_length, &path)?;
            offset = offset
                .checked_add(u64::try_from(data_length).unwrap_or(u64::MAX))
                .ok_or(MetraError::InvalidOffset {
                    context: "JPEG segment end".to_owned(),
                    offset: data_offset,
                })?;
            segments += 1;
            continue;
        }

        let mut data = vec![0_u8; data_length];
        read_exact(reader, &mut data, &path)?;
        consumed_metadata = consumed_metadata.saturating_add(data_length);
        offset = offset
            .checked_add(
                u64::try_from(data_length).map_err(|_| MetraError::InvalidOffset {
                    context: "JPEG segment length".to_owned(),
                    offset: data_length as u64,
                })?,
            )
            .ok_or(MetraError::InvalidOffset {
                context: "JPEG segment end".to_owned(),
                offset: data_offset,
            })?;
        process_segment(marker, &data, data_offset, &mut metadata, limits)?;
        segments += 1;
    }
    metadata.sort_tags();
    Ok(metadata)
}

fn read_marker<R: Read>(reader: &mut R, offset: &mut u64, path: &Path) -> Result<Option<u8>> {
    let mut byte = [0_u8; 1];
    match reader.read_exact(&mut byte) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => {
            return Ok(None);
        }
        Err(error) => return Err(io_error(path, error)),
    }
    *offset = offset.saturating_add(1);
    if byte[0] != 0xFF {
        return Err(MetraError::CorruptMetadata {
            message: format!(
                "expected JPEG marker at offset {}",
                offset.saturating_sub(1)
            ),
        });
    }
    loop {
        read_exact(reader, &mut byte, path)?;
        *offset = offset.saturating_add(1);
        if byte[0] != 0xFF {
            break;
        }
    }
    if byte[0] == 0x00 {
        return Err(MetraError::CorruptMetadata {
            message: "entropy-coded data appeared before SOS".to_owned(),
        });
    }
    Ok(Some(byte[0]))
}

fn process_segment(
    marker: u8,
    data: &[u8],
    data_offset: u64,
    metadata: &mut Metadata,
    limits: ParseLimits,
) -> Result<()> {
    match marker {
        0xE1 if data.starts_with(b"Exif\0\0") => {
            if data.len() <= 6 {
                metadata.add_warning(Warning::new(
                    "empty-exif",
                    "JPEG APP1 segment has an EXIF header but no TIFF payload",
                ));
                return Ok(());
            }
            let tiff_data = &data[6..];
            let mut cursor = std::io::Cursor::new(tiff_data);
            if let Err(error) = parse_tiff_from_reader(
                &mut cursor,
                0,
                tiff_data.len() as u64,
                data_offset.saturating_add(6),
                metadata,
                limits,
            ) {
                metadata.add_warning(
                    Warning::new("invalid-exif", error.to_string()).at(data_offset + 6),
                );
            }
        }
        0xE1 if data.starts_with(b"http://ns.adobe.com/xap/1.0/\0") => {
            let prefix_len = b"http://ns.adobe.com/xap/1.0/\0".len();
            if let Err(error) = parse_xmp(
                &data[prefix_len..],
                data_offset.saturating_add(prefix_len as u64),
                "JPEG/APP1-XMP",
                metadata,
                limits,
            ) {
                metadata
                    .add_warning(Warning::new("invalid-xmp", error.to_string()).at(data_offset));
            }
        }
        0xE2 if data.starts_with(b"ICC_PROFILE\0") => metadata.add_warning(
            Warning::new(
                "unsupported-icc",
                "JPEG contains an ICC profile; ICC parsing is planned",
            )
            .at(data_offset),
        ),
        0xED if data.starts_with(b"Photoshop 3.0\0") => metadata.add_warning(
            Warning::new(
                "unsupported-photoshop",
                "JPEG contains Photoshop resources; Photoshop/IPTC parsing is planned",
            )
            .at(data_offset),
        ),
        0xE0 if data.starts_with(b"JFIF\0") => parse_jfif(data, data_offset, metadata),
        0xFE => {
            let value = String::from_utf8_lossy(data).into_owned();
            metadata.add_tag(Tag {
                namespace: "JPEG".to_owned(),
                group: "COM".to_owned(),
                id: None,
                name: "Comment".to_owned(),
                description: Some("JPEG comment".to_owned()),
                raw_value: Some(data.to_vec()),
                value: TagValue::String(value),
                value_type: ValueType::String,
                source: Source::new("JPEG/COM", Some(data_offset), Some(data.len() as u64)),
                writable: false,
            });
        }
        _ => {}
    }
    Ok(())
}

fn parse_jfif(data: &[u8], data_offset: u64, metadata: &mut Metadata) {
    if data.len() < 14 {
        metadata.add_warning(
            Warning::new(
                "truncated-jfif",
                "JFIF segment is shorter than its fixed header",
            )
            .at(data_offset),
        );
        return;
    }
    let version = format!("{}.{}", data[5], data[6]);
    let units = match data[7] {
        0 => "None",
        1 => "inches",
        2 => "cm",
        other => {
            metadata.add_warning(
                Warning::new(
                    "invalid-jfif-units",
                    format!("unknown JFIF density unit {other}"),
                )
                .at(data_offset.saturating_add(7)),
            );
            "Unknown"
        }
    };
    add_jpeg_tag(
        metadata,
        "JFIFVersion",
        TagValue::String(version),
        ValueType::String,
        data_offset.saturating_add(5),
        2,
    );
    add_jpeg_tag(
        metadata,
        "ResolutionUnit",
        TagValue::String(units.to_owned()),
        ValueType::String,
        data_offset.saturating_add(7),
        1,
    );
    add_jpeg_tag(
        metadata,
        "XResolution",
        TagValue::Unsigned(u64::from(u16::from_be_bytes([data[8], data[9]]))),
        ValueType::UnsignedInteger,
        data_offset.saturating_add(8),
        2,
    );
    add_jpeg_tag(
        metadata,
        "YResolution",
        TagValue::Unsigned(u64::from(u16::from_be_bytes([data[10], data[11]]))),
        ValueType::UnsignedInteger,
        data_offset.saturating_add(10),
        2,
    );
}

fn add_jpeg_tag(
    metadata: &mut Metadata,
    name: &str,
    value: TagValue,
    value_type: ValueType,
    offset: u64,
    length: u64,
) {
    metadata.add_tag(Tag {
        namespace: "JFIF".to_owned(),
        group: "APP0".to_owned(),
        id: None,
        name: name.to_owned(),
        description: Some("JFIF container property".to_owned()),
        raw_value: None,
        value,
        value_type,
        source: Source::new("JPEG/APP0", Some(offset), Some(length)),
        writable: false,
    });
}

fn seek_forward<R: Seek>(reader: &mut R, length: usize, path: &Path) -> Result<()> {
    let distance = i64::try_from(length).map_err(|_| MetraError::InvalidOffset {
        context: "JPEG segment skip".to_owned(),
        offset: length as u64,
    })?;
    reader
        .seek(SeekFrom::Current(distance))
        .map(|_| ())
        .map_err(|source| io_error(path, source))
}

fn read_exact<R: Read>(reader: &mut R, buffer: &mut [u8], path: &Path) -> Result<()> {
    reader
        .read_exact(buffer)
        .map_err(|source| io_error(path, source))
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
    use metra_core::FileFormat;

    fn jpeg_with_comment() -> Vec<u8> {
        let comment = b"hello from Metra";
        let length = u16::try_from(comment.len() + 2).unwrap();
        let mut bytes = vec![0xFF, 0xD8, 0xFF, 0xFE];
        bytes.extend_from_slice(&length.to_be_bytes());
        bytes.extend_from_slice(comment);
        bytes.extend_from_slice(&[0xFF, 0xD9]);
        bytes
    }

    #[test]
    fn reads_jpeg_comment_without_decoding_pixels() {
        let bytes = jpeg_with_comment();
        let info = FileInfo::new("comment.jpg".into(), bytes.len() as u64, FileFormat::Jpeg);
        let metadata = read_jpeg(&mut Cursor::new(bytes), info, ParseLimits::default()).unwrap();
        assert_eq!(
            metadata.find("JPEG:Comment").unwrap().display_value(),
            "hello from Metra"
        );
    }

    #[test]
    fn reads_structured_xmp_from_app1() {
        let packet = br#"<x:xmpmeta><rdf:RDF><rdf:Description dc:format="image/jpeg" xmlns:dc="urn:dc"/></rdf:RDF></x:xmpmeta>"#;
        let mut data = b"http://ns.adobe.com/xap/1.0/\0".to_vec();
        data.extend_from_slice(packet);
        let length = u16::try_from(data.len() + 2).unwrap();
        let mut bytes = vec![0xFF, 0xD8, 0xFF, 0xE1];
        bytes.extend_from_slice(&length.to_be_bytes());
        bytes.extend_from_slice(&data);
        bytes.extend_from_slice(&[0xFF, 0xD9]);
        let info = FileInfo::new("xmp.jpg".into(), bytes.len() as u64, FileFormat::Jpeg);
        let metadata = read_jpeg(&mut Cursor::new(bytes), info, ParseLimits::default()).unwrap();
        assert_eq!(
            metadata.find("XMP:dc:format").unwrap().display_value(),
            "image/jpeg"
        );
    }

    #[test]
    fn malformed_segment_length_is_rejected() {
        let bytes = vec![0xFF, 0xD8, 0xFF, 0xE1, 0, 1];
        let info = FileInfo::new("bad.jpg".into(), bytes.len() as u64, FileFormat::Jpeg);
        let result = read_jpeg(&mut Cursor::new(bytes), info, ParseLimits::default());
        assert!(matches!(result, Err(MetraError::InvalidTag { .. })));
    }
}
