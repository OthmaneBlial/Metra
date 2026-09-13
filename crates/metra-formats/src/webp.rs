use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use metra_core::{
    FileInfo, Metadata, MetraError, ParseLimits, Result, Source, Tag, TagValue, ValueType, Warning,
};

use crate::icc::parse_icc_profile;
use crate::tiff::parse_tiff_from_reader;
use crate::xmp::parse_xmp;

pub fn read_webp<R: Read + Seek>(
    reader: &mut R,
    file_info: FileInfo,
    limits: ParseLimits,
) -> Result<Metadata> {
    let path = file_info.path.clone();
    let file_length = file_info.size;
    let mut metadata = Metadata::new(file_info);
    let mut header = [0_u8; 12];
    read_exact(reader, &mut header, &path)?;
    if &header[..4] != b"RIFF" || &header[8..12] != b"WEBP" {
        return Err(MetraError::InvalidHeader {
            context: "WebP".to_owned(),
            message: "missing RIFF/WEBP header".to_owned(),
        });
    }
    let riff_size = u64::from(u32::from_le_bytes(
        header[4..8].try_into().expect("RIFF size"),
    ));
    let declared_end = riff_size.saturating_add(8);
    if declared_end > file_length {
        metadata.add_warning(Warning::new(
            "truncated-riff",
            format!("RIFF declares {declared_end} bytes but file has {file_length}"),
        ));
    }

    let mut offset = 12_u64;
    let mut chunks = 0_usize;
    let mut consumed_metadata = 0_usize;
    while offset.saturating_add(8) <= file_length && chunks < limits.max_jpeg_segments {
        let mut chunk_header = [0_u8; 8];
        read_exact(reader, &mut chunk_header, &path)?;
        offset = checked_add(offset, 8, "WebP chunk header")?;
        let kind: [u8; 4] = chunk_header[..4].try_into().expect("WebP chunk type");
        let name = String::from_utf8_lossy(&kind).into_owned();
        let data_length = u64::from(u32::from_le_bytes(
            chunk_header[4..8].try_into().expect("WebP chunk length"),
        ));
        let data_end = data_length
            .checked_add(offset)
            .ok_or(MetraError::InvalidOffset {
                context: format!("WebP {name} chunk end"),
                offset,
            })?;
        if data_end > file_length {
            return Err(MetraError::UnexpectedEof {
                context: format!("WebP {name} chunk"),
            });
        }
        let remaining = limits.max_metadata_bytes.saturating_sub(consumed_metadata);
        if data_length > remaining as u64 {
            metadata.add_warning(
                Warning::new(
                    "webp-metadata-limit",
                    format!("skipped {data_length}-byte {name} chunk"),
                )
                .at(offset),
            );
            seek_forward(reader, data_length, &path)?;
            consumed_metadata = limits.max_metadata_bytes;
        } else {
            let data_length_usize =
                usize::try_from(data_length).map_err(|_| MetraError::ResourceLimitExceeded {
                    resource: format!("WebP {name} chunk"),
                    limit: limits.max_metadata_bytes,
                })?;
            let mut data = vec![0_u8; data_length_usize];
            read_exact(reader, &mut data, &path)?;
            consumed_metadata = consumed_metadata.saturating_add(data_length_usize);
            process_chunk(&kind, &data, offset, &mut metadata, limits)?;
        }
        offset = checked_add(offset, data_length, "WebP chunk data")?;
        if data_length & 1 == 1 {
            read_exact(reader, &mut [0_u8; 1], &path)?;
            offset = checked_add(offset, 1, "WebP chunk padding")?;
        }
        chunks += 1;
        if offset >= declared_end {
            break;
        }
    }
    if chunks >= limits.max_jpeg_segments {
        metadata.add_warning(Warning::new(
            "webp-chunk-limit",
            format!("stopped after {} WebP chunks", limits.max_jpeg_segments),
        ));
    }
    metadata.sort_tags();
    Ok(metadata)
}

fn process_chunk(
    kind: &[u8; 4],
    data: &[u8],
    data_offset: u64,
    metadata: &mut Metadata,
    limits: ParseLimits,
) -> Result<()> {
    match kind {
        b"VP8X" => parse_vp8x(data, data_offset, metadata),
        b"VP8 " => parse_vp8(data, data_offset, metadata),
        b"VP8L" => parse_vp8l(data, data_offset, metadata),
        b"EXIF" => {
            let (tiff_data, absolute_start) = if data.starts_with(b"Exif\0\0") {
                (&data[6..], data_offset + 6)
            } else {
                (data, data_offset)
            };
            if tiff_data.len() < 8 {
                metadata.add_warning(
                    Warning::new(
                        "truncated-exif",
                        "WebP EXIF chunk is shorter than a TIFF header",
                    )
                    .at(data_offset),
                );
            } else {
                let mut cursor = std::io::Cursor::new(tiff_data);
                if let Err(error) = parse_tiff_from_reader(
                    &mut cursor,
                    0,
                    tiff_data.len() as u64,
                    absolute_start,
                    metadata,
                    limits,
                ) {
                    metadata.add_warning(
                        Warning::new("invalid-exif", error.to_string()).at(absolute_start),
                    );
                }
            }
        }
        b"XMP " => {
            if let Err(error) = parse_xmp(data, data_offset, "WebP/XMP", metadata, limits) {
                metadata
                    .add_warning(Warning::new("invalid-xmp", error.to_string()).at(data_offset));
            }
        }
        b"ICCP" => {
            if let Err(error) = parse_icc_profile(data, data_offset, metadata, limits) {
                metadata
                    .add_warning(Warning::new("invalid-icc", error.to_string()).at(data_offset));
            }
        }
        _ => {}
    }
    Ok(())
}

fn parse_vp8x(data: &[u8], data_offset: u64, metadata: &mut Metadata) {
    if data.len() < 10 {
        metadata.add_warning(
            Warning::new("truncated-vp8x", "WebP VP8X chunk is shorter than 10 bytes")
                .at(data_offset),
        );
        return;
    }
    let width = 1 + u32::from(data[4]) + (u32::from(data[5]) << 8) + (u32::from(data[6]) << 16);
    let height = 1 + u32::from(data[7]) + (u32::from(data[8]) << 8) + (u32::from(data[9]) << 16);
    add_dimensions(
        metadata,
        "VP8X",
        "Canvas dimension",
        "WebP/VP8X",
        data_offset,
        [("ImageWidth", width, 4, 3), ("ImageHeight", height, 7, 3)],
    );
}

fn parse_vp8(data: &[u8], data_offset: u64, metadata: &mut Metadata) {
    if data.len() < 10 {
        metadata.add_warning(
            Warning::new(
                "truncated-vp8",
                "WebP VP8 chunk is shorter than its frame header",
            )
            .at(data_offset),
        );
        return;
    }
    if data[3..6] != [0x9D, 0x01, 0x2A] {
        metadata.add_warning(
            Warning::new("invalid-vp8-frame", "WebP VP8 frame start code is invalid")
                .at(data_offset + 3),
        );
        return;
    }
    let width = u32::from(u16::from_le_bytes([data[6], data[7]]) & 0x3FFF);
    let height = u32::from(u16::from_le_bytes([data[8], data[9]]) & 0x3FFF);
    add_dimensions(
        metadata,
        "VP8",
        "Bitstream dimension",
        "WebP/VP8",
        data_offset,
        [("ImageWidth", width, 6, 2), ("ImageHeight", height, 8, 2)],
    );
}

fn parse_vp8l(data: &[u8], data_offset: u64, metadata: &mut Metadata) {
    if data.len() < 5 {
        metadata.add_warning(
            Warning::new(
                "truncated-vp8l",
                "WebP VP8L chunk is shorter than its frame header",
            )
            .at(data_offset),
        );
        return;
    }
    if data[0] != 0x2F {
        metadata.add_warning(
            Warning::new("invalid-vp8l-frame", "WebP VP8L signature is invalid").at(data_offset),
        );
        return;
    }
    let width = 1 + u32::from(data[1]) + (u32::from(data[2] & 0x3F) << 8);
    let height =
        1 + u32::from(data[2] >> 6) + (u32::from(data[3]) << 2) + (u32::from(data[4] & 0x0F) << 10);
    add_dimensions(
        metadata,
        "VP8L",
        "Bitstream dimension",
        "WebP/VP8L",
        data_offset,
        [("ImageWidth", width, 1, 2), ("ImageHeight", height, 2, 3)],
    );
}

fn add_dimensions(
    metadata: &mut Metadata,
    group: &str,
    description: &str,
    source_name: &str,
    data_offset: u64,
    dimensions: [(&str, u32, u64, u64); 2],
) {
    for (name, value, start, length) in dimensions {
        metadata.add_tag(Tag {
            namespace: "WebP".to_owned(),
            group: group.to_owned(),
            id: None,
            name: name.to_owned(),
            description: Some(description.to_owned()),
            raw_value: None,
            value: TagValue::Unsigned(u64::from(value)),
            value_type: ValueType::UnsignedInteger,
            source: Source::new(source_name, Some(data_offset + start), Some(length)),
            writable: false,
        });
    }
}

fn checked_add(left: u64, right: u64, context: &str) -> Result<u64> {
    left.checked_add(right).ok_or(MetraError::InvalidOffset {
        context: context.to_owned(),
        offset: left,
    })
}

fn seek_forward<R: Seek>(reader: &mut R, length: u64, path: &Path) -> Result<()> {
    let distance = i64::try_from(length).map_err(|_| MetraError::InvalidOffset {
        context: "WebP chunk skip".to_owned(),
        offset: length,
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
    use metra_core::{FileFormat, FileInfo};

    fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(kind);
        bytes.extend_from_slice(&(data.len() as u32).to_le_bytes());
        bytes.extend_from_slice(data);
        if data.len() & 1 == 1 {
            bytes.push(0);
        }
        bytes
    }

    fn minimal_icc_profile() -> Vec<u8> {
        let mut profile = vec![0_u8; 132];
        let profile_size = profile.len() as u32;
        profile[0..4].copy_from_slice(&profile_size.to_be_bytes());
        profile[8] = 4;
        profile[9] = 0x30;
        profile[12..16].copy_from_slice(b"mntr");
        profile[16..20].copy_from_slice(b"RGB ");
        profile[20..24].copy_from_slice(b"XYZ ");
        profile[36..40].copy_from_slice(b"acsp");
        profile[40..44].copy_from_slice(b"APPL");
        profile[48..52].copy_from_slice(b"TEST");
        profile[52..56].copy_from_slice(b"MODL");
        profile[64..68].copy_from_slice(&1_u32.to_be_bytes());
        profile
    }

    #[test]
    fn reads_vp8x_dimensions() {
        let vp8x = [0, 0, 0, 0, 0x7F, 0x02, 0, 0xDF, 0x01, 0];
        let payload = chunk(b"VP8X", &vp8x);
        let riff_size = 4 + payload.len() as u32;
        let mut bytes = b"RIFF".to_vec();
        bytes.extend_from_slice(&riff_size.to_le_bytes());
        bytes.extend_from_slice(b"WEBP");
        bytes.extend_from_slice(&payload);
        let info = FileInfo::new("test.webp".into(), bytes.len() as u64, FileFormat::Webp);
        let metadata = read_webp(&mut Cursor::new(bytes), info, ParseLimits::default()).unwrap();
        assert_eq!(
            metadata.find("WebP:ImageWidth").unwrap().display_value(),
            "640"
        );
        assert_eq!(
            metadata.find("WebP:ImageHeight").unwrap().display_value(),
            "480"
        );
    }

    #[test]
    fn reads_vp8_and_vp8l_bitstream_dimensions() {
        let vp8 = [0, 0, 0, 0x9D, 0x01, 0x2A, 0x80, 0x02, 0xE0, 0x01];
        let vp8_payload = chunk(b"VP8 ", &vp8);
        let mut vp8_bytes = b"RIFF".to_vec();
        vp8_bytes.extend_from_slice(&(4_u32 + vp8_payload.len() as u32).to_le_bytes());
        vp8_bytes.extend_from_slice(b"WEBP");
        vp8_bytes.extend_from_slice(&vp8_payload);
        let vp8_metadata = read_webp(
            &mut Cursor::new(vp8_bytes.clone()),
            FileInfo::new(
                "lossy.webp".into(),
                vp8_bytes.len() as u64,
                FileFormat::Webp,
            ),
            ParseLimits::default(),
        )
        .unwrap();
        assert_eq!(
            vp8_metadata
                .find("WebP:ImageWidth")
                .unwrap()
                .display_value(),
            "640"
        );
        assert_eq!(
            vp8_metadata
                .find("WebP:ImageHeight")
                .unwrap()
                .display_value(),
            "480"
        );

        let vp8l = [0x2F, 0x7F, 0xC2, 0x77, 0];
        let vp8l_payload = chunk(b"VP8L", &vp8l);
        let mut vp8l_bytes = b"RIFF".to_vec();
        vp8l_bytes.extend_from_slice(&(4_u32 + vp8l_payload.len() as u32).to_le_bytes());
        vp8l_bytes.extend_from_slice(b"WEBP");
        vp8l_bytes.extend_from_slice(&vp8l_payload);
        let vp8l_metadata = read_webp(
            &mut Cursor::new(vp8l_bytes.clone()),
            FileInfo::new(
                "lossless.webp".into(),
                vp8l_bytes.len() as u64,
                FileFormat::Webp,
            ),
            ParseLimits::default(),
        )
        .unwrap();
        assert_eq!(
            vp8l_metadata
                .find("WebP:ImageWidth")
                .unwrap()
                .display_value(),
            "640"
        );
        assert_eq!(
            vp8l_metadata
                .find("WebP:ImageHeight")
                .unwrap()
                .display_value(),
            "480"
        );
    }

    #[test]
    fn reads_icc_profile_chunk() {
        let iccp = chunk(b"ICCP", &minimal_icc_profile());
        let riff_size = 4 + iccp.len() as u32;
        let mut bytes = b"RIFF".to_vec();
        bytes.extend_from_slice(&riff_size.to_le_bytes());
        bytes.extend_from_slice(b"WEBP");
        bytes.extend_from_slice(&iccp);
        let info = FileInfo::new("profile.webp".into(), bytes.len() as u64, FileFormat::Webp);
        let metadata = read_webp(&mut Cursor::new(bytes), info, ParseLimits::default()).unwrap();
        assert_eq!(
            metadata.find("ICC:DeviceClass").unwrap().display_value(),
            "mntr"
        );
        assert!(
            !metadata
                .warnings
                .iter()
                .any(|warning| warning.code == "unsupported-icc")
        );
    }
}
