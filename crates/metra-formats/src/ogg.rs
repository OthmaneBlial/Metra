use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use metra_core::{
    FileInfo, Metadata, MetraError, ParseLimits, Result, Source, Tag, TagValue, ValueType, Warning,
};

const OGG_PAGE_HEADER_LENGTH: usize = 27;
const MAX_PACKETS_PER_STREAM: usize = 3;

#[derive(Debug, Default)]
struct StreamState {
    pending: Vec<u8>,
    packet_offset: Option<u64>,
    packets_seen: usize,
    exhausted: bool,
}

struct OggPage<'a> {
    continued: bool,
    lacing: &'a [u8],
    body: &'a [u8],
    body_start: u64,
    sequence: u32,
}

pub fn read_ogg<R: Read + Seek>(
    reader: &mut R,
    file_info: FileInfo,
    limits: ParseLimits,
) -> Result<Metadata> {
    let path = file_info.path.clone();
    let file_length = file_info.size;
    if file_length < OGG_PAGE_HEADER_LENGTH as u64 {
        return Err(MetraError::InvalidHeader {
            context: "OGG".to_owned(),
            message: "file is shorter than an Ogg page header".to_owned(),
        });
    }

    let mut metadata = Metadata::new(file_info);
    let mut cursor = 0_u64;
    let mut page_count = 0_usize;
    let mut metadata_bytes = 0_usize;
    let mut streams = BTreeMap::<u32, StreamState>::new();
    while cursor < file_length {
        if page_count >= limits.max_jpeg_segments {
            metadata
                .add_warning(Warning::new("ogg-page-limit", "Ogg page limit reached").at(cursor));
            break;
        }
        if file_length.saturating_sub(cursor) < OGG_PAGE_HEADER_LENGTH as u64 {
            metadata.add_warning(
                Warning::new("truncated-ogg-page", "Ogg page header is truncated").at(cursor),
            );
            break;
        }
        let header = read_at(
            reader,
            cursor,
            OGG_PAGE_HEADER_LENGTH,
            file_length,
            &path,
            "Ogg page header",
        )?;
        if &header[..4] != b"OggS" {
            if page_count == 0 {
                return Err(MetraError::InvalidHeader {
                    context: "OGG".to_owned(),
                    message: "expected OggS page signature".to_owned(),
                });
            }
            metadata.add_warning(
                Warning::new("invalid-ogg-page", "Ogg page signature is invalid").at(cursor),
            );
            break;
        }
        if header[4] != 0 {
            metadata.add_warning(
                Warning::new(
                    "unsupported-ogg-version",
                    format!("Ogg bitstream version {} is not supported", header[4]),
                )
                .at(cursor + 4),
            );
        }
        let serial = u32::from_le_bytes(header[14..18].try_into().expect("Ogg serial number"));
        let sequence = u32::from_le_bytes(header[18..22].try_into().expect("Ogg sequence number"));
        let segment_count = usize::from(header[26]);
        let lacing_offset = cursor + OGG_PAGE_HEADER_LENGTH as u64;
        let lacing = read_at(
            reader,
            lacing_offset,
            segment_count,
            file_length,
            &path,
            "Ogg segment table",
        )?;
        let body_length = lacing
            .iter()
            .try_fold(0_u64, |total, segment| {
                total.checked_add(u64::from(*segment))
            })
            .ok_or(MetraError::InvalidOffset {
                context: "Ogg page body length".to_owned(),
                offset: cursor,
            })?;
        let body_start =
            lacing_offset
                .checked_add(segment_count as u64)
                .ok_or(MetraError::InvalidOffset {
                    context: "Ogg page body".to_owned(),
                    offset: cursor,
                })?;
        let body_end = body_start
            .checked_add(body_length)
            .ok_or(MetraError::InvalidOffset {
                context: "Ogg page end".to_owned(),
                offset: body_start,
            })?;
        if body_end > file_length {
            metadata.add_warning(
                Warning::new("truncated-ogg-page", "Ogg page body exceeds the file").at(body_start),
            );
            break;
        }

        let needs_packets = streams
            .get(&serial)
            .is_none_or(|state| !state.exhausted && state.packets_seen < MAX_PACKETS_PER_STREAM);
        if needs_packets {
            let body_length_usize =
                usize::try_from(body_length).map_err(|_| MetraError::ResourceLimitExceeded {
                    resource: "Ogg page body".to_owned(),
                    limit: limits.max_value_bytes,
                })?;
            let next_metadata_bytes = metadata_bytes.checked_add(body_length_usize).ok_or(
                MetraError::ResourceLimitExceeded {
                    resource: "Ogg metadata pages".to_owned(),
                    limit: limits.max_metadata_bytes,
                },
            )?;
            if next_metadata_bytes > limits.max_metadata_bytes {
                metadata.add_warning(
                    Warning::new("ogg-metadata-limit", "Ogg metadata page budget reached")
                        .at(body_start),
                );
                streams.entry(serial).or_default().exhausted = true;
            } else if streams.len() >= limits.max_jpeg_segments && !streams.contains_key(&serial) {
                metadata.add_warning(
                    Warning::new("ogg-stream-limit", "Ogg logical stream limit reached").at(cursor),
                );
            } else {
                let body = read_at(
                    reader,
                    body_start,
                    body_length_usize,
                    file_length,
                    &path,
                    "Ogg page body",
                )?;
                metadata_bytes = next_metadata_bytes;
                let state = streams.entry(serial).or_default();
                process_page(
                    OggPage {
                        continued: header[5] & 0x01 != 0,
                        lacing: &lacing,
                        body: &body,
                        body_start,
                        sequence,
                    },
                    state,
                    &mut metadata,
                    limits,
                );
            }
        }
        cursor = body_end;
        page_count += 1;
    }

    if page_count == 0 {
        return Err(MetraError::InvalidHeader {
            context: "OGG".to_owned(),
            message: "no complete Ogg pages were found".to_owned(),
        });
    }
    add_tag(
        &mut metadata,
        "PageCount",
        TagValue::Unsigned(page_count as u64),
        ValueType::UnsignedInteger,
        "Container",
        0,
        cursor,
    );
    metadata.sort_tags();
    Ok(metadata)
}

pub(crate) fn is_ogg_signature(bytes: &[u8]) -> bool {
    bytes.starts_with(b"OggS")
}

fn process_page(
    page: OggPage<'_>,
    state: &mut StreamState,
    metadata: &mut Metadata,
    limits: ParseLimits,
) {
    if !page.continued && !state.pending.is_empty() {
        metadata.add_warning(
            Warning::new(
                "ogg-packet-reset",
                format!(
                    "page {} starts before the previous packet ended",
                    page.sequence
                ),
            )
            .at(page.body_start),
        );
        state.pending.clear();
        state.packet_offset = None;
    }

    let mut body_cursor = 0_usize;
    for segment_length in page.lacing.iter().map(|length| usize::from(*length)) {
        let Some(segment_end) = body_cursor.checked_add(segment_length) else {
            state.exhausted = true;
            return;
        };
        let Some(segment) = page.body.get(body_cursor..segment_end) else {
            state.exhausted = true;
            return;
        };
        if !state.exhausted && state.packets_seen < MAX_PACKETS_PER_STREAM {
            if state.pending.is_empty() {
                state.packet_offset = Some(page.body_start + body_cursor as u64);
            }
            let Some(packet_length) = state.pending.len().checked_add(segment.len()) else {
                state.exhausted = true;
                state.pending.clear();
                state.packet_offset = None;
                return;
            };
            if packet_length > limits.max_value_bytes {
                metadata.add_warning(
                    Warning::new("ogg-packet-limit", "Ogg packet value budget reached")
                        .at(state.packet_offset.unwrap_or(page.body_start)),
                );
                state.exhausted = true;
                state.pending.clear();
                state.packet_offset = None;
            } else {
                state.pending.extend_from_slice(segment);
            }
            if segment_length < 255 {
                let packet = std::mem::take(&mut state.pending);
                let packet_offset = state.packet_offset.take().unwrap_or(page.body_start);
                state.packets_seen += 1;
                parse_packet(&packet, packet_offset, metadata, limits);
            }
        }
        body_cursor = segment_end;
    }
}

fn parse_packet(packet: &[u8], offset: u64, metadata: &mut Metadata, limits: ParseLimits) {
    if packet.starts_with(b"OpusHead") {
        parse_opus_head(packet, offset, metadata);
    } else if packet.starts_with(b"OpusTags") {
        parse_comments(packet, 8, offset, metadata, limits, "OpusTags");
    } else if packet.starts_with(&[1]) && packet.get(1..7) == Some(b"vorbis") {
        parse_vorbis_identification(packet, offset, metadata);
    } else if packet.starts_with(&[3]) && packet.get(1..7) == Some(b"vorbis") {
        parse_comments(packet, 7, offset, metadata, limits, "VORBIS_COMMENT");
    } else if packet.starts_with(&[0x7F]) && packet.get(1..5) == Some(b"FLAC") {
        add_tag(
            metadata,
            "Codec",
            TagValue::String("FLAC (Ogg mapping)".to_owned()),
            ValueType::String,
            "Stream",
            offset,
            packet.len() as u64,
        );
    }
}

fn parse_vorbis_identification(packet: &[u8], offset: u64, metadata: &mut Metadata) {
    if packet.len() < 30 {
        metadata.add_warning(
            Warning::new(
                "truncated-vorbis-identification",
                "Vorbis identification packet is shorter than its fixed fields",
            )
            .at(offset),
        );
        return;
    }
    add_tag(
        metadata,
        "Codec",
        TagValue::String("Vorbis".to_owned()),
        ValueType::String,
        "Stream",
        offset + 1,
        6,
    );
    add_tag(
        metadata,
        "Channels",
        TagValue::Unsigned(u64::from(packet[11])),
        ValueType::UnsignedInteger,
        "Stream",
        offset + 11,
        1,
    );
    add_tag(
        metadata,
        "SampleRate",
        TagValue::Unsigned(u64::from(u32::from_le_bytes(
            packet[12..16].try_into().expect("Vorbis sample rate"),
        ))),
        ValueType::UnsignedInteger,
        "Stream",
        offset + 12,
        4,
    );
    for (name, start) in [
        ("MaximumBitrate", 16_usize),
        ("NominalBitrate", 20_usize),
        ("MinimumBitrate", 24_usize),
    ] {
        add_tag(
            metadata,
            name,
            TagValue::Signed(i64::from(i32::from_le_bytes(
                packet[start..start + 4].try_into().expect("Vorbis bitrate"),
            ))),
            ValueType::SignedInteger,
            "Stream",
            offset + start as u64,
            4,
        );
    }
}

fn parse_opus_head(packet: &[u8], offset: u64, metadata: &mut Metadata) {
    if packet.len() < 19 {
        metadata.add_warning(
            Warning::new(
                "truncated-opus-head",
                "OpusHead packet is shorter than its fixed fields",
            )
            .at(offset),
        );
        return;
    }
    add_tag(
        metadata,
        "Codec",
        TagValue::String("Opus".to_owned()),
        ValueType::String,
        "Stream",
        offset,
        8,
    );
    add_tag(
        metadata,
        "Channels",
        TagValue::Unsigned(u64::from(packet[9])),
        ValueType::UnsignedInteger,
        "Stream",
        offset + 9,
        1,
    );
    add_tag(
        metadata,
        "PreSkip",
        TagValue::Unsigned(u64::from(u16::from_le_bytes(
            packet[10..12].try_into().expect("Opus pre-skip"),
        ))),
        ValueType::UnsignedInteger,
        "Stream",
        offset + 10,
        2,
    );
    add_tag(
        metadata,
        "SampleRate",
        TagValue::Unsigned(u64::from(u32::from_le_bytes(
            packet[12..16].try_into().expect("Opus sample rate"),
        ))),
        ValueType::UnsignedInteger,
        "Stream",
        offset + 12,
        4,
    );
    add_tag(
        metadata,
        "OutputGain",
        TagValue::Signed(i64::from(i16::from_le_bytes(
            packet[16..18].try_into().expect("Opus output gain"),
        ))),
        ValueType::SignedInteger,
        "Stream",
        offset + 16,
        2,
    );
}

fn parse_comments(
    packet: &[u8],
    prefix_length: usize,
    offset: u64,
    metadata: &mut Metadata,
    limits: ParseLimits,
    group: &str,
) {
    let Some(mut cursor) = prefix_length.checked_add(4) else {
        return;
    };
    let Some(vendor_length_bytes) = packet.get(prefix_length..cursor) else {
        metadata.add_warning(
            Warning::new("truncated-ogg-comment", "vendor length is truncated").at(offset),
        );
        return;
    };
    let vendor_length =
        u32::from_le_bytes(vendor_length_bytes.try_into().expect("Ogg vendor length")) as usize;
    let Some(vendor_end) = cursor.checked_add(vendor_length) else {
        metadata
            .add_warning(Warning::new("invalid-ogg-comment", "vendor length overflows").at(offset));
        return;
    };
    let Some(vendor_bytes) = packet.get(cursor..vendor_end) else {
        metadata
            .add_warning(Warning::new("truncated-ogg-comment", "vendor exceeds packet").at(offset));
        return;
    };
    add_tag(
        metadata,
        "Vendor",
        TagValue::String(String::from_utf8_lossy(vendor_bytes).into_owned()),
        ValueType::String,
        group,
        offset + prefix_length as u64 + 4,
        vendor_length as u64,
    );
    cursor = vendor_end;
    let Some(count_end) = cursor.checked_add(4) else {
        return;
    };
    let Some(count_bytes) = packet.get(cursor..count_end) else {
        metadata.add_warning(
            Warning::new("truncated-ogg-comment", "comment count is truncated").at(offset),
        );
        return;
    };
    let count = u32::from_le_bytes(count_bytes.try_into().expect("Ogg comment count"));
    cursor = count_end;
    let count = usize::try_from(count).unwrap_or(usize::MAX);
    for index in 0..count {
        if index >= limits.max_jpeg_segments {
            metadata.add_warning(
                Warning::new("ogg-comment-limit", "Ogg comment limit reached")
                    .at(offset + cursor as u64),
            );
            return;
        }
        let Some(length_end) = cursor.checked_add(4) else {
            metadata.add_warning(
                Warning::new("invalid-ogg-comment", "comment length overflows").at(offset),
            );
            return;
        };
        let Some(length_bytes) = packet.get(cursor..length_end) else {
            metadata.add_warning(
                Warning::new("truncated-ogg-comment", "comment length is truncated").at(offset),
            );
            return;
        };
        let length =
            u32::from_le_bytes(length_bytes.try_into().expect("Ogg comment length")) as usize;
        cursor = length_end;
        let Some(end) = cursor.checked_add(length) else {
            metadata.add_warning(
                Warning::new("invalid-ogg-comment", "comment length overflows").at(offset),
            );
            return;
        };
        let Some(raw) = packet.get(cursor..end) else {
            metadata.add_warning(
                Warning::new("truncated-ogg-comment", "comment exceeds packet").at(offset),
            );
            return;
        };
        if let Some(separator) = raw.iter().position(|byte| *byte == b'=') {
            let key = String::from_utf8_lossy(&raw[..separator]).to_ascii_uppercase();
            let name = vorbis_name(&key).unwrap_or_else(|| format!("Comment:{key}"));
            add_tag(
                metadata,
                &name,
                TagValue::String(String::from_utf8_lossy(&raw[separator + 1..]).into_owned()),
                ValueType::String,
                group,
                offset + cursor as u64,
                length as u64,
            );
        } else {
            metadata.add_warning(
                Warning::new("invalid-ogg-comment", "comment lacks a KEY=VALUE separator")
                    .at(offset + cursor as u64),
            );
        }
        cursor = end;
    }
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
        namespace: "Ogg".to_owned(),
        group: group.to_owned(),
        id: None,
        name: name.to_owned(),
        description: Some("Ogg container metadata".to_owned()),
        raw_value: None,
        value,
        value_type,
        source: Source::new("OGG", Some(offset), Some(length)),
        writable: false,
    });
}

fn read_at<R: Read + Seek>(
    reader: &mut R,
    offset: u64,
    length: usize,
    file_length: u64,
    path: &Path,
    context: &str,
) -> Result<Vec<u8>> {
    let end = offset
        .checked_add(
            u64::try_from(length).map_err(|_| MetraError::InvalidOffset {
                context: context.to_owned(),
                offset,
            })?,
        )
        .ok_or(MetraError::InvalidOffset {
            context: context.to_owned(),
            offset,
        })?;
    if end > file_length {
        return Err(MetraError::UnexpectedEof {
            context: context.to_owned(),
        });
    }
    reader
        .seek(SeekFrom::Start(offset))
        .map_err(|source| MetraError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    let mut bytes = vec![0_u8; length];
    reader
        .read_exact(&mut bytes)
        .map_err(|source| match source.kind() {
            std::io::ErrorKind::UnexpectedEof => MetraError::UnexpectedEof {
                context: context.to_owned(),
            },
            _ => MetraError::Io {
                path: path.to_path_buf(),
                source,
            },
        })?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    fn page(serial: u32, sequence: u32, header_type: u8, packet: &[u8]) -> Vec<u8> {
        assert!(packet.len() < 255);
        let mut page = Vec::from(&b"OggS"[..]);
        page.extend_from_slice(&[0, header_type]);
        page.extend_from_slice(&0_u64.to_le_bytes());
        page.extend_from_slice(&serial.to_le_bytes());
        page.extend_from_slice(&sequence.to_le_bytes());
        page.extend_from_slice(&0_u32.to_le_bytes());
        page.push(1);
        page.push(packet.len() as u8);
        page.extend_from_slice(packet);
        page
    }

    fn info(bytes: &[u8]) -> FileInfo {
        FileInfo::new(
            "audio.ogg".into(),
            bytes.len() as u64,
            metra_core::FileFormat::Ogg,
        )
    }

    #[test]
    fn reads_vorbis_identification_and_comments() {
        let mut identification = vec![1];
        identification.extend_from_slice(b"vorbis");
        identification.extend_from_slice(&0_u32.to_le_bytes());
        identification.push(2);
        identification.extend_from_slice(&44_100_u32.to_le_bytes());
        identification.extend_from_slice(&(-1_i32).to_le_bytes());
        identification.extend_from_slice(&128_000_i32.to_le_bytes());
        identification.extend_from_slice(&(-1_i32).to_le_bytes());
        identification.extend_from_slice(&[0x98, 0x88, 1, 1]);

        let vendor = b"Metra";
        let comment = b"TITLE=Ogg demo";
        let mut comments = vec![3];
        comments.extend_from_slice(b"vorbis");
        comments.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
        comments.extend_from_slice(vendor);
        comments.extend_from_slice(&1_u32.to_le_bytes());
        comments.extend_from_slice(&(comment.len() as u32).to_le_bytes());
        comments.extend_from_slice(comment);

        let mut bytes = page(7, 0, 0x02, &identification);
        bytes.extend_from_slice(&page(7, 1, 0, &comments));
        let metadata = read_ogg(
            &mut Cursor::new(bytes.clone()),
            info(&bytes),
            ParseLimits::default(),
        )
        .expect("Vorbis Ogg fixture should parse");
        assert_eq!(
            metadata.find("Ogg:Codec").unwrap().display_value(),
            "Vorbis"
        );
        assert_eq!(metadata.find("Ogg:Channels").unwrap().display_value(), "2");
        assert_eq!(
            metadata.find("Ogg:SampleRate").unwrap().display_value(),
            "44100"
        );
        assert_eq!(
            metadata.find("Ogg:Title").unwrap().display_value(),
            "Ogg demo"
        );
        assert_eq!(
            metadata.find("Ogg:Vendor").unwrap().display_value(),
            "Metra"
        );
        assert_eq!(metadata.find("Ogg:PageCount").unwrap().display_value(), "2");
    }

    #[test]
    fn reads_opus_head_and_tags() {
        let mut head = b"OpusHead".to_vec();
        head.extend_from_slice(&[1, 2]);
        head.extend_from_slice(&312_u16.to_le_bytes());
        head.extend_from_slice(&48_000_u32.to_le_bytes());
        head.extend_from_slice(&(-12_i16).to_le_bytes());
        head.push(0);
        let mut tags = b"OpusTags".to_vec();
        tags.extend_from_slice(&0_u32.to_le_bytes());
        tags.extend_from_slice(&1_u32.to_le_bytes());
        let comment = b"ARTIST=Metra";
        tags.extend_from_slice(&(comment.len() as u32).to_le_bytes());
        tags.extend_from_slice(comment);

        let mut bytes = page(8, 0, 0x02, &head);
        bytes.extend_from_slice(&page(8, 1, 0, &tags));
        let metadata = read_ogg(
            &mut Cursor::new(bytes.clone()),
            info(&bytes),
            ParseLimits::default(),
        )
        .expect("Opus Ogg fixture should parse");
        assert_eq!(metadata.find("Ogg:Codec").unwrap().display_value(), "Opus");
        assert_eq!(metadata.find("Ogg:PreSkip").unwrap().display_value(), "312");
        assert_eq!(
            metadata.find("Ogg:Artist").unwrap().display_value(),
            "Metra"
        );
    }

    #[test]
    fn rejects_non_ogg_input() {
        let bytes = b"not an ogg file".to_vec();
        let error = read_ogg(
            &mut Cursor::new(bytes.clone()),
            info(&bytes),
            ParseLimits::default(),
        )
        .expect_err("non-Ogg input should be rejected");
        assert!(matches!(error, MetraError::InvalidHeader { .. }));
    }
}
