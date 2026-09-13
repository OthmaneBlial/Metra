use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use metra_core::{
    FileInfo, Metadata, MetraError, ParseLimits, Result, Source, Tag, TagValue, ValueType, Warning,
};

const STREAMINFO_LENGTH: usize = 34;

pub fn read_flac<R: Read + Seek>(
    reader: &mut R,
    file_info: FileInfo,
    limits: ParseLimits,
) -> Result<Metadata> {
    let path = file_info.path.clone();
    let file_length = file_info.size;
    let mut metadata = Metadata::new(file_info);
    if file_length < 4 {
        return Err(MetraError::InvalidHeader {
            context: "FLAC".to_owned(),
            message: "file is shorter than the FLAC signature".to_owned(),
        });
    }
    let signature = read_at(reader, 0, 4, &path, "FLAC signature")?;
    if signature.as_slice() != b"fLaC" {
        return Err(MetraError::InvalidHeader {
            context: "FLAC".to_owned(),
            message: "expected fLaC signature".to_owned(),
        });
    }

    let mut offset = 4_u64;
    let mut block_count = 0_usize;
    let mut metadata_bytes = 0_usize;
    let mut saw_streaminfo = false;
    let mut saw_last_block = false;
    while offset < file_length && !saw_last_block {
        if block_count >= limits.max_jpeg_segments {
            metadata.add_warning(Warning::new(
                "flac-block-limit",
                format!(
                    "stopped after {} FLAC metadata blocks",
                    limits.max_jpeg_segments
                ),
            ));
            break;
        }
        if file_length.saturating_sub(offset) < 4 {
            metadata.add_warning(Warning::new(
                "truncated-flac-block",
                "FLAC metadata block header is truncated",
            ));
            break;
        }
        let header = read_at(reader, offset, 4, &path, "FLAC metadata block header")?;
        saw_last_block = header[0] & 0x80 != 0;
        let block_type = header[0] & 0x7F;
        let length =
            (usize::from(header[1]) << 16) | (usize::from(header[2]) << 8) | usize::from(header[3]);
        let data_offset = offset + 4;
        let data_end = data_offset
            .checked_add(length as u64)
            .ok_or(MetraError::InvalidOffset {
                context: "FLAC metadata block".to_owned(),
                offset: data_offset,
            })?;
        if data_end > file_length {
            return Err(MetraError::UnexpectedEof {
                context: format!("FLAC metadata block type {block_type}"),
            });
        }
        metadata_bytes =
            metadata_bytes
                .checked_add(length)
                .ok_or(MetraError::ResourceLimitExceeded {
                    resource: "FLAC metadata".to_owned(),
                    limit: limits.max_metadata_bytes,
                })?;
        if metadata_bytes > limits.max_metadata_bytes {
            metadata.add_warning(
                Warning::new("flac-metadata-limit", "FLAC metadata budget was reached")
                    .at(data_offset),
            );
            break;
        }
        match block_type {
            0 => {
                if saw_streaminfo {
                    metadata.add_warning(
                        Warning::new(
                            "duplicate-flac-streaminfo",
                            "FLAC contains more than one STREAMINFO block",
                        )
                        .at(data_offset),
                    );
                } else if length != STREAMINFO_LENGTH {
                    metadata.add_warning(
                        Warning::new(
                            "invalid-flac-streaminfo",
                            format!("STREAMINFO length is {length}, expected {STREAMINFO_LENGTH}"),
                        )
                        .at(data_offset),
                    );
                } else {
                    let data = read_at(reader, data_offset, length, &path, "FLAC STREAMINFO")?;
                    parse_streaminfo(&data, data_offset, &mut metadata);
                    saw_streaminfo = true;
                }
            }
            4 => {
                let data = read_at(reader, data_offset, length, &path, "FLAC Vorbis comments")?;
                parse_vorbis_comments(&data, data_offset, limits, &mut metadata);
            }
            6 => {
                let data = read_at(reader, data_offset, length, &path, "FLAC picture")?;
                parse_picture(&data, data_offset, limits, &mut metadata);
            }
            1 => {}
            2 => {
                let data = read_at(reader, data_offset, length, &path, "FLAC SEEKTABLE")?;
                parse_seektable(&data, data_offset, limits, &mut metadata);
            }
            3 => metadata.add_warning(
                Warning::new(
                    "flac-vorbis-comment",
                    "FLAC VORBIS_COMMENT block was not expected before STREAMINFO",
                )
                .at(data_offset),
            ),
            5 => metadata.add_warning(
                Warning::new(
                    "flac-cuesheet",
                    "FLAC CUESHEET is present but cue points are not decoded",
                )
                .at(data_offset),
            ),
            7..=126 => metadata.add_warning(
                Warning::new(
                    "flac-application",
                    format!("FLAC metadata block type {block_type} is not decoded"),
                )
                .at(data_offset),
            ),
            127 => metadata.add_warning(
                Warning::new(
                    "invalid-flac-block-type",
                    "FLAC metadata block type 127 is reserved",
                )
                .at(offset),
            ),
            _ => unreachable!("FLAC block types are limited to seven bits"),
        }
        offset = data_end;
        block_count += 1;
    }
    if !saw_streaminfo {
        return Err(MetraError::InvalidHeader {
            context: "FLAC".to_owned(),
            message: "missing STREAMINFO block".to_owned(),
        });
    }
    if !saw_last_block {
        metadata.add_warning(Warning::new(
            "truncated-flac-metadata",
            "FLAC metadata block list ended without a last-block marker",
        ));
    }
    metadata.sort_tags();
    Ok(metadata)
}

fn parse_streaminfo(data: &[u8], offset: u64, metadata: &mut Metadata) {
    add_tag(
        metadata,
        "MinBlockSize",
        TagValue::Unsigned(u64::from(u16::from_be_bytes([data[0], data[1]]))),
        ValueType::UnsignedInteger,
        "STREAMINFO",
        offset,
        2,
    );
    add_tag(
        metadata,
        "MaxBlockSize",
        TagValue::Unsigned(u64::from(u16::from_be_bytes([data[2], data[3]]))),
        ValueType::UnsignedInteger,
        "STREAMINFO",
        offset + 2,
        2,
    );
    add_tag(
        metadata,
        "MinFrameSize",
        TagValue::Unsigned(u64::from(read_u24(&data[4..7]))),
        ValueType::UnsignedInteger,
        "STREAMINFO",
        offset + 4,
        3,
    );
    add_tag(
        metadata,
        "MaxFrameSize",
        TagValue::Unsigned(u64::from(read_u24(&data[7..10]))),
        ValueType::UnsignedInteger,
        "STREAMINFO",
        offset + 7,
        3,
    );
    let packed = u64::from_be_bytes(
        data[10..18]
            .try_into()
            .expect("FLAC streaminfo packed fields"),
    );
    let sample_rate = packed >> 44;
    let channels = ((packed >> 41) & 0x07) + 1;
    let bits_per_sample = ((packed >> 36) & 0x1F) + 1;
    let total_samples = packed & 0x0F_FFFF_FFFF;
    add_tag(
        metadata,
        "SampleRateHz",
        TagValue::Unsigned(sample_rate),
        ValueType::UnsignedInteger,
        "STREAMINFO",
        offset + 10,
        4,
    );
    add_tag(
        metadata,
        "Channels",
        TagValue::Unsigned(channels),
        ValueType::UnsignedInteger,
        "STREAMINFO",
        offset + 14,
        1,
    );
    add_tag(
        metadata,
        "BitsPerSample",
        TagValue::Unsigned(bits_per_sample),
        ValueType::UnsignedInteger,
        "STREAMINFO",
        offset + 14,
        1,
    );
    add_tag(
        metadata,
        "TotalSamples",
        TagValue::Unsigned(total_samples),
        ValueType::UnsignedInteger,
        "STREAMINFO",
        offset + 14,
        4,
    );
    if sample_rate != 0 {
        add_tag(
            metadata,
            "DurationSeconds",
            TagValue::Float(total_samples as f64 / sample_rate as f64),
            ValueType::Float,
            "STREAMINFO",
            offset + 10,
            8,
        );
    }
    if data[18..34].iter().any(|byte| *byte != 0) {
        let md5 = data[18..34].to_vec();
        add_tag(
            metadata,
            "Md5Signature",
            TagValue::Bytes(md5.clone()),
            ValueType::Bytes,
            "STREAMINFO",
            offset + 18,
            16,
        );
    }
}

fn parse_seektable(data: &[u8], offset: u64, limits: ParseLimits, metadata: &mut Metadata) {
    const SEEKPOINT_LENGTH: usize = 18;

    let remainder = data.len() % SEEKPOINT_LENGTH;
    if remainder != 0 {
        metadata.add_warning(
            Warning::new(
                "invalid-flac-seektable",
                "SEEKTABLE length is not a multiple of 18 bytes",
            )
            .at(offset + (data.len() - remainder) as u64),
        );
    }

    let entry_count = data.len() / SEEKPOINT_LENGTH;
    add_tag(
        metadata,
        "SeekPointCount",
        TagValue::Unsigned(entry_count as u64),
        ValueType::UnsignedInteger,
        "SEEKTABLE",
        offset,
        data.len() as u64,
    );

    let materialized_count = entry_count.min(limits.max_jpeg_segments);
    if materialized_count < entry_count {
        metadata.add_warning(
            Warning::new(
                "flac-seekpoint-limit",
                format!(
                    "stopped after {} FLAC seek points",
                    limits.max_jpeg_segments
                ),
            )
            .at(offset),
        );
    }

    let points = data[..materialized_count * SEEKPOINT_LENGTH]
        .chunks_exact(SEEKPOINT_LENGTH)
        .map(|entry| {
            let mut point = BTreeMap::new();
            point.insert(
                "SampleNumber".to_owned(),
                TagValue::Unsigned(u64::from_be_bytes(
                    entry[..8]
                        .try_into()
                        .expect("FLAC seek point sample number"),
                )),
            );
            point.insert(
                "StreamOffset".to_owned(),
                TagValue::Unsigned(u64::from_be_bytes(
                    entry[8..16]
                        .try_into()
                        .expect("FLAC seek point stream offset"),
                )),
            );
            point.insert(
                "FrameSamples".to_owned(),
                TagValue::Unsigned(u64::from(u16::from_be_bytes(
                    entry[16..18]
                        .try_into()
                        .expect("FLAC seek point frame samples"),
                ))),
            );
            TagValue::Structure(point)
        })
        .collect::<Vec<_>>();
    add_tag(
        metadata,
        "SeekPoints",
        TagValue::Array(points),
        ValueType::Array,
        "SEEKTABLE",
        offset,
        (materialized_count * SEEKPOINT_LENGTH) as u64,
    );
}

fn parse_vorbis_comments(data: &[u8], offset: u64, limits: ParseLimits, metadata: &mut Metadata) {
    if data.len() < 8 {
        metadata.add_warning(
            Warning::new(
                "truncated-vorbis-comment",
                "VORBIS_COMMENT block is shorter than its two count fields",
            )
            .at(offset),
        );
        return;
    }
    let vendor_length = u32::from_le_bytes(data[..4].try_into().expect("Vorbis vendor length"));
    let Some(mut cursor) = 4_usize.checked_add(vendor_length as usize) else {
        metadata.add_warning(
            Warning::new("invalid-vorbis-comment", "vendor string length overflows").at(offset),
        );
        return;
    };
    if cursor + 4 > data.len() {
        metadata.add_warning(
            Warning::new(
                "truncated-vorbis-comment",
                "vendor string exceeds the block",
            )
            .at(offset),
        );
        return;
    }
    let vendor = String::from_utf8_lossy(&data[4..cursor]).into_owned();
    add_tag(
        metadata,
        "Vendor",
        TagValue::String(vendor),
        ValueType::String,
        "VORBIS_COMMENT",
        offset + 4,
        vendor_length as u64,
    );
    let count = u32::from_le_bytes(
        data[cursor..cursor + 4]
            .try_into()
            .expect("Vorbis comment count"),
    );
    cursor += 4;
    let mut comments_read = 0_usize;
    while comments_read < usize::try_from(count).unwrap_or(usize::MAX) {
        if comments_read >= limits.max_jpeg_segments {
            metadata.add_warning(Warning::new(
                "vorbis-comment-limit",
                format!("stopped after {} Vorbis comments", limits.max_jpeg_segments),
            ));
            return;
        }
        if cursor + 4 > data.len() {
            metadata.add_warning(
                Warning::new("truncated-vorbis-comment", "comment length is truncated")
                    .at(offset + cursor as u64),
            );
            return;
        }
        let length = u32::from_le_bytes(
            data[cursor..cursor + 4]
                .try_into()
                .expect("Vorbis comment length"),
        ) as usize;
        cursor += 4;
        let Some(end) = cursor.checked_add(length) else {
            metadata.add_warning(
                Warning::new("invalid-vorbis-comment", "comment length overflows")
                    .at(offset + cursor as u64),
            );
            return;
        };
        if end > data.len() {
            metadata.add_warning(
                Warning::new("truncated-vorbis-comment", "comment exceeds the block")
                    .at(offset + cursor as u64),
            );
            return;
        }
        let raw = &data[cursor..end];
        if let Some(separator) = raw.iter().position(|byte| *byte == b'=') {
            let key = String::from_utf8_lossy(&raw[..separator]).to_ascii_uppercase();
            let value = String::from_utf8_lossy(&raw[separator + 1..]).into_owned();
            let name = vorbis_name(&key).unwrap_or_else(|| format!("Comment:{key}"));
            add_tag(
                metadata,
                &name,
                TagValue::String(value),
                ValueType::String,
                "VORBIS_COMMENT",
                offset + cursor as u64,
                length as u64,
            );
        } else {
            metadata.add_warning(
                Warning::new(
                    "invalid-vorbis-comment",
                    "comment does not contain a KEY=VALUE separator",
                )
                .at(offset + cursor as u64),
            );
        }
        cursor = end;
        comments_read += 1;
    }
}

fn parse_picture(data: &[u8], offset: u64, limits: ParseLimits, metadata: &mut Metadata) {
    let mut cursor = 0_usize;
    let Some(picture_type) = take_u32(data, &mut cursor) else {
        warn_picture(metadata, offset, "picture type is truncated");
        return;
    };
    let Some(mime) = take_string(data, &mut cursor) else {
        warn_picture(metadata, offset, "MIME type is truncated");
        return;
    };
    let Some(description) = take_string(data, &mut cursor) else {
        warn_picture(metadata, offset, "picture description is truncated");
        return;
    };
    let Some(width) = take_u32(data, &mut cursor) else {
        warn_picture(metadata, offset, "picture width is truncated");
        return;
    };
    let Some(height) = take_u32(data, &mut cursor) else {
        warn_picture(metadata, offset, "picture height is truncated");
        return;
    };
    let Some(depth) = take_u32(data, &mut cursor) else {
        warn_picture(metadata, offset, "picture color depth is truncated");
        return;
    };
    let Some(colors) = take_u32(data, &mut cursor) else {
        warn_picture(metadata, offset, "picture color count is truncated");
        return;
    };
    let Some(image_length) = take_u32(data, &mut cursor).map(|value| value as usize) else {
        warn_picture(metadata, offset, "picture data length is truncated");
        return;
    };
    let Some(image_end) = cursor.checked_add(image_length) else {
        warn_picture(metadata, offset, "picture data length overflows");
        return;
    };
    if image_end > data.len() {
        warn_picture(metadata, offset, "picture data exceeds the metadata block");
        return;
    }
    add_tag(
        metadata,
        "PictureType",
        TagValue::Unsigned(u64::from(picture_type)),
        ValueType::UnsignedInteger,
        "PICTURE",
        offset,
        4,
    );
    add_tag(
        metadata,
        "PictureMimeType",
        TagValue::String(mime),
        ValueType::String,
        "PICTURE",
        offset + 4,
        (cursor - 4) as u64,
    );
    if !description.is_empty() {
        add_tag(
            metadata,
            "PictureDescription",
            TagValue::String(description),
            ValueType::String,
            "PICTURE",
            offset + 4,
            (cursor - 4) as u64,
        );
    }
    add_tag(
        metadata,
        "PictureWidth",
        TagValue::Unsigned(u64::from(width)),
        ValueType::UnsignedInteger,
        "PICTURE",
        offset,
        cursor as u64,
    );
    add_tag(
        metadata,
        "PictureHeight",
        TagValue::Unsigned(u64::from(height)),
        ValueType::UnsignedInteger,
        "PICTURE",
        offset,
        cursor as u64,
    );
    add_tag(
        metadata,
        "PictureColorDepth",
        TagValue::Unsigned(u64::from(depth)),
        ValueType::UnsignedInteger,
        "PICTURE",
        offset,
        cursor as u64,
    );
    add_tag(
        metadata,
        "PictureColors",
        TagValue::Unsigned(u64::from(colors)),
        ValueType::UnsignedInteger,
        "PICTURE",
        offset,
        cursor as u64,
    );
    if image_length > limits.max_value_bytes {
        warn_picture(
            metadata,
            offset + cursor as u64,
            "picture payload exceeded the value budget",
        );
        return;
    }
    add_tag(
        metadata,
        "PictureData",
        TagValue::Bytes(data[cursor..image_end].to_vec()),
        ValueType::Bytes,
        "PICTURE",
        offset + cursor as u64,
        image_length as u64,
    );
}

fn vorbis_name(key: &str) -> Option<String> {
    Some(
        match key {
            "TITLE" => "Title",
            "ARTIST" => "Artist",
            "ALBUM" => "Album",
            "ALBUMARTIST" | "ALBUM ARTIST" => "AlbumArtist",
            "DATE" | "YEAR" => "Date",
            "GENRE" => "Genre",
            "TRACKNUMBER" | "TRACK" => "TrackNumber",
            "DISCNUMBER" | "DISC" => "DiscNumber",
            "COMMENT" => "Comment",
            "COMPOSER" => "Composer",
            "COPYRIGHT" => "Copyright",
            "DESCRIPTION" => "Description",
            "ENCODER" => "Encoder",
            "LICENSE" => "License",
            "ORGANIZATION" | "LABEL" => "Organization",
            "ISRC" => "ISRC",
            _ => return None,
        }
        .to_owned(),
    )
}

fn take_u32(data: &[u8], cursor: &mut usize) -> Option<u32> {
    let end = cursor.checked_add(4)?;
    let value = u32::from_be_bytes(data.get(*cursor..end)?.try_into().ok()?);
    *cursor = end;
    Some(value)
}

fn take_string(data: &[u8], cursor: &mut usize) -> Option<String> {
    let length = take_u32_le(data, cursor)? as usize;
    let end = cursor.checked_add(length)?;
    let value = String::from_utf8_lossy(data.get(*cursor..end)?).into_owned();
    *cursor = end;
    Some(value)
}

fn take_u32_le(data: &[u8], cursor: &mut usize) -> Option<u32> {
    let end = cursor.checked_add(4)?;
    let value = u32::from_le_bytes(data.get(*cursor..end)?.try_into().ok()?);
    *cursor = end;
    Some(value)
}

fn read_u24(bytes: &[u8]) -> u32 {
    (u32::from(bytes[0]) << 16) | (u32::from(bytes[1]) << 8) | u32::from(bytes[2])
}

fn warn_picture(metadata: &mut Metadata, offset: u64, message: &str) {
    metadata.add_warning(Warning::new("invalid-flac-picture", message).at(offset));
}

fn add_tag(
    metadata: &mut Metadata,
    name: &str,
    value: TagValue,
    value_type: ValueType,
    group: &str,
    offset: u64,
    length: u64,
) {
    metadata.add_tag(Tag {
        namespace: "FLAC".to_owned(),
        group: group.to_owned(),
        id: None,
        name: name.to_owned(),
        description: Some("FLAC metadata".to_owned()),
        raw_value: None,
        value,
        value_type,
        source: Source::new("FLAC", Some(offset), Some(length)),
        writable: false,
    });
}

fn read_at<R: Read + Seek>(
    reader: &mut R,
    offset: u64,
    length: usize,
    path: &Path,
    context: &str,
) -> Result<Vec<u8>> {
    reader
        .seek(SeekFrom::Start(offset))
        .map_err(|source| io_error(path, source))?;
    let mut bytes = vec![0_u8; length];
    reader
        .read_exact(&mut bytes)
        .map_err(|source| match source.kind() {
            std::io::ErrorKind::UnexpectedEof => MetraError::UnexpectedEof {
                context: context.to_owned(),
            },
            _ => io_error(path, source),
        })?;
    Ok(bytes)
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

    fn block(last: bool, kind: u8, data: &[u8]) -> Vec<u8> {
        let mut result = vec![if last { 0x80 | kind } else { kind }];
        result.extend_from_slice(&(data.len() as u32).to_be_bytes()[1..]);
        result.extend_from_slice(data);
        result
    }

    fn streaminfo() -> Vec<u8> {
        let mut data = vec![0_u8; STREAMINFO_LENGTH];
        data[0..2].copy_from_slice(&4096_u16.to_be_bytes());
        data[2..4].copy_from_slice(&4096_u16.to_be_bytes());
        let packed = (44_100_u64 << 44) | (1_u64 << 41) | (15_u64 << 36) | 88_200;
        data[10..18].copy_from_slice(&packed.to_be_bytes());
        data
    }

    fn vorbis_comments() -> Vec<u8> {
        let vendor = b"Metra";
        let comments = [b"TITLE=Track".as_slice(), b"ARTIST=Artist".as_slice()];
        let mut data = (vendor.len() as u32).to_le_bytes().to_vec();
        data.extend_from_slice(vendor);
        data.extend_from_slice(&(comments.len() as u32).to_le_bytes());
        for comment in comments {
            data.extend_from_slice(&(comment.len() as u32).to_le_bytes());
            data.extend_from_slice(comment);
        }
        data
    }

    fn seektable() -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(&0_u64.to_be_bytes());
        data.extend_from_slice(&0_u64.to_be_bytes());
        data.extend_from_slice(&4096_u16.to_be_bytes());
        data.extend_from_slice(&88_200_u64.to_be_bytes());
        data.extend_from_slice(&12_345_u64.to_be_bytes());
        data.extend_from_slice(&4096_u16.to_be_bytes());
        data
    }

    #[test]
    fn reads_streaminfo_and_vorbis_comments() {
        let mut bytes = b"fLaC".to_vec();
        bytes.extend(block(false, 0, &streaminfo()));
        bytes.extend(block(true, 4, &vorbis_comments()));
        let info = FileInfo::new("track.flac".into(), bytes.len() as u64, FileFormat::Flac);
        let metadata = read_flac(&mut Cursor::new(bytes), info, ParseLimits::default()).unwrap();
        assert_eq!(
            metadata.find("FLAC:SampleRateHz").unwrap().display_value(),
            "44100"
        );
        assert_eq!(
            metadata.find("FLAC:Title").unwrap().display_value(),
            "Track"
        );
        assert_eq!(
            metadata.find("FLAC:Artist").unwrap().display_value(),
            "Artist"
        );
    }

    #[test]
    fn reads_seektable_points_as_bounded_structures() {
        let mut bytes = b"fLaC".to_vec();
        bytes.extend(block(false, 0, &streaminfo()));
        bytes.extend(block(true, 2, &seektable()));
        let info = FileInfo::new(
            "seektable.flac".into(),
            bytes.len() as u64,
            FileFormat::Flac,
        );
        let metadata = read_flac(&mut Cursor::new(bytes), info, ParseLimits::default())
            .expect("FLAC seektable fixture should parse");

        assert_eq!(
            metadata.find("FLAC:SeekPointCount").unwrap().value,
            TagValue::Unsigned(2)
        );
        let points = match &metadata.find("FLAC:SeekPoints").unwrap().value {
            TagValue::Array(points) => points,
            value => panic!("expected seek point array, got {value:?}"),
        };
        assert_eq!(points.len(), 2);
        assert_eq!(
            points[1],
            TagValue::Structure(BTreeMap::from([
                ("FrameSamples".to_owned(), TagValue::Unsigned(4096)),
                ("SampleNumber".to_owned(), TagValue::Unsigned(88_200)),
                ("StreamOffset".to_owned(), TagValue::Unsigned(12_345)),
            ]))
        );
        assert!(
            metadata
                .warnings()
                .iter()
                .all(|warning| warning.code != "flac-seektable")
        );
    }

    #[test]
    fn rejects_missing_streaminfo() {
        let bytes = [b'f', b'L', b'a', b'C', 0x80, 0, 0, 0];
        let info = FileInfo::new("bad.flac".into(), bytes.len() as u64, FileFormat::Flac);
        let result = read_flac(&mut Cursor::new(bytes), info, ParseLimits::default());
        assert!(matches!(result, Err(MetraError::InvalidHeader { .. })));
    }
}
