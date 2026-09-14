use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use metra_core::{
    FileInfo, Metadata, MetraError, ParseLimits, Result, Source, Tag, TagValue, ValueType, Warning,
};

const OGG_PAGE_HEADER_LENGTH: usize = 27;
const MAX_PACKETS_PER_STREAM: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StreamCodec {
    Vorbis,
    Opus,
    OggFlac,
}

#[derive(Debug, Default)]
struct StreamState {
    pending: Vec<u8>,
    packet_offset: Option<u64>,
    packets_seen: usize,
    codec: Option<StreamCodec>,
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
        let lacing_end =
            lacing_offset
                .checked_add(segment_count as u64)
                .ok_or(MetraError::InvalidOffset {
                    context: "Ogg segment table".to_owned(),
                    offset: lacing_offset,
                })?;
        if lacing_end > file_length {
            metadata.add_warning(
                Warning::new("truncated-ogg-page", "Ogg segment table is truncated")
                    .at(lacing_offset),
            );
            break;
        }
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
                let declared_crc =
                    u32::from_le_bytes(header[22..26].try_into().expect("Ogg page checksum bytes"));
                let computed_crc = page_crc(&header, &lacing, &body);
                if declared_crc != computed_crc {
                    metadata.add_warning(
                        Warning::new(
                            "ogg-crc",
                            format!(
                                "Ogg page checksum {declared_crc:08X} does not match computed {computed_crc:08X}"
                            ),
                        )
                        .at(cursor + 22),
                    );
                }
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

fn page_crc(header: &[u8], lacing: &[u8], body: &[u8]) -> u32 {
    let mut normalized_header = [0_u8; OGG_PAGE_HEADER_LENGTH];
    normalized_header.copy_from_slice(header);
    normalized_header[22..26].fill(0);
    let mut crc_input = Vec::with_capacity(normalized_header.len() + lacing.len() + body.len());
    crc_input.extend_from_slice(&normalized_header);
    crc_input.extend_from_slice(lacing);
    crc_input.extend_from_slice(body);
    ogg_crc(&crc_input)
}

pub(crate) fn ogg_crc(bytes: &[u8]) -> u32 {
    let mut crc = 0_u32;
    for byte in bytes {
        crc ^= u32::from(*byte) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 {
                (crc << 1) ^ 0x04C1_1DB7
            } else {
                crc << 1
            };
        }
    }
    crc
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
                let detected_codec = if state.packets_seen == 1 {
                    parse_packet(&packet, packet_offset, metadata, limits)
                } else if state.codec == Some(StreamCodec::OggFlac) {
                    parse_ogg_flac_metadata_packet(&packet, packet_offset, metadata, limits);
                    None
                } else {
                    parse_packet(&packet, packet_offset, metadata, limits)
                };
                if state.codec.is_none() {
                    state.codec = detected_codec;
                }
            }
        }
        body_cursor = segment_end;
    }
}

fn parse_packet(
    packet: &[u8],
    offset: u64,
    metadata: &mut Metadata,
    limits: ParseLimits,
) -> Option<StreamCodec> {
    if packet.starts_with(b"OpusHead") {
        parse_opus_head(packet, offset, metadata);
        Some(StreamCodec::Opus)
    } else if packet.starts_with(b"OpusTags") {
        parse_comments(packet, 8, offset, metadata, limits, "OpusTags");
        None
    } else if packet.starts_with(&[1]) && packet.get(1..7) == Some(b"vorbis") {
        parse_vorbis_identification(packet, offset, metadata);
        Some(StreamCodec::Vorbis)
    } else if packet.starts_with(&[3]) && packet.get(1..7) == Some(b"vorbis") {
        parse_comments(packet, 7, offset, metadata, limits, "VORBIS_COMMENT");
        None
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
        parse_ogg_flac_mapping(packet, offset, metadata, limits);
        Some(StreamCodec::OggFlac)
    } else {
        None
    }
}

fn parse_ogg_flac_mapping(
    packet: &[u8],
    offset: u64,
    metadata: &mut Metadata,
    limits: ParseLimits,
) {
    if packet.len() < 13 || packet.get(9..13) != Some(b"fLaC") {
        metadata.add_warning(
            Warning::new(
                "invalid-ogg-flac-mapping",
                "Ogg-FLAC mapping header does not contain a native FLAC marker",
            )
            .at(offset),
        );
        return;
    }
    let mut cursor = 13_usize;
    let mut found_streaminfo = false;
    while let Some(header) = packet.get(cursor..cursor.saturating_add(4)) {
        let block_type = header[0] & 0x7F;
        let block_length =
            (usize::from(header[1]) << 16) | (usize::from(header[2]) << 8) | usize::from(header[3]);
        let data_start = cursor + 4;
        let Some(data_end) = data_start.checked_add(block_length) else {
            metadata.add_warning(
                Warning::new(
                    "invalid-ogg-flac-mapping",
                    "Ogg-FLAC block length overflows",
                )
                .at(offset + cursor as u64),
            );
            return;
        };
        let Some(data) = packet.get(data_start..data_end) else {
            metadata.add_warning(
                Warning::new(
                    "truncated-ogg-flac-mapping",
                    "Ogg-FLAC block exceeds its packet",
                )
                .at(offset + data_start as u64),
            );
            return;
        };
        if block_type == 0 {
            parse_ogg_flac_streaminfo(data, offset + data_start as u64, metadata);
            found_streaminfo = true;
        } else if block_type == 4 {
            parse_comments(
                data,
                0,
                offset + data_start as u64,
                metadata,
                limits,
                "FLAC_COMMENT",
            );
        }
        cursor = data_end;
        if header[0] & 0x80 != 0 {
            break;
        }
    }
    if !found_streaminfo {
        metadata.add_warning(
            Warning::new(
                "missing-ogg-flac-streaminfo",
                "Ogg-FLAC streaminfo block is missing",
            )
            .at(offset),
        );
    }
}

fn parse_ogg_flac_metadata_packet(
    packet: &[u8],
    offset: u64,
    metadata: &mut Metadata,
    limits: ParseLimits,
) {
    let mut cursor = 0_usize;
    let mut blocks = Vec::new();
    while let Some(header) = packet.get(cursor..cursor.saturating_add(4)) {
        let block_type = header[0] & 0x7F;
        if block_type > 6 {
            return;
        }
        let block_length =
            (usize::from(header[1]) << 16) | (usize::from(header[2]) << 8) | usize::from(header[3]);
        let data_start = cursor + 4;
        let Some(data_end) = data_start.checked_add(block_length) else {
            return;
        };
        let Some(_) = packet.get(data_start..data_end) else {
            return;
        };
        blocks.push((block_type, data_start, data_end));
        cursor = data_end;
        if header[0] & 0x80 != 0 {
            break;
        }
    }
    if blocks.is_empty() || cursor != packet.len() {
        return;
    }
    for (block_type, data_start, data_end) in blocks {
        if block_type == 4 {
            parse_comments(
                &packet[data_start..data_end],
                0,
                offset + data_start as u64,
                metadata,
                limits,
                "FLAC_COMMENT",
            );
        }
    }
}

fn parse_ogg_flac_streaminfo(data: &[u8], offset: u64, metadata: &mut Metadata) {
    if data.len() < 34 {
        metadata.add_warning(
            Warning::new(
                "truncated-ogg-flac-streaminfo",
                "Ogg-FLAC STREAMINFO block is shorter than 34 bytes",
            )
            .at(offset),
        );
        return;
    }
    add_tag(
        metadata,
        "BlockSizeMin",
        TagValue::Unsigned(u64::from(u16::from_be_bytes([data[0], data[1]]))),
        ValueType::UnsignedInteger,
        "StreamInfo",
        offset,
        2,
    );
    add_tag(
        metadata,
        "BlockSizeMax",
        TagValue::Unsigned(u64::from(u16::from_be_bytes([data[2], data[3]]))),
        ValueType::UnsignedInteger,
        "StreamInfo",
        offset + 2,
        2,
    );
    add_tag(
        metadata,
        "FrameSizeMin",
        TagValue::Unsigned(u64::from(read_be_u24(&data[4..7]))),
        ValueType::UnsignedInteger,
        "StreamInfo",
        offset + 4,
        3,
    );
    add_tag(
        metadata,
        "FrameSizeMax",
        TagValue::Unsigned(u64::from(read_be_u24(&data[7..10]))),
        ValueType::UnsignedInteger,
        "StreamInfo",
        offset + 7,
        3,
    );
    let packed = u64::from_be_bytes(data[10..18].try_into().expect("FLAC streaminfo fields"));
    let sample_rate = packed >> 44;
    let channels = ((packed >> 41) & 0x07) + 1;
    let bits_per_sample = ((packed >> 36) & 0x1F) + 1;
    let total_samples = packed & 0x0F_FFFF_FFFF;
    for (name, value, field_offset, length) in [
        ("SampleRate", sample_rate, 10_u64, 8_u64),
        ("Channels", channels, 10, 8),
        ("BitsPerSample", bits_per_sample, 10, 8),
        ("TotalSamples", total_samples, 10, 8),
    ] {
        add_tag(
            metadata,
            name,
            TagValue::Unsigned(value),
            ValueType::UnsignedInteger,
            "StreamInfo",
            offset + field_offset,
            length,
        );
    }
    if sample_rate > 0 && total_samples > 0 {
        add_tag(
            metadata,
            "DurationSeconds",
            TagValue::Float(total_samples as f64 / sample_rate as f64),
            ValueType::Float,
            "StreamInfo",
            offset + 10,
            8,
        );
    }
    add_tag(
        metadata,
        "MD5Signature",
        TagValue::Bytes(data[18..34].to_vec()),
        ValueType::Bytes,
        "StreamInfo",
        offset + 18,
        16,
    );
}

fn read_be_u24(bytes: &[u8]) -> u32 {
    (u32::from(bytes[0]) << 16) | (u32::from(bytes[1]) << 8) | u32::from(bytes[2])
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
        let checksum = page_crc(&page[..27], &page[27..28], packet);
        page[22..26].copy_from_slice(&checksum.to_le_bytes());
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
    fn reads_ogg_flac_streaminfo() {
        let sample_rate = 8_000_u64;
        let channels = 2_u64;
        let bits_per_sample = 8_u64;
        let total_samples = 100_u64;
        let packed = (sample_rate << 44)
            | ((channels - 1) << 41)
            | ((bits_per_sample - 1) << 36)
            | total_samples;
        let mut streaminfo = Vec::new();
        streaminfo.extend_from_slice(&4608_u16.to_be_bytes());
        streaminfo.extend_from_slice(&4608_u16.to_be_bytes());
        streaminfo.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
        streaminfo.extend_from_slice(&packed.to_be_bytes());
        streaminfo.extend_from_slice(&[0; 16]);

        let mut packet = vec![0x7F, b'F', b'L', b'A', b'C', 1, 0, 0, 2];
        packet.extend_from_slice(b"fLaC");
        packet.extend_from_slice(&[0, 0, 0, 34]);
        packet.extend_from_slice(&streaminfo);
        let comment = b"TITLE=Ogg FLAC demo";
        let mut comments = Vec::new();
        comments.extend_from_slice(&5_u32.to_le_bytes());
        comments.extend_from_slice(b"Metra");
        comments.extend_from_slice(&1_u32.to_le_bytes());
        comments.extend_from_slice(&(comment.len() as u32).to_le_bytes());
        comments.extend_from_slice(comment);
        let mut comment_packet = vec![0x84];
        comment_packet.extend_from_slice(&[
            ((comments.len() >> 16) & 0xFF) as u8,
            ((comments.len() >> 8) & 0xFF) as u8,
            (comments.len() & 0xFF) as u8,
        ]);
        comment_packet.extend_from_slice(&comments);

        let mut bytes = page(9, 0, 0x02, &packet);
        bytes.extend_from_slice(&page(9, 1, 0, &comment_packet));
        let metadata = read_ogg(
            &mut Cursor::new(bytes.clone()),
            info(&bytes),
            ParseLimits::default(),
        )
        .expect("Ogg-FLAC fixture should parse");
        assert_eq!(
            metadata.find("Ogg:SampleRate").unwrap().value,
            TagValue::Unsigned(8_000)
        );
        assert_eq!(
            metadata.find("Ogg:Channels").unwrap().value,
            TagValue::Unsigned(2)
        );
        assert_eq!(
            metadata.find("Ogg:BitsPerSample").unwrap().value,
            TagValue::Unsigned(8)
        );
        assert_eq!(
            metadata.find("Ogg:TotalSamples").unwrap().value,
            TagValue::Unsigned(100)
        );
        assert_eq!(
            metadata.find("Ogg:Title").unwrap().display_value(),
            "Ogg FLAC demo"
        );
        assert_eq!(
            metadata.find("Ogg:Vendor").unwrap().display_value(),
            "Metra"
        );
    }

    #[test]
    fn warns_on_invalid_metadata_page_crc_without_failing_read() {
        let mut packet = b"OpusHead".to_vec();
        packet.extend_from_slice(&[1, 1]);
        packet.extend_from_slice(&0_u16.to_le_bytes());
        packet.extend_from_slice(&48_000_u32.to_le_bytes());
        packet.extend_from_slice(&0_i16.to_le_bytes());
        packet.push(0);
        let mut bytes = page(10, 0, 0x02, &packet);
        bytes[22] ^= 0xFF;

        let metadata = read_ogg(
            &mut Cursor::new(bytes.clone()),
            info(&bytes),
            ParseLimits::default(),
        )
        .expect("CRC mismatch should remain a recoverable warning");
        assert!(
            metadata
                .warnings
                .iter()
                .any(|warning| warning.code == "ogg-crc")
        );
        assert_eq!(metadata.find("Ogg:Codec").unwrap().display_value(), "Opus");
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

    #[test]
    fn preserves_complete_pages_before_a_truncated_segment_table() {
        let mut packet = b"OpusHead".to_vec();
        packet.extend_from_slice(&[1, 1]);
        packet.extend_from_slice(&0_u16.to_le_bytes());
        packet.extend_from_slice(&48_000_u32.to_le_bytes());
        packet.extend_from_slice(&0_i16.to_le_bytes());
        packet.push(0);

        let mut bytes = page(8, 0, 0x02, &packet);
        let mut truncated = Vec::from(&b"OggS"[..]);
        truncated.extend_from_slice(&[0, 4]);
        truncated.extend_from_slice(&0_u64.to_le_bytes());
        truncated.extend_from_slice(&8_u32.to_le_bytes());
        truncated.extend_from_slice(&1_u32.to_le_bytes());
        truncated.extend_from_slice(&0_u32.to_le_bytes());
        truncated.push(2);
        bytes.extend_from_slice(&truncated);

        let metadata = read_ogg(
            &mut Cursor::new(bytes.clone()),
            info(&bytes),
            ParseLimits::default(),
        )
        .expect("complete Ogg pages should remain readable");
        assert!(
            metadata
                .warnings
                .iter()
                .any(|warning| warning.code == "truncated-ogg-page")
        );
        assert_eq!(metadata.find("Ogg:Codec").unwrap().display_value(), "Opus");
    }
}
