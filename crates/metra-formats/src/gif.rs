use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use metra_core::{
    FileInfo, Metadata, MetraError, ParseLimits, Result, Source, Tag, TagValue, ValueType, Warning,
};

pub fn read_gif<R: Read + Seek>(
    reader: &mut R,
    file_info: FileInfo,
    limits: ParseLimits,
) -> Result<Metadata> {
    let path = file_info.path.clone();
    let file_length = file_info.size;
    let mut metadata = Metadata::new(file_info);
    let mut header = [0_u8; 6];
    read_exact(reader, &mut header, &path)?;
    if &header != b"GIF87a" && &header != b"GIF89a" {
        return Err(MetraError::InvalidHeader {
            context: "GIF".to_owned(),
            message: "expected GIF87a or GIF89a header".to_owned(),
        });
    }
    let mut descriptor = [0_u8; 7];
    read_exact(reader, &mut descriptor, &path)?;
    let width = u16::from_le_bytes([descriptor[0], descriptor[1]]);
    let height = u16::from_le_bytes([descriptor[2], descriptor[3]]);
    add_tag(
        &mut metadata,
        "ImageWidth",
        TagValue::Unsigned(u64::from(width)),
        ValueType::UnsignedInteger,
        6,
        2,
    );
    add_tag(
        &mut metadata,
        "ImageHeight",
        TagValue::Unsigned(u64::from(height)),
        ValueType::UnsignedInteger,
        8,
        2,
    );
    let packed = descriptor[4];
    let mut offset = 13_u64;
    if packed & 0x80 != 0 {
        let entries = 1_usize.checked_shl(u32::from((packed & 0x07) + 1)).ok_or(
            MetraError::InvalidOffset {
                context: "GIF global color table".to_owned(),
                offset: u64::from(packed),
            },
        )?;
        let length = entries.checked_mul(3).ok_or(MetraError::InvalidOffset {
            context: "GIF global color table".to_owned(),
            offset: entries as u64,
        })?;
        skip_bytes(reader, length, &path)?;
        offset = offset.saturating_add(length as u64);
    }

    let mut block_count = 0_usize;
    let mut metadata_bytes = 0_usize;
    let mut reached_trailer = false;
    while offset < file_length && block_count < limits.max_jpeg_segments {
        let mut introducer = [0_u8; 1];
        read_exact(reader, &mut introducer, &path)?;
        offset = offset.saturating_add(1);
        match introducer[0] {
            0x3B => {
                reached_trailer = true;
                break;
            }
            0x2C => {
                skip_image(reader, &path, &mut offset)?;
            }
            0x21 => {
                let mut label = [0_u8; 1];
                read_exact(reader, &mut label, &path)?;
                offset = offset.saturating_add(1);
                let payload = read_sub_blocks(
                    reader,
                    &path,
                    &mut offset,
                    &mut metadata_bytes,
                    limits,
                    &mut metadata,
                )?;
                if let Some(payload) = payload {
                    if label[0] == 0xFE {
                        add_comment(&mut metadata, payload, offset);
                    } else if label[0] == 0xFF {
                        metadata.add_warning(
                            Warning::new(
                                "gif-application-extension",
                                "GIF application extension was inspected but not decoded",
                            )
                            .at(offset),
                        );
                    }
                }
            }
            other => {
                metadata.add_warning(
                    Warning::new(
                        "invalid-gif-block",
                        format!("unknown GIF block introducer 0x{other:02X}"),
                    )
                    .at(offset.saturating_sub(1)),
                );
                break;
            }
        }
        block_count += 1;
    }
    if block_count >= limits.max_jpeg_segments {
        metadata.add_warning(Warning::new(
            "gif-block-limit",
            format!("stopped after {} GIF blocks", limits.max_jpeg_segments),
        ));
    } else if !reached_trailer {
        metadata.add_warning(Warning::new(
            "truncated-gif",
            "GIF ended before its trailer",
        ));
    }
    metadata.sort_tags();
    Ok(metadata)
}

fn skip_image<R: Read + Seek>(reader: &mut R, path: &Path, offset: &mut u64) -> Result<()> {
    let mut image_descriptor = [0_u8; 9];
    read_exact(reader, &mut image_descriptor, path)?;
    *offset = offset.saturating_add(9);
    let packed = image_descriptor[8];
    if packed & 0x80 != 0 {
        let entries = 1_usize.checked_shl(u32::from((packed & 0x07) + 1)).ok_or(
            MetraError::InvalidOffset {
                context: "GIF local color table".to_owned(),
                offset: u64::from(packed),
            },
        )?;
        let length = entries.checked_mul(3).ok_or(MetraError::InvalidOffset {
            context: "GIF local color table".to_owned(),
            offset: entries as u64,
        })?;
        skip_bytes(reader, length, path)?;
        *offset = offset.saturating_add(length as u64);
    }
    let mut code_size = [0_u8; 1];
    read_exact(reader, &mut code_size, path)?;
    *offset = offset.saturating_add(1);
    if code_size[0] == 0 {
        return Err(MetraError::InvalidTag {
            context: "GIF image data".to_owned(),
            message: "LZW minimum code size cannot be zero".to_owned(),
        });
    }
    skip_sub_blocks(reader, path, offset)
}

fn read_sub_blocks<R: Read + Seek>(
    reader: &mut R,
    path: &Path,
    offset: &mut u64,
    metadata_bytes: &mut usize,
    limits: ParseLimits,
    metadata: &mut Metadata,
) -> Result<Option<Vec<u8>>> {
    let mut result = Vec::new();
    let mut limited = false;
    loop {
        let mut size = [0_u8; 1];
        read_exact(reader, &mut size, path)?;
        *offset = offset.saturating_add(1);
        let length = usize::from(size[0]);
        if length == 0 {
            break;
        }
        let remaining = limits.max_metadata_bytes.saturating_sub(*metadata_bytes);
        if length > remaining {
            limited = true;
            metadata.add_warning(
                Warning::new(
                    "gif-metadata-limit",
                    "GIF extension payload exceeded the metadata budget",
                )
                .at(*offset),
            );
            skip_bytes(reader, length, path)?;
        } else {
            let mut bytes = vec![0_u8; length];
            read_exact(reader, &mut bytes, path)?;
            *metadata_bytes = metadata_bytes.saturating_add(length);
            if !limited {
                result.extend_from_slice(&bytes);
            }
        }
        *offset = offset.saturating_add(length as u64);
    }
    if limited { Ok(None) } else { Ok(Some(result)) }
}

fn skip_sub_blocks<R: Read + Seek>(reader: &mut R, path: &Path, offset: &mut u64) -> Result<()> {
    loop {
        let mut size = [0_u8; 1];
        read_exact(reader, &mut size, path)?;
        *offset = offset.saturating_add(1);
        let length = usize::from(size[0]);
        if length == 0 {
            return Ok(());
        }
        skip_bytes(reader, length, path)?;
        *offset = offset.saturating_add(length as u64);
    }
}

fn add_comment(metadata: &mut Metadata, payload: Vec<u8>, offset: u64) {
    metadata.add_tag(Tag {
        namespace: "GIF".to_owned(),
        group: "CommentExtension".to_owned(),
        id: None,
        name: "Comment".to_owned(),
        description: Some("GIF comment extension".to_owned()),
        raw_value: Some(payload.clone()),
        value: TagValue::String(String::from_utf8_lossy(&payload).into_owned()),
        value_type: ValueType::String,
        source: Source::new(
            "GIF/CommentExtension",
            Some(offset),
            Some(payload.len() as u64),
        ),
        writable: false,
    });
}

fn add_tag(
    metadata: &mut Metadata,
    name: &str,
    value: TagValue,
    value_type: ValueType,
    offset: u64,
    length: u64,
) {
    metadata.add_tag(Tag {
        namespace: "GIF".to_owned(),
        group: "LogicalScreenDescriptor".to_owned(),
        id: None,
        name: name.to_owned(),
        description: Some("GIF logical screen property".to_owned()),
        raw_value: None,
        value,
        value_type,
        source: Source::new("GIF/LogicalScreenDescriptor", Some(offset), Some(length)),
        writable: false,
    });
}

fn skip_bytes<R: Seek>(reader: &mut R, length: usize, path: &Path) -> Result<()> {
    let distance = i64::try_from(length).map_err(|_| MetraError::InvalidOffset {
        context: "GIF skip".to_owned(),
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
    use metra_core::{FileFormat, FileInfo};

    #[test]
    fn reads_dimensions_and_comment_without_decoding_frames() {
        let mut bytes = b"GIF89a".to_vec();
        bytes.extend_from_slice(&2_u16.to_le_bytes());
        bytes.extend_from_slice(&3_u16.to_le_bytes());
        bytes.extend_from_slice(&[0, 0, 0]); // packed, background, aspect
        bytes.extend_from_slice(&[0x21, 0xFE, 5]);
        bytes.extend_from_slice(b"hello");
        bytes.extend_from_slice(&[0, 0x3B]);
        let info = FileInfo::new("test.gif".into(), bytes.len() as u64, FileFormat::Gif);
        let metadata = read_gif(&mut Cursor::new(bytes), info, ParseLimits::default()).unwrap();
        assert_eq!(
            metadata.find("GIF:ImageWidth").unwrap().display_value(),
            "2"
        );
        assert_eq!(
            metadata.find("GIF:ImageHeight").unwrap().display_value(),
            "3"
        );
        assert_eq!(
            metadata.find("GIF:Comment").unwrap().display_value(),
            "hello"
        );
    }

    #[test]
    fn rejects_zero_lzw_code_size() {
        let mut bytes = b"GIF89a".to_vec();
        bytes.extend_from_slice(&[1, 0, 1, 0, 0, 0, 0]);
        bytes.extend_from_slice(&[0x2C]);
        bytes.extend_from_slice(&[0; 9]);
        bytes.push(0);
        let info = FileInfo::new("bad.gif".into(), bytes.len() as u64, FileFormat::Gif);
        let result = read_gif(&mut Cursor::new(bytes), info, ParseLimits::default());
        assert!(matches!(result, Err(MetraError::InvalidTag { .. })));
    }
}
