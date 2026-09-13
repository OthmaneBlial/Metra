use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use metra_core::{
    FileInfo, Metadata, MetraError, ParseLimits, Result, Source, Tag, TagValue, ValueType, Warning,
};

const ID3_HEADER_LENGTH: usize = 10;
const ID3V1_LENGTH: u64 = 128;

pub fn read_mp3<R: Read + Seek>(
    reader: &mut R,
    file_info: FileInfo,
    limits: ParseLimits,
) -> Result<Metadata> {
    let path = file_info.path.clone();
    let file_length = file_info.size;
    let mut metadata = Metadata::new(file_info);
    let mut audio_offset = 0_u64;

    if file_length >= 3 {
        reader
            .seek(SeekFrom::Start(0))
            .map_err(|source| io_error(&path, source))?;
        let mut signature = [0_u8; 3];
        read_exact(reader, &mut signature, &path)?;
        if &signature == b"ID3" {
            audio_offset = parse_id3v2(reader, &path, file_length, limits, &mut metadata)?;
        }
    }

    parse_mpeg_header(reader, &path, audio_offset, file_length, &mut metadata)?;
    if file_length >= ID3V1_LENGTH {
        parse_id3v1(reader, &path, file_length, limits, &mut metadata)?;
    }
    metadata.sort_tags();
    Ok(metadata)
}

fn parse_id3v2<R: Read + Seek>(
    reader: &mut R,
    path: &Path,
    file_length: u64,
    limits: ParseLimits,
    metadata: &mut Metadata,
) -> Result<u64> {
    let header = read_at(reader, 0, ID3_HEADER_LENGTH, path, "ID3v2 header")?;
    if &header[..3] != b"ID3" {
        return Err(MetraError::InvalidHeader {
            context: "ID3v2".to_owned(),
            message: "expected ID3 signature".to_owned(),
        });
    }
    let version = header[3];
    if !matches!(version, 2..=4) {
        return Err(MetraError::InvalidHeader {
            context: "ID3v2".to_owned(),
            message: format!("unsupported ID3 major version {version}"),
        });
    }
    let flags = header[5];
    let tag_size = parse_synchsafe(&header[6..10], "ID3v2 tag size")?;
    let tag_size = usize::try_from(tag_size).map_err(|_| MetraError::ResourceLimitExceeded {
        resource: "ID3v2 tag".to_owned(),
        limit: limits.max_metadata_bytes,
    })?;
    if tag_size > limits.max_metadata_bytes {
        return Err(MetraError::ResourceLimitExceeded {
            resource: "ID3v2 tag".to_owned(),
            limit: limits.max_metadata_bytes,
        });
    }
    let tag_end = 10_u64
        .checked_add(tag_size as u64)
        .ok_or(MetraError::InvalidOffset {
            context: "ID3v2 tag end".to_owned(),
            offset: tag_size as u64,
        })?;
    if tag_end > file_length {
        return Err(MetraError::UnexpectedEof {
            context: "ID3v2 tag body".to_owned(),
        });
    }

    add_header_tag(
        metadata,
        "Version",
        TagValue::String(format!("2.{version}.{}", header[4])),
        3,
        2,
    );
    add_header_tag(
        metadata,
        "TagSize",
        TagValue::Unsigned(tag_size as u64),
        6,
        4,
    );

    let mut body = read_at(reader, 10, tag_size, path, "ID3v2 tag body")?;
    if version == 4 && flags & 0x10 != 0 {
        if body.len() < ID3_HEADER_LENGTH {
            metadata.add_warning(
                Warning::new("truncated-id3-footer", "ID3v2.4 footer is truncated")
                    .at(tag_end.saturating_sub(body.len() as u64)),
            );
        } else {
            let footer_start = body.len() - ID3_HEADER_LENGTH;
            if &body[footer_start..footer_start + 3] != b"3DI" {
                metadata.add_warning(
                    Warning::new("invalid-id3-footer", "ID3v2.4 footer signature is invalid")
                        .at(10 + footer_start as u64),
                );
            }
            body.truncate(footer_start);
        }
    }
    if flags & 0x80 != 0 {
        body = remove_unsynchronization(&body);
        metadata.add_warning(Warning::new(
            "id3-unsynchronization",
            "ID3 unsynchronization was removed before frame parsing",
        ));
    }

    let frame_start = if flags & 0x40 != 0 {
        skip_extended_header(&body, version)?
    } else {
        0
    };
    parse_frames(&body, frame_start, version, 10, limits, metadata);
    Ok(tag_end)
}

fn skip_extended_header(body: &[u8], version: u8) -> Result<usize> {
    if body.len() < 4 {
        return Err(MetraError::InvalidTag {
            context: "ID3v2 extended header".to_owned(),
            message: "extended header is shorter than its size field".to_owned(),
        });
    }
    let declared = if version == 3 {
        u32::from_be_bytes(body[..4].try_into().expect("ID3v2.3 extended size")) as usize
    } else {
        usize::try_from(parse_synchsafe(&body[..4], "ID3v2.4 extended header size")?).map_err(
            |_| MetraError::InvalidTag {
                context: "ID3v2 extended header".to_owned(),
                message: "extended header size does not fit in memory".to_owned(),
            },
        )?
    };
    let total = if version == 3 {
        4_usize
            .checked_add(declared)
            .ok_or(MetraError::InvalidTag {
                context: "ID3v2 extended header".to_owned(),
                message: "extended header size overflows".to_owned(),
            })?
    } else if declared >= 4 {
        declared
    } else {
        4_usize
            .checked_add(declared)
            .ok_or(MetraError::InvalidTag {
                context: "ID3v2 extended header".to_owned(),
                message: "extended header size overflows".to_owned(),
            })?
    };
    if total > body.len() {
        return Err(MetraError::UnexpectedEof {
            context: "ID3v2 extended header".to_owned(),
        });
    }
    Ok(total)
}

fn parse_frames(
    body: &[u8],
    start: usize,
    version: u8,
    body_file_offset: u64,
    limits: ParseLimits,
    metadata: &mut Metadata,
) {
    let (id_length, header_length) = if version == 2 {
        (3_usize, 6_usize)
    } else {
        (4, 10)
    };
    let mut cursor = start;
    let mut frame_count = 0_usize;
    while cursor < body.len() {
        if frame_count >= limits.max_jpeg_segments {
            metadata.add_warning(Warning::new(
                "id3-frame-limit",
                format!("stopped after {} ID3 frames", limits.max_jpeg_segments),
            ));
            return;
        }
        if body.len().saturating_sub(cursor) < header_length {
            if body[cursor..].iter().any(|byte| *byte != 0) {
                metadata.add_warning(
                    Warning::new("truncated-id3-frame", "ID3 frame header is truncated")
                        .at(body_file_offset + cursor as u64),
                );
            }
            return;
        }
        let id = &body[cursor..cursor + id_length];
        if id.iter().all(|byte| *byte == 0) {
            return;
        }
        if !valid_frame_id(id) {
            metadata.add_warning(
                Warning::new(
                    "invalid-id3-frame-id",
                    format!("invalid ID3 frame identifier {}", display_frame_id(id)),
                )
                .at(body_file_offset + cursor as u64),
            );
            return;
        }
        let size = if version == 2 {
            (u32::from(id3_byte(body, cursor + 3)) << 16)
                | (u32::from(id3_byte(body, cursor + 4)) << 8)
                | u32::from(id3_byte(body, cursor + 5))
        } else if version == 4 {
            match parse_synchsafe(&body[cursor + 4..cursor + 8], "ID3v2.4 frame size") {
                Ok(value) => value,
                Err(error) => {
                    metadata.add_warning(
                        Warning::new("invalid-id3-frame-size", error.to_string())
                            .at(body_file_offset + cursor as u64),
                    );
                    return;
                }
            }
        } else {
            u32::from_be_bytes(
                body[cursor + 4..cursor + 8]
                    .try_into()
                    .expect("ID3v2.3 frame size"),
            )
        };
        let Ok(size) = usize::try_from(size) else {
            metadata.add_warning(
                Warning::new("invalid-id3-frame-size", "ID3 frame size does not fit")
                    .at(body_file_offset + cursor as u64),
            );
            return;
        };
        let payload_start = cursor + header_length;
        let Some(payload_end) = payload_start.checked_add(size) else {
            metadata.add_warning(
                Warning::new("invalid-id3-frame-size", "ID3 frame size overflows")
                    .at(body_file_offset + cursor as u64),
            );
            return;
        };
        if payload_end > body.len() {
            metadata.add_warning(
                Warning::new(
                    "truncated-id3-frame",
                    "ID3 frame extends beyond the tag body",
                )
                .at(body_file_offset + cursor as u64),
            );
            return;
        }
        let flags = if version == 2 {
            0
        } else {
            u16::from_be_bytes(
                body[cursor + 8..cursor + 10]
                    .try_into()
                    .expect("ID3 frame flags"),
            )
        };
        let payload = &body[payload_start..payload_end];
        if payload.len() > limits.max_value_bytes {
            metadata.add_warning(
                Warning::new(
                    "id3-value-limit",
                    format!(
                        "ID3 frame {} exceeds the value budget",
                        display_frame_id(id)
                    ),
                )
                .at(body_file_offset + payload_start as u64),
            );
        } else {
            let mut normalized = if flags & 0x0002 != 0 {
                remove_unsynchronization(payload)
            } else {
                payload.to_vec()
            };
            if version == 4 && flags & 0x0001 != 0 {
                if normalized.len() < 4 {
                    metadata.add_warning(
                        Warning::new(
                            "invalid-id3-data-length",
                            "ID3v2.4 frame is missing its data length indicator",
                        )
                        .at(body_file_offset + payload_start as u64),
                    );
                    cursor = payload_end;
                    frame_count += 1;
                    continue;
                }
                normalized.drain(..4);
            }
            parse_frame(
                id,
                version,
                &normalized,
                body_file_offset + payload_start as u64,
                limits,
                metadata,
            );
        }
        cursor = payload_end;
        frame_count += 1;
    }
}

fn parse_frame(
    id: &[u8],
    version: u8,
    payload: &[u8],
    payload_offset: u64,
    limits: ParseLimits,
    metadata: &mut Metadata,
) {
    let frame_id = display_frame_id(id);
    let group = format!("ID3v{version}");
    let raw_length = payload.len() as u64;
    if let Some(name) = text_frame_name(id) {
        if let Some((value, value_type)) = decode_text_frame(payload) {
            add_tag(
                metadata,
                name,
                value,
                value_type,
                &group,
                frame_numeric_id(id),
                Some(payload),
                payload_offset,
                raw_length,
            );
        } else {
            metadata.add_warning(
                Warning::new(
                    "invalid-id3-text",
                    format!("ID3 text frame {frame_id} has no decodable value"),
                )
                .at(payload_offset),
            );
        }
        return;
    }
    if id == b"TXXX" || id == b"TXX" {
        parse_user_text(
            payload,
            &group,
            frame_numeric_id(id),
            payload_offset,
            metadata,
        );
        return;
    }
    if id == b"COMM" || id == b"COM" {
        parse_comment(
            payload,
            &group,
            frame_numeric_id(id),
            payload_offset,
            metadata,
        );
        return;
    }
    if id == b"USLT" || id == b"ULT" {
        parse_lyrics(
            payload,
            &group,
            frame_numeric_id(id),
            payload_offset,
            metadata,
        );
        return;
    }
    if id == b"APIC" || id == b"PIC" {
        parse_picture(
            payload,
            &group,
            frame_numeric_id(id),
            payload_offset,
            limits,
            metadata,
        );
        return;
    }
    if id == b"WXXX" || id == b"WXX" {
        parse_user_url(
            payload,
            &group,
            frame_numeric_id(id),
            payload_offset,
            metadata,
        );
        return;
    }
    if is_url_frame(id) {
        if let Some(value) = decode_latin1_value(payload) {
            add_tag(
                metadata,
                &frame_id,
                TagValue::String(value),
                ValueType::String,
                &group,
                frame_numeric_id(id),
                Some(payload),
                payload_offset,
                raw_length,
            );
        }
        return;
    }
    if id == b"PCNT" || id == b"CNT" {
        if let Some(value) = unsigned_be(payload) {
            add_tag(
                metadata,
                "PlayCount",
                TagValue::Unsigned(value),
                ValueType::UnsignedInteger,
                &group,
                frame_numeric_id(id),
                Some(payload),
                payload_offset,
                raw_length,
            );
        }
        return;
    }
    if id == b"POPM" || id == b"POP" {
        if let Some(rating) = payload.iter().rev().nth(1).copied() {
            add_tag(
                metadata,
                "Rating",
                TagValue::Unsigned(u64::from(rating)),
                ValueType::UnsignedInteger,
                &group,
                frame_numeric_id(id),
                Some(payload),
                payload_offset,
                raw_length,
            );
        }
        return;
    }
    if is_text_like_frame(id)
        && let Some((value, value_type)) = decode_text_frame(payload)
    {
        add_tag(
            metadata,
            &frame_id,
            value,
            value_type,
            &group,
            frame_numeric_id(id),
            Some(payload),
            payload_offset,
            raw_length,
        );
        return;
    }
    add_tag(
        metadata,
        &frame_id,
        TagValue::Bytes(payload.to_vec()),
        ValueType::Bytes,
        &group,
        frame_numeric_id(id),
        Some(payload),
        payload_offset,
        raw_length,
    );
}

fn parse_user_text(
    payload: &[u8],
    group: &str,
    id: Option<u32>,
    offset: u64,
    metadata: &mut Metadata,
) {
    let Some(encoding) = payload.first().copied() else {
        return;
    };
    let Some((description_bytes, values_bytes)) = split_first_encoded(&payload[1..], encoding)
    else {
        metadata.add_warning(
            Warning::new("invalid-id3-txxx", "ID3 user-text frame has no description").at(offset),
        );
        return;
    };
    let description = decode_text_value(description_bytes, encoding)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "Value".to_owned());
    let Some((value, value_type)) = decode_text_values(encoding, values_bytes) else {
        return;
    };
    add_tag(
        metadata,
        &format!("UserText:{description}"),
        value,
        value_type,
        group,
        id,
        Some(payload),
        offset,
        payload.len() as u64,
    );
}

fn parse_comment(
    payload: &[u8],
    group: &str,
    id: Option<u32>,
    offset: u64,
    metadata: &mut Metadata,
) {
    if payload.len() < 4 {
        metadata.add_warning(
            Warning::new(
                "truncated-id3-comment",
                "ID3 comment frame is shorter than its header",
            )
            .at(offset),
        );
        return;
    }
    let encoding = payload[0];
    let Some((_, text_bytes)) = split_first_encoded(&payload[4..], encoding) else {
        metadata.add_warning(
            Warning::new(
                "invalid-id3-comment",
                "ID3 comment frame has no text separator",
            )
            .at(offset),
        );
        return;
    };
    let Some((value, value_type)) = decode_text_values(encoding, text_bytes) else {
        return;
    };
    add_tag(
        metadata,
        "Comment",
        value,
        value_type,
        group,
        id,
        Some(payload),
        offset,
        payload.len() as u64,
    );
}

fn parse_lyrics(
    payload: &[u8],
    group: &str,
    id: Option<u32>,
    offset: u64,
    metadata: &mut Metadata,
) {
    if payload.len() < 4 {
        metadata.add_warning(
            Warning::new(
                "truncated-id3-lyrics",
                "ID3 lyrics frame is shorter than its header",
            )
            .at(offset),
        );
        return;
    }
    let encoding = payload[0];
    let Some((_, text_bytes)) = split_first_encoded(&payload[4..], encoding) else {
        metadata.add_warning(
            Warning::new(
                "invalid-id3-lyrics",
                "ID3 lyrics frame has no text separator",
            )
            .at(offset),
        );
        return;
    };
    let Some((value, value_type)) = decode_text_values(encoding, text_bytes) else {
        return;
    };
    add_tag(
        metadata,
        "Lyrics",
        value,
        value_type,
        group,
        id,
        Some(payload),
        offset,
        payload.len() as u64,
    );
}

fn parse_picture(
    payload: &[u8],
    group: &str,
    id: Option<u32>,
    offset: u64,
    limits: ParseLimits,
    metadata: &mut Metadata,
) {
    let Some(encoding) = payload.first().copied() else {
        return;
    };
    let Some(mime_end) = payload[1..].iter().position(|byte| *byte == 0) else {
        metadata.add_warning(
            Warning::new(
                "invalid-id3-picture",
                "attached-picture MIME type is unterminated",
            )
            .at(offset),
        );
        return;
    };
    let mime_end = 1 + mime_end;
    let mime = String::from_utf8_lossy(&payload[1..mime_end]).into_owned();
    let Some(picture_type) = payload.get(mime_end + 1).copied() else {
        return;
    };
    let description_start = mime_end + 2;
    let Some((_, image)) = split_first_encoded(&payload[description_start..], encoding) else {
        metadata.add_warning(
            Warning::new(
                "invalid-id3-picture",
                "attached-picture description is unterminated",
            )
            .at(offset),
        );
        return;
    };
    if image.len() > limits.max_value_bytes {
        metadata.add_warning(
            Warning::new(
                "id3-picture-limit",
                "attached-picture payload exceeded the value budget",
            )
            .at(offset + description_start as u64),
        );
        return;
    }
    add_tag(
        metadata,
        "AttachedPicture",
        TagValue::Bytes(image.to_vec()),
        ValueType::Bytes,
        group,
        id,
        Some(image),
        offset + (payload.len() - image.len()) as u64,
        image.len() as u64,
    );
    add_tag(
        metadata,
        "AttachedPictureMimeType",
        TagValue::String(mime),
        ValueType::String,
        group,
        id,
        None,
        offset + 1,
        (mime_end - 1) as u64,
    );
    add_tag(
        metadata,
        "AttachedPictureType",
        TagValue::Unsigned(u64::from(picture_type)),
        ValueType::UnsignedInteger,
        group,
        id,
        None,
        offset + mime_end as u64 + 1,
        1,
    );
}

fn parse_user_url(
    payload: &[u8],
    group: &str,
    id: Option<u32>,
    offset: u64,
    metadata: &mut Metadata,
) {
    let Some(encoding) = payload.first().copied() else {
        return;
    };
    let Some((description_bytes, url_bytes)) = split_first_encoded(&payload[1..], encoding) else {
        metadata.add_warning(
            Warning::new(
                "invalid-id3-wxxx",
                "ID3 user URL frame has no description separator",
            )
            .at(offset),
        );
        return;
    };
    let description = decode_text_value(description_bytes, encoding)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "Value".to_owned());
    let url = String::from_utf8_lossy(url_bytes)
        .trim_end_matches('\0')
        .to_owned();
    if url.is_empty() {
        return;
    }
    add_tag(
        metadata,
        &format!("UserUrl:{description}"),
        TagValue::String(url),
        ValueType::String,
        group,
        id,
        Some(payload),
        offset,
        payload.len() as u64,
    );
}

fn parse_id3v1<R: Read + Seek>(
    reader: &mut R,
    path: &Path,
    file_length: u64,
    limits: ParseLimits,
    metadata: &mut Metadata,
) -> Result<()> {
    let offset = file_length - ID3V1_LENGTH;
    let data = read_at(reader, offset, ID3V1_LENGTH as usize, path, "ID3v1 tag")?;
    if &data[..3] != b"TAG" {
        return Ok(());
    }
    let group = "ID3v1";
    let fields = [
        ("Title", 3_usize, 30_usize),
        ("Artist", 33, 30),
        ("Album", 63, 30),
        ("Year", 93, 4),
    ];
    for (name, start, length) in fields {
        let value = decode_latin1_value(&data[start..start + length]);
        if let Some(value) = value {
            add_tag(
                metadata,
                name,
                TagValue::String(value),
                ValueType::String,
                group,
                None,
                Some(&data[start..start + length]),
                offset + start as u64,
                length as u64,
            );
        }
    }
    let comment = if data[125] == 0 && data[126] != 0 {
        &data[97..125]
    } else {
        &data[97..127]
    };
    if let Some(value) = decode_latin1_value(comment) {
        add_tag(
            metadata,
            "Comment",
            TagValue::String(value),
            ValueType::String,
            group,
            None,
            Some(comment),
            offset + 97,
            comment.len() as u64,
        );
    }
    if data[125] == 0 && data[126] != 0 {
        add_tag(
            metadata,
            "TrackNumber",
            TagValue::Unsigned(u64::from(data[126])),
            ValueType::UnsignedInteger,
            group,
            None,
            Some(&data[126..127]),
            offset + 126,
            1,
        );
    }
    if limits.max_value_bytes >= 1 {
        add_tag(
            metadata,
            "GenreIndex",
            TagValue::Unsigned(u64::from(data[127])),
            ValueType::UnsignedInteger,
            group,
            None,
            Some(&data[127..128]),
            offset + 127,
            1,
        );
    }
    Ok(())
}

fn parse_mpeg_header<R: Read + Seek>(
    reader: &mut R,
    path: &Path,
    offset: u64,
    file_length: u64,
    metadata: &mut Metadata,
) -> Result<()> {
    let Some(end) = offset.checked_add(4) else {
        return Ok(());
    };
    if end > file_length {
        return Ok(());
    }
    let header = read_at(reader, offset, 4, path, "MPEG audio frame header")?;
    if header[0] != 0xFF || header[1] & 0xE0 != 0xE0 {
        return Ok(());
    }
    let version_bits = (header[1] >> 3) & 0x03;
    let layer_bits = (header[1] >> 1) & 0x03;
    if version_bits == 1 || layer_bits == 0 {
        metadata.add_warning(
            Warning::new(
                "invalid-mpeg-header",
                "MPEG frame has a reserved version or layer",
            )
            .at(offset),
        );
        return Ok(());
    }
    let version = match version_bits {
        3 => "MPEG 1",
        2 => "MPEG 2",
        0 => "MPEG 2.5",
        _ => unreachable!("reserved MPEG version was handled above"),
    };
    let layer = match layer_bits {
        3 => "Layer I",
        2 => "Layer II",
        1 => "Layer III",
        _ => unreachable!("reserved MPEG layer was handled above"),
    };
    add_mpeg_tag(
        metadata,
        "Version",
        TagValue::String(version.to_owned()),
        ValueType::String,
        offset + 1,
        1,
    );
    add_mpeg_tag(
        metadata,
        "Layer",
        TagValue::String(layer.to_owned()),
        ValueType::String,
        offset + 1,
        1,
    );
    let bitrate_index = usize::from(header[2] >> 4);
    if let Some(bitrate) = bitrate(version_bits, layer_bits, bitrate_index) {
        add_mpeg_tag(
            metadata,
            "BitrateKbps",
            TagValue::Unsigned(u64::from(bitrate)),
            ValueType::UnsignedInteger,
            offset + 2,
            1,
        );
    }
    let sample_rate_index = usize::from((header[2] >> 2) & 0x03);
    if let Some(sample_rate) = sample_rate(version_bits, sample_rate_index) {
        add_mpeg_tag(
            metadata,
            "SampleRateHz",
            TagValue::Unsigned(u64::from(sample_rate)),
            ValueType::UnsignedInteger,
            offset + 2,
            1,
        );
    }
    let channel_mode = match header[3] >> 6 {
        0 => "Stereo",
        1 => "JointStereo",
        2 => "DualChannel",
        _ => "Mono",
    };
    add_mpeg_tag(
        metadata,
        "ChannelMode",
        TagValue::String(channel_mode.to_owned()),
        ValueType::String,
        offset + 3,
        1,
    );
    Ok(())
}

fn bitrate(version: u8, layer: u8, index: usize) -> Option<u16> {
    const MPEG1_LAYER1: [u16; 16] = [
        0, 32, 64, 96, 128, 160, 192, 224, 256, 288, 320, 352, 384, 0, 0, 0,
    ];
    const MPEG1_LAYER2: [u16; 16] = [
        0, 32, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384, 0,
    ];
    const MPEG1_LAYER3: [u16; 16] = [
        0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 0,
    ];
    const MPEG2_LAYER1: [u16; 16] = [
        0, 32, 48, 56, 64, 80, 96, 112, 128, 144, 160, 176, 192, 224, 256, 0,
    ];
    const MPEG2_LAYER2_3: [u16; 16] = [
        0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160, 0,
    ];
    let table = if version == 3 {
        match layer {
            3 => &MPEG1_LAYER1,
            2 => &MPEG1_LAYER2,
            1 => &MPEG1_LAYER3,
            _ => return None,
        }
    } else if layer == 3 {
        &MPEG2_LAYER1
    } else {
        &MPEG2_LAYER2_3
    };
    table.get(index).copied().filter(|value| *value != 0)
}

fn sample_rate(version: u8, index: usize) -> Option<u32> {
    let rates = match version {
        3 => [44_100, 48_000, 32_000],
        2 => [22_050, 24_000, 16_000],
        0 => [11_025, 12_000, 8_000],
        _ => return None,
    };
    rates.get(index).copied()
}

fn decode_text_frame(payload: &[u8]) -> Option<(TagValue, ValueType)> {
    let encoding = payload.first().copied()?;
    decode_text_values(encoding, &payload[1..])
}

fn decode_text_values(encoding: u8, bytes: &[u8]) -> Option<(TagValue, ValueType)> {
    if !matches!(encoding, 0..=3) {
        return None;
    }
    let values = split_encoded(bytes, encoding)
        .into_iter()
        .filter_map(|value| decode_text_value(value, encoding))
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    match values.as_slice() {
        [] => None,
        [value] => Some((TagValue::String(value.clone()), ValueType::String)),
        values => Some((
            TagValue::Array(values.iter().cloned().map(TagValue::String).collect()),
            ValueType::Array,
        )),
    }
}

fn decode_latin1_value(bytes: &[u8]) -> Option<String> {
    let value = decode_latin1(bytes)
        .trim_matches(|character| character == '\0' || character == ' ')
        .to_owned();
    (!value.is_empty()).then_some(value)
}

fn decode_text_value(bytes: &[u8], encoding: u8) -> Option<String> {
    let value = match encoding {
        0 => decode_latin1(bytes),
        1 | 2 => decode_utf16(bytes, encoding),
        3 => String::from_utf8_lossy(bytes).into_owned(),
        _ => return None,
    };
    Some(value.trim_matches('\0').to_owned())
}

fn decode_latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| char::from(*byte)).collect()
}

fn decode_utf16(bytes: &[u8], encoding: u8) -> String {
    let mut byte_order_be = encoding == 2;
    let mut start = 0_usize;
    if bytes.len() >= 2 {
        match &bytes[..2] {
            [0xFE, 0xFF] => {
                byte_order_be = true;
                start = 2;
            }
            [0xFF, 0xFE] => {
                byte_order_be = false;
                start = 2;
            }
            _ => {}
        }
    }
    let units = bytes[start..]
        .chunks_exact(2)
        .map(|pair| {
            if byte_order_be {
                u16::from_be_bytes([pair[0], pair[1]])
            } else {
                u16::from_le_bytes([pair[0], pair[1]])
            }
        })
        .collect::<Vec<_>>();
    String::from_utf16_lossy(&units)
}

fn split_first_encoded(bytes: &[u8], encoding: u8) -> Option<(&[u8], &[u8])> {
    let width = if matches!(encoding, 1 | 2) { 2 } else { 1 };
    if width == 1 {
        let index = bytes.iter().position(|byte| *byte == 0)?;
        Some((&bytes[..index], &bytes[index + 1..]))
    } else {
        let index = bytes.windows(2).position(|window| window == [0, 0])?;
        Some((&bytes[..index], &bytes[index + 2..]))
    }
}

fn split_encoded(bytes: &[u8], encoding: u8) -> Vec<&[u8]> {
    let width = if matches!(encoding, 1 | 2) { 2 } else { 1 };
    let mut values = Vec::new();
    let mut start = 0_usize;
    let mut cursor = 0_usize;
    while cursor + width <= bytes.len() {
        let is_separator = if width == 1 {
            bytes[cursor] == 0
        } else {
            bytes[cursor..cursor + 2] == [0, 0]
        };
        if is_separator {
            values.push(&bytes[start..cursor]);
            cursor += width;
            start = cursor;
        } else {
            cursor += width;
        }
    }
    values.push(&bytes[start..]);
    values
}

fn text_frame_name(id: &[u8]) -> Option<&'static str> {
    Some(match id {
        b"TIT2" | b"TT2" => "Title",
        b"TPE1" | b"TP1" => "Artist",
        b"TPE2" | b"TP2" => "AlbumArtist",
        b"TALB" | b"TAL" => "Album",
        b"TDRC" | b"TYER" | b"TYE" => "RecordingDate",
        b"TCON" | b"TCO" => "Genre",
        b"TRCK" | b"TRK" => "TrackNumber",
        b"TPOS" | b"TPA" => "DiscNumber",
        b"TCOM" | b"TCM" => "Composer",
        b"TBPM" => "BPM",
        b"TLEN" | b"TLE" => "DurationMilliseconds",
        b"TCOP" | b"TCR" => "Copyright",
        b"TPUB" | b"TPB" => "Publisher",
        b"TENC" | b"TEN" => "EncodedBy",
        b"TSSE" | b"TSS" => "EncoderSettings",
        b"TSOA" => "AlbumSortOrder",
        b"TSOP" => "ArtistSortOrder",
        b"TSOT" => "TitleSortOrder",
        b"TDOR" => "OriginalReleaseDate",
        b"TDRL" => "ReleaseDate",
        b"TKEY" | b"TKE" => "InitialKey",
        b"TLAN" | b"TLA" => "Language",
        b"TT1" => "ContentGroup",
        b"TT3" => "Subtitle",
        b"TFLT" => "FileType",
        b"TMED" | b"TMT" => "MediaType",
        _ => return None,
    })
}

fn is_text_like_frame(id: &[u8]) -> bool {
    id.first() == Some(&b'T') && id != b"TXXX" && id != b"TXX"
}

fn is_url_frame(id: &[u8]) -> bool {
    matches!(
        id,
        b"WCOM"
            | b"WCOP"
            | b"WOAF"
            | b"WOAR"
            | b"WOAS"
            | b"WORS"
            | b"WPAY"
            | b"WPUB"
            | b"WCM"
            | b"WCP"
            | b"WAF"
            | b"WAR"
            | b"WAS"
            | b"WRS"
            | b"WPB"
    )
}

fn valid_frame_id(id: &[u8]) -> bool {
    id.iter().all(|byte| byte.is_ascii_alphanumeric())
}

fn display_frame_id(id: &[u8]) -> String {
    String::from_utf8_lossy(id).into_owned()
}

fn frame_numeric_id(id: &[u8]) -> Option<u32> {
    match id {
        [a, b, c] => Some(u32::from_be_bytes([0, *a, *b, *c])),
        [a, b, c, d] => Some(u32::from_be_bytes([*a, *b, *c, *d])),
        _ => None,
    }
}

fn unsigned_be(bytes: &[u8]) -> Option<u64> {
    (!bytes.is_empty()).then(|| {
        bytes
            .iter()
            .fold(0_u64, |value, byte| (value << 8) | u64::from(*byte))
    })
}

fn parse_synchsafe(bytes: &[u8], context: &str) -> Result<u32> {
    let mut value = 0_u32;
    for byte in bytes {
        if byte & 0x80 != 0 {
            return Err(MetraError::InvalidTag {
                context: context.to_owned(),
                message: "synchsafe byte has its high bit set".to_owned(),
            });
        }
        value = (value << 7) | u32::from(*byte);
    }
    Ok(value)
}

fn remove_unsynchronization(bytes: &[u8]) -> Vec<u8> {
    let mut result = Vec::with_capacity(bytes.len());
    let mut cursor = 0_usize;
    while cursor < bytes.len() {
        let byte = bytes[cursor];
        result.push(byte);
        cursor += 1;
        if byte == 0xFF && bytes.get(cursor) == Some(&0) {
            cursor += 1;
        }
    }
    result
}

fn add_header_tag(metadata: &mut Metadata, name: &str, value: TagValue, offset: u64, length: u64) {
    let value_type = match value {
        TagValue::String(_) => ValueType::String,
        TagValue::Unsigned(_) => ValueType::UnsignedInteger,
        _ => ValueType::Unknown,
    };
    add_tag(
        metadata, name, value, value_type, "Header", None, None, offset, length,
    );
}

fn add_mpeg_tag(
    metadata: &mut Metadata,
    name: &str,
    value: TagValue,
    value_type: ValueType,
    offset: u64,
    length: u64,
) {
    add_tag(
        metadata,
        name,
        value,
        value_type,
        "MPEGFrame",
        None,
        None,
        offset,
        length,
    );
}

#[allow(clippy::too_many_arguments)]
fn add_tag(
    metadata: &mut Metadata,
    name: &str,
    value: TagValue,
    value_type: ValueType,
    group: &str,
    id: Option<u32>,
    raw_value: Option<&[u8]>,
    offset: u64,
    length: u64,
) {
    metadata.add_tag(Tag {
        namespace: "ID3".to_owned(),
        group: group.to_owned(),
        id,
        name: name.to_owned(),
        description: Some("MP3/ID3 metadata".to_owned()),
        raw_value: raw_value.map(<[u8]>::to_vec),
        value,
        value_type,
        source: Source::new("MP3", Some(offset), Some(length)),
        writable: false,
    });
}

fn id3_byte(bytes: &[u8], index: usize) -> u8 {
    bytes[index]
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

fn read_exact<R: Read>(reader: &mut R, bytes: &mut [u8], path: &Path) -> Result<()> {
    reader
        .read_exact(bytes)
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

    fn synchsafe(value: usize) -> [u8; 4] {
        [
            ((value >> 21) & 0x7F) as u8,
            ((value >> 14) & 0x7F) as u8,
            ((value >> 7) & 0x7F) as u8,
            (value & 0x7F) as u8,
        ]
    }

    fn frame(id: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut result = id.to_vec();
        result.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        result.extend_from_slice(&[0, 0]);
        result.extend_from_slice(payload);
        result
    }

    fn id3_file(frames: &[u8]) -> Vec<u8> {
        let mut result = b"ID3".to_vec();
        result.extend_from_slice(&[4, 0, 0]);
        result.extend_from_slice(&synchsafe(frames.len()));
        result.extend_from_slice(frames);
        result.extend_from_slice(&[0xFF, 0xFB, 0x90, 0x64]);
        result
    }

    #[test]
    fn reads_id3_text_and_mpeg_header() {
        let mut frames = frame(b"TIT2", &[3, b'M', b'e', b't', b'r', b'a']);
        frames.extend(frame(b"TPE1", &[3, b'A', b'r', b't', b'i', b's', b't']));
        let bytes = id3_file(&frames);
        let info = FileInfo::new("song.mp3".into(), bytes.len() as u64, FileFormat::Mp3);
        let metadata = read_mp3(&mut Cursor::new(bytes), info, ParseLimits::default()).unwrap();
        assert_eq!(metadata.find("ID3:Title").unwrap().display_value(), "Metra");
        assert_eq!(
            metadata.find("ID3:Artist").unwrap().display_value(),
            "Artist"
        );
        assert_eq!(
            metadata.find("ID3:SampleRateHz").unwrap().display_value(),
            "44100"
        );
        assert_eq!(
            metadata.find("ID3:BitrateKbps").unwrap().display_value(),
            "128"
        );
    }

    #[test]
    fn reads_id3v1_when_v2_is_absent() {
        let mut bytes = vec![0xFF, 0xFB, 0x90, 0x64];
        bytes.extend_from_slice(&[0; 16]);
        let mut tag = [0_u8; 128];
        tag[..3].copy_from_slice(b"TAG");
        tag[3..8].copy_from_slice(b"Metra");
        tag[33..39].copy_from_slice(b"Artist");
        tag[125] = 0;
        tag[126] = 7;
        bytes.extend_from_slice(&tag);
        let info = FileInfo::new("legacy.mp3".into(), bytes.len() as u64, FileFormat::Mp3);
        let metadata = read_mp3(&mut Cursor::new(bytes), info, ParseLimits::default()).unwrap();
        assert_eq!(metadata.find("ID3:Title").unwrap().display_value(), "Metra");
        assert_eq!(
            metadata.find("ID3:TrackNumber").unwrap().display_value(),
            "7"
        );
    }

    #[test]
    fn rejects_non_synchsafe_tag_size() {
        let mut bytes = b"ID3".to_vec();
        bytes.extend_from_slice(&[4, 0, 0, 0x80, 0, 0, 0, 0]);
        let info = FileInfo::new("bad.mp3".into(), bytes.len() as u64, FileFormat::Mp3);
        let result = read_mp3(&mut Cursor::new(bytes), info, ParseLimits::default());
        assert!(matches!(result, Err(MetraError::InvalidTag { .. })));
    }
}
