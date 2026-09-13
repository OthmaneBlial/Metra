use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use metra_core::{FileFormat, FileInfo, MetraError, ParseLimits, Result};

use crate::atomic::atomic_replace;
use crate::ogg::read_ogg;

/// Narrow, lossless Ogg comment edits for Vorbis Comments, OpusTags, and
/// Ogg-FLAC Vorbis Comment blocks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OggEdit {
    SetComment { key: String, value: String },
    DeleteComment { key: String },
}

pub fn rewrite_ogg<R: Read + Seek, W: Write>(
    reader: &mut R,
    writer: &mut W,
    file_info: FileInfo,
    limits: ParseLimits,
    edits: &[OggEdit],
) -> Result<()> {
    read_ogg(reader, file_info.clone(), limits)?;
    reader
        .seek(SeekFrom::Start(0))
        .map_err(|source| io_error(&file_info.path, source))?;
    rewrite_ogg_stream(
        reader,
        writer,
        &file_info.path,
        file_info.size,
        limits,
        edits,
    )
}

pub fn rewrite_ogg_to_vec(
    bytes: &[u8],
    file_info: FileInfo,
    limits: ParseLimits,
    edits: &[OggEdit],
) -> Result<Vec<u8>> {
    let mut reader = std::io::Cursor::new(bytes);
    let mut output = Vec::new();
    rewrite_ogg(&mut reader, &mut output, file_info.clone(), limits, edits)?;
    let output_size = output.len() as u64;
    read_ogg(
        &mut std::io::Cursor::new(output.as_slice()),
        FileInfo::new(file_info.path, output_size, FileFormat::Ogg),
        limits,
    )?;
    Ok(output)
}

pub fn rewrite_ogg_path(
    path: impl AsRef<Path>,
    limits: ParseLimits,
    edits: &[OggEdit],
) -> Result<()> {
    let path = path.as_ref().to_path_buf();
    let source_metadata = fs::metadata(&path).map_err(|source| MetraError::Io {
        path: path.clone(),
        source,
    })?;
    let file_info = FileInfo::new(path.clone(), source_metadata.len(), FileFormat::Ogg);
    let temp_path = temporary_path(&path)?;
    let result = (|| {
        let mut input = File::open(&path).map_err(|source| MetraError::Io {
            path: path.clone(),
            source,
        })?;
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
            .map_err(|source| MetraError::WriteFailure {
                message: format!("cannot create {}: {source}", temp_path.display()),
            })?;
        rewrite_ogg(&mut input, &mut output, file_info.clone(), limits, edits)?;
        output
            .sync_all()
            .map_err(|source| MetraError::WriteFailure {
                message: format!("cannot sync {}: {source}", temp_path.display()),
            })?;
        drop(output);

        let mut validation = File::open(&temp_path).map_err(|source| MetraError::Io {
            path: temp_path.clone(),
            source,
        })?;
        let written_size = validation
            .metadata()
            .map_err(|source| MetraError::Io {
                path: temp_path.clone(),
                source,
            })?
            .len();
        read_ogg(
            &mut validation,
            FileInfo::new(temp_path.clone(), written_size, FileFormat::Ogg),
            limits,
        )?;
        fs::set_permissions(&temp_path, source_metadata.permissions()).map_err(|source| {
            MetraError::WriteFailure {
                message: format!(
                    "cannot preserve permissions on {}: {source}",
                    temp_path.display()
                ),
            }
        })?;
        atomic_replace(&temp_path, &path).map_err(|source| MetraError::WriteFailure {
            message: format!("cannot atomically replace {}: {source}", path.display()),
        })?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result
}

#[derive(Debug)]
enum CommentAction {
    Set { key: Vec<u8>, value: Vec<u8> },
    Delete { key: Vec<u8> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Codec {
    Vorbis,
    Opus,
    OggFlac,
}

#[derive(Debug)]
struct OggPage {
    start: u64,
    end: u64,
    header: [u8; 27],
    lacing: Vec<u8>,
    body: Vec<u8>,
}

#[derive(Debug, Clone)]
struct PacketRange {
    page_start: u64,
    body_offset: usize,
    packet_offset: usize,
    length: usize,
}

#[derive(Debug)]
struct TargetPacket {
    codec: Codec,
    length: usize,
    bytes: Vec<u8>,
    ranges: Vec<PacketRange>,
}

#[derive(Debug, Default)]
struct StreamScan {
    pending: Vec<u8>,
    ranges: Vec<PacketRange>,
    packets_seen: usize,
    codec: Option<Codec>,
    exhausted: bool,
}

fn rewrite_ogg_stream<R: Read + Seek, W: Write>(
    reader: &mut R,
    writer: &mut W,
    path: &Path,
    file_length: u64,
    limits: ParseLimits,
    edits: &[OggEdit],
) -> Result<()> {
    let Some(action) = comment_action(edits, limits)? else {
        copy_exact(reader, writer, file_length, path)?;
        return Ok(());
    };
    let target = find_comment_packet(reader, file_length, path, limits)?.ok_or_else(|| {
        MetraError::WriteFailure {
            message: "Ogg file has no supported Vorbis Comments or OpusTags packet".to_owned(),
        }
    })?;
    reader
        .seek(SeekFrom::Start(0))
        .map_err(|source| io_error(path, source))?;
    let replacement = rewrite_comment_packet(&target.bytes, target.codec, &action, limits)?;
    if replacement.len() != target.length {
        return Err(MetraError::WriteFailure {
            message: "Ogg comment rewrite did not preserve packet size".to_owned(),
        });
    }

    let mut ranges_by_page = BTreeMap::<u64, Vec<PacketRange>>::new();
    for range in target.ranges {
        ranges_by_page
            .entry(range.page_start)
            .or_default()
            .push(range);
    }
    let mut cursor = 0_u64;
    while cursor < file_length {
        let page = match read_page(reader, cursor, file_length, path) {
            Ok(page) => page,
            Err(MetraError::UnexpectedEof { .. }) => {
                reader
                    .seek(SeekFrom::Start(cursor))
                    .map_err(|source| io_error(path, source))?;
                copy_exact(reader, writer, file_length - cursor, path)?;
                break;
            }
            Err(error) => return Err(error),
        };
        let Some(mut page) = page else {
            break;
        };
        if let Some(ranges) = ranges_by_page.get(&page.start) {
            for range in ranges {
                let replacement_end = range.packet_offset.checked_add(range.length).ok_or(
                    MetraError::InvalidOffset {
                        context: "Ogg replacement packet".to_owned(),
                        offset: range.packet_offset as u64,
                    },
                )?;
                let replacement_bytes = replacement
                    .get(range.packet_offset..replacement_end)
                    .ok_or(MetraError::InvalidOffset {
                        context: "Ogg replacement packet range".to_owned(),
                        offset: range.packet_offset as u64,
                    })?;
                let body_end = range.body_offset.checked_add(range.length).ok_or(
                    MetraError::InvalidOffset {
                        context: "Ogg replacement page body".to_owned(),
                        offset: range.body_offset as u64,
                    },
                )?;
                let body = page.body.get_mut(range.body_offset..body_end).ok_or(
                    MetraError::InvalidOffset {
                        context: "Ogg replacement page body range".to_owned(),
                        offset: range.body_offset as u64,
                    },
                )?;
                body.copy_from_slice(replacement_bytes);
            }
            write_page(writer, &page)?;
        } else {
            write_all(writer, &page.header)?;
            write_all(writer, &page.lacing)?;
            write_all(writer, &page.body)?;
        }
        cursor = page.end;
    }
    Ok(())
}

fn find_comment_packet<R: Read + Seek>(
    reader: &mut R,
    file_length: u64,
    path: &Path,
    limits: ParseLimits,
) -> Result<Option<TargetPacket>> {
    let mut cursor = 0_u64;
    let mut streams = BTreeMap::<u32, StreamScan>::new();
    while cursor < file_length {
        let Some(page) = read_page(reader, cursor, file_length, path)? else {
            break;
        };
        let serial = u32::from_le_bytes(page.header[14..18].try_into().expect("Ogg serial"));
        let state = if streams.contains_key(&serial) || streams.len() < limits.max_jpeg_segments {
            streams.entry(serial).or_default()
        } else {
            cursor = page.end;
            continue;
        };
        if page.header[5] & 0x01 == 0 && !state.pending.is_empty() {
            return Err(MetraError::CorruptMetadata {
                message: "Ogg page breaks a packet without the continuation flag".to_owned(),
            });
        }
        let mut body_cursor = 0_usize;
        for segment_length in page.lacing.iter().map(|length| usize::from(*length)) {
            let segment_end =
                body_cursor
                    .checked_add(segment_length)
                    .ok_or(MetraError::InvalidOffset {
                        context: "Ogg packet segment".to_owned(),
                        offset: body_cursor as u64,
                    })?;
            let segment =
                page.body
                    .get(body_cursor..segment_end)
                    .ok_or(MetraError::UnexpectedEof {
                        context: "Ogg packet segment".to_owned(),
                    })?;
            if !state.exhausted {
                let packet_offset = state.pending.len();
                let packet_length = packet_offset.checked_add(segment.len()).ok_or(
                    MetraError::ResourceLimitExceeded {
                        resource: "Ogg packet".to_owned(),
                        limit: limits.max_value_bytes,
                    },
                )?;
                if packet_length > limits.max_value_bytes {
                    return Err(MetraError::ResourceLimitExceeded {
                        resource: "Ogg packet".to_owned(),
                        limit: limits.max_value_bytes,
                    });
                }
                state.ranges.push(PacketRange {
                    page_start: page.start,
                    body_offset: body_cursor,
                    packet_offset,
                    length: segment.len(),
                });
                state.pending.extend_from_slice(segment);
                if segment_length < 255 {
                    let packet = std::mem::take(&mut state.pending);
                    let ranges = std::mem::take(&mut state.ranges);
                    state.packets_seen += 1;
                    if state.packets_seen == 1 {
                        state.codec = codec_from_identification(&packet);
                    }
                    if let Some(codec) = state.codec {
                        let comment_packet = match codec {
                            Codec::OggFlac => true,
                            Codec::Vorbis | Codec::Opus => state.packets_seen == 2,
                        };
                        if comment_packet && let Some((start, end)) = comment_range(codec, &packet)
                        {
                            return Ok(Some(target_packet(codec, &packet, &ranges, start, end)));
                        }
                    }
                    if state.packets_seen >= 8 {
                        state.exhausted = true;
                    }
                }
            }
            body_cursor = segment_end;
        }
        cursor = page.end;
    }
    Ok(None)
}

fn codec_from_identification(packet: &[u8]) -> Option<Codec> {
    if packet.starts_with(&[1]) && packet.get(1..7) == Some(b"vorbis") {
        Some(Codec::Vorbis)
    } else if packet.starts_with(b"OpusHead") {
        Some(Codec::Opus)
    } else if packet.starts_with(&[0x7F]) && packet.get(1..5) == Some(b"FLAC") {
        Some(Codec::OggFlac)
    } else {
        None
    }
}

fn comment_prefix(packet: &[u8]) -> Option<(Codec, usize)> {
    if packet.starts_with(&[3]) && packet.get(1..7) == Some(b"vorbis") {
        Some((Codec::Vorbis, 7))
    } else if packet.starts_with(b"OpusTags") {
        Some((Codec::Opus, 8))
    } else if packet.len() >= 4 && packet[0] & 0x7F == 4 {
        let block_length =
            (usize::from(packet[1]) << 16) | (usize::from(packet[2]) << 8) | usize::from(packet[3]);
        (block_length == packet.len().saturating_sub(4)).then_some((Codec::OggFlac, 4))
    } else {
        None
    }
}

fn comment_range(codec: Codec, packet: &[u8]) -> Option<(usize, usize)> {
    match codec {
        Codec::Vorbis | Codec::Opus => comment_prefix(packet)
            .filter(|(packet_codec, _)| *packet_codec == codec)
            .map(|_| (0, packet.len())),
        Codec::OggFlac => ogg_flac_comment_range(packet),
    }
}

fn ogg_flac_comment_range(packet: &[u8]) -> Option<(usize, usize)> {
    let mut cursor: usize = if packet.starts_with(&[0x7F]) && packet.get(1..5) == Some(b"FLAC") {
        if packet.get(9..13) != Some(b"fLaC") {
            return None;
        }
        13
    } else {
        0
    };
    while let Some(header) = packet.get(cursor..cursor.saturating_add(4)) {
        let block_type = header[0] & 0x7F;
        if block_type > 6 {
            return None;
        }
        let block_length =
            (usize::from(header[1]) << 16) | (usize::from(header[2]) << 8) | usize::from(header[3]);
        let data_start = cursor.checked_add(4)?;
        let data_end = data_start.checked_add(block_length)?;
        packet.get(data_start..data_end)?;
        if block_type == 4 {
            return Some((cursor, data_end));
        }
        cursor = data_end;
        if header[0] & 0x80 != 0 {
            break;
        }
    }
    None
}

fn target_packet(
    codec: Codec,
    packet: &[u8],
    ranges: &[PacketRange],
    start: usize,
    end: usize,
) -> TargetPacket {
    let adjusted_ranges = ranges
        .iter()
        .filter_map(|range| {
            let range_end = range.packet_offset.checked_add(range.length)?;
            let overlap_start = range.packet_offset.max(start);
            let overlap_end = range_end.min(end);
            (overlap_start < overlap_end).then(|| PacketRange {
                page_start: range.page_start,
                body_offset: range.body_offset + (overlap_start - range.packet_offset),
                packet_offset: overlap_start - start,
                length: overlap_end - overlap_start,
            })
        })
        .collect();
    TargetPacket {
        codec,
        length: end - start,
        bytes: packet[start..end].to_vec(),
        ranges: adjusted_ranges,
    }
}

fn comment_action(edits: &[OggEdit], limits: ParseLimits) -> Result<Option<CommentAction>> {
    let mut action = None;
    for edit in edits {
        match edit {
            OggEdit::SetComment { key, value } => {
                let key = validate_key(key)?;
                if value.contains('\0') {
                    return Err(MetraError::WriteFailure {
                        message: "Ogg comment values cannot contain NUL".to_owned(),
                    });
                }
                if value.len() > limits.max_value_bytes {
                    return Err(MetraError::ResourceLimitExceeded {
                        resource: "Ogg comment value".to_owned(),
                        limit: limits.max_value_bytes,
                    });
                }
                action = Some(CommentAction::Set {
                    key,
                    value: value.as_bytes().to_vec(),
                });
            }
            OggEdit::DeleteComment { key } => {
                action = Some(CommentAction::Delete {
                    key: validate_key(key)?,
                });
            }
        }
    }
    Ok(action)
}

fn validate_key(key: &str) -> Result<Vec<u8>> {
    if key.is_empty()
        || !key.is_ascii()
        || key.contains('=')
        || key.contains('\0')
        || key.bytes().any(|byte| byte < 0x20 || byte == 0x7F)
    {
        return Err(MetraError::WriteFailure {
            message: "Ogg comment keys must be printable ASCII without '='".to_owned(),
        });
    }
    Ok(key.to_ascii_uppercase().into_bytes())
}

fn rewrite_comment_packet(
    packet: &[u8],
    codec: Codec,
    action: &CommentAction,
    limits: ParseLimits,
) -> Result<Vec<u8>> {
    let prefix_length = match codec {
        Codec::Vorbis => 7,
        Codec::Opus => 8,
        Codec::OggFlac => 4,
    };
    if packet.get(..prefix_length).is_none() || comment_prefix(packet).is_none() {
        return Err(MetraError::InvalidTag {
            context: "Ogg comments".to_owned(),
            message: "comment packet signature is invalid".to_owned(),
        });
    }
    let mut cursor = prefix_length
        .checked_add(4)
        .ok_or(MetraError::InvalidOffset {
            context: "Ogg vendor length".to_owned(),
            offset: prefix_length as u64,
        })?;
    let vendor_length = read_u32(packet, prefix_length)? as usize;
    let vendor_end = cursor
        .checked_add(vendor_length)
        .ok_or(MetraError::InvalidOffset {
            context: "Ogg vendor string".to_owned(),
            offset: cursor as u64,
        })?;
    let vendor = packet
        .get(cursor..vendor_end)
        .ok_or(MetraError::UnexpectedEof {
            context: "Ogg vendor string".to_owned(),
        })?;
    if vendor_length > limits.max_value_bytes {
        return Err(MetraError::ResourceLimitExceeded {
            resource: "Ogg vendor string".to_owned(),
            limit: limits.max_value_bytes,
        });
    }
    cursor = vendor_end;
    let count = read_u32(packet, cursor)? as usize;
    cursor = cursor.checked_add(4).ok_or(MetraError::InvalidOffset {
        context: "Ogg comment count".to_owned(),
        offset: cursor as u64,
    })?;
    if count > limits.max_jpeg_segments {
        return Err(MetraError::ResourceLimitExceeded {
            resource: "Ogg comments".to_owned(),
            limit: limits.max_jpeg_segments,
        });
    }
    let mut comments = Vec::with_capacity(count.saturating_add(1));
    let mut found = false;
    for _ in 0..count {
        let length = read_u32(packet, cursor)? as usize;
        cursor = cursor.checked_add(4).ok_or(MetraError::InvalidOffset {
            context: "Ogg comment length".to_owned(),
            offset: cursor as u64,
        })?;
        let end = cursor
            .checked_add(length)
            .ok_or(MetraError::InvalidOffset {
                context: "Ogg comment value".to_owned(),
                offset: cursor as u64,
            })?;
        let raw = packet.get(cursor..end).ok_or(MetraError::UnexpectedEof {
            context: "Ogg comment value".to_owned(),
        })?;
        let matches = raw
            .iter()
            .position(|byte| *byte == b'=')
            .is_some_and(|separator| raw[..separator].eq_ignore_ascii_case(action_key(action)));
        if matches {
            found = true;
            if let CommentAction::Set { key, value } = action
                && !comments.iter().any(|comment: &Vec<u8>| {
                    comment
                        .iter()
                        .position(|byte| *byte == b'=')
                        .is_some_and(|separator| comment[..separator].eq_ignore_ascii_case(key))
                })
            {
                comments.push(make_comment(key, value));
            }
        } else {
            comments.push(raw.to_vec());
        }
        cursor = end;
    }
    let trailing = packet.get(cursor..).unwrap_or_default();
    if let CommentAction::Set { key, value } = action
        && !found
    {
        comments.push(make_comment(key, value));
        found = true;
    }
    if matches!(action, CommentAction::Delete { .. }) && !found {
        return Ok(packet.to_vec());
    }

    let mut output = encode_comment_packet(&packet[..prefix_length], vendor, &comments)?;
    output.extend_from_slice(trailing);
    if output.len() > packet.len() {
        return Err(MetraError::WriteFailure {
            message: "Ogg comment rewrite needs a larger packet; lossless page layout is preserved"
                .to_owned(),
        });
    }
    if output.len() < packet.len() {
        let difference = packet.len() - output.len();
        if difference < 12 {
            return Err(MetraError::WriteFailure {
                message: "Ogg comment rewrite cannot fit its lossless padding".to_owned(),
            });
        }
        let padding = vec![b' '; difference - 12];
        comments.push(make_comment(b"PADDING", &padding));
        output = encode_comment_packet(&packet[..prefix_length], vendor, &comments)?;
        output.extend_from_slice(trailing);
    }
    if output.len() != packet.len() {
        return Err(MetraError::WriteFailure {
            message: "Ogg comment rewrite could not preserve packet size".to_owned(),
        });
    }
    Ok(output)
}

fn action_key(action: &CommentAction) -> &[u8] {
    match action {
        CommentAction::Set { key, .. } | CommentAction::Delete { key } => key,
    }
}

fn make_comment(key: &[u8], value: &[u8]) -> Vec<u8> {
    let mut comment = Vec::with_capacity(key.len() + value.len() + 1);
    comment.extend_from_slice(key);
    comment.push(b'=');
    comment.extend_from_slice(value);
    comment
}

fn encode_comment_packet(prefix: &[u8], vendor: &[u8], comments: &[Vec<u8>]) -> Result<Vec<u8>> {
    let vendor_length = u32::try_from(vendor.len()).map_err(|_| MetraError::WriteFailure {
        message: "Ogg vendor string exceeds the 32-bit size limit".to_owned(),
    })?;
    let count = u32::try_from(comments.len()).map_err(|_| MetraError::WriteFailure {
        message: "Ogg comment count exceeds the 32-bit size limit".to_owned(),
    })?;
    let mut output = Vec::new();
    output.extend_from_slice(prefix);
    output.extend_from_slice(&vendor_length.to_le_bytes());
    output.extend_from_slice(vendor);
    output.extend_from_slice(&count.to_le_bytes());
    for comment in comments {
        let length = u32::try_from(comment.len()).map_err(|_| MetraError::WriteFailure {
            message: "Ogg comment exceeds the 32-bit size limit".to_owned(),
        })?;
        output.extend_from_slice(&length.to_le_bytes());
        output.extend_from_slice(comment);
    }
    Ok(output)
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32> {
    let bytes = bytes
        .get(offset..offset.saturating_add(4))
        .ok_or(MetraError::UnexpectedEof {
            context: "Ogg little-endian integer".to_owned(),
        })?;
    Ok(u32::from_le_bytes(bytes.try_into().expect("Ogg u32")))
}

fn read_page<R: Read + Seek>(
    reader: &mut R,
    start: u64,
    file_length: u64,
    path: &Path,
) -> Result<Option<OggPage>> {
    if start >= file_length {
        return Ok(None);
    }
    let remaining = file_length.saturating_sub(start);
    if remaining < 27 {
        return Err(MetraError::UnexpectedEof {
            context: "Ogg page header".to_owned(),
        });
    }
    reader
        .seek(SeekFrom::Start(start))
        .map_err(|source| io_error(path, source))?;
    let mut header = [0_u8; 27];
    read_exact(reader, &mut header, path, "Ogg page header")?;
    if &header[..4] != b"OggS" {
        return Err(MetraError::InvalidHeader {
            context: "OGG".to_owned(),
            message: "expected OggS page signature".to_owned(),
        });
    }
    let segment_count = usize::from(header[26]);
    let mut lacing = vec![0_u8; segment_count];
    read_exact(reader, &mut lacing, path, "Ogg segment table")?;
    let body_length = lacing
        .iter()
        .try_fold(0_usize, |total, length| {
            total.checked_add(usize::from(*length))
        })
        .ok_or(MetraError::InvalidOffset {
            context: "Ogg page body".to_owned(),
            offset: start,
        })?;
    let header_and_table =
        27_u64
            .checked_add(segment_count as u64)
            .ok_or(MetraError::InvalidOffset {
                context: "Ogg page body".to_owned(),
                offset: start,
            })?;
    let body_start = start
        .checked_add(header_and_table)
        .ok_or(MetraError::InvalidOffset {
            context: "Ogg page body".to_owned(),
            offset: start,
        })?;
    let end = body_start
        .checked_add(body_length as u64)
        .ok_or(MetraError::InvalidOffset {
            context: "Ogg page end".to_owned(),
            offset: body_start,
        })?;
    if end > file_length {
        return Err(MetraError::UnexpectedEof {
            context: "Ogg page body".to_owned(),
        });
    }
    let mut body = vec![0_u8; body_length];
    read_exact(reader, &mut body, path, "Ogg page body")?;
    Ok(Some(OggPage {
        start,
        end,
        header,
        lacing,
        body,
    }))
}

fn write_page<W: Write>(writer: &mut W, page: &OggPage) -> Result<()> {
    let mut header = page.header;
    header[22..26].fill(0);
    let mut encoded = Vec::with_capacity(27 + page.lacing.len() + page.body.len());
    encoded.extend_from_slice(&header);
    encoded.extend_from_slice(&page.lacing);
    encoded.extend_from_slice(&page.body);
    let checksum = ogg_crc(&encoded);
    encoded[22..26].copy_from_slice(&checksum.to_le_bytes());
    write_all(writer, &encoded)
}

fn ogg_crc(bytes: &[u8]) -> u32 {
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

fn copy_exact<R: Read, W: Write>(
    reader: &mut R,
    writer: &mut W,
    length: u64,
    path: &Path,
) -> Result<()> {
    let mut remaining = length;
    let mut buffer = [0_u8; 64 * 1024];
    while remaining > 0 {
        let amount = remaining.min(buffer.len() as u64) as usize;
        read_exact(reader, &mut buffer[..amount], path, "Ogg copy")?;
        write_all(writer, &buffer[..amount])?;
        remaining -= amount as u64;
    }
    Ok(())
}

fn read_exact<R: Read>(reader: &mut R, bytes: &mut [u8], path: &Path, context: &str) -> Result<()> {
    reader
        .read_exact(bytes)
        .map_err(|source| match source.kind() {
            std::io::ErrorKind::UnexpectedEof => MetraError::UnexpectedEof {
                context: context.to_owned(),
            },
            _ => io_error(path, source),
        })
}

fn write_all<W: Write>(writer: &mut W, bytes: &[u8]) -> Result<()> {
    writer
        .write_all(bytes)
        .map_err(|source| MetraError::WriteFailure {
            message: format!("cannot write Ogg output: {source}"),
        })
}

fn io_error(path: &Path, source: std::io::Error) -> MetraError {
    MetraError::Io {
        path: path.to_path_buf(),
        source,
    }
}

fn temporary_path(path: &Path) -> Result<PathBuf> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| MetraError::WriteFailure {
            message: "Ogg rewrite requires a valid UTF-8 filename".to_owned(),
        })?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| MetraError::WriteFailure {
            message: format!("cannot create Ogg temporary name: {error}"),
        })?
        .as_nanos();
    Ok(parent.join(format!(".{name}.metra-ogg-{nonce}.tmp")))
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;
    use crate::ogg::read_ogg;

    fn page(serial: u32, sequence: u32, header_type: u8, packet: &[u8]) -> Vec<u8> {
        assert!(packet.len() < 255);
        let mut output = Vec::from(&b"OggS"[..]);
        output.extend_from_slice(&[0, header_type]);
        output.extend_from_slice(&0_u64.to_le_bytes());
        output.extend_from_slice(&serial.to_le_bytes());
        output.extend_from_slice(&sequence.to_le_bytes());
        output.extend_from_slice(&[0; 4]);
        output.push(1);
        output.push(packet.len() as u8);
        output.extend_from_slice(packet);
        let checksum = ogg_crc(&output);
        output[22..26].copy_from_slice(&checksum.to_le_bytes());
        output
    }

    fn vorbis_fixture() -> Vec<u8> {
        let mut identification = vec![1];
        identification.extend_from_slice(b"vorbis");
        identification.extend_from_slice(&0_u32.to_le_bytes());
        identification.push(2);
        identification.extend_from_slice(&44_100_u32.to_le_bytes());
        identification.extend_from_slice(&[0; 12]);
        identification.extend_from_slice(&[0x98, 0x88, 1, 1]);

        let vendor = b"Metra";
        let comment = b"TITLE=before";
        let mut comments = vec![3];
        comments.extend_from_slice(b"vorbis");
        comments.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
        comments.extend_from_slice(vendor);
        comments.extend_from_slice(&1_u32.to_le_bytes());
        comments.extend_from_slice(&(comment.len() as u32).to_le_bytes());
        comments.extend_from_slice(comment);

        let mut output = page(12, 0, 0x02, &identification);
        output.extend_from_slice(&page(12, 1, 0, &comments));
        output
    }

    fn ogg_flac_fixture() -> Vec<u8> {
        let mut mapping = vec![0x7F, b'F', b'L', b'A', b'C', 1, 0, 0, 2];
        mapping.extend_from_slice(b"fLaC");
        mapping.extend_from_slice(&[0, 0, 0, 34]);
        mapping.extend_from_slice(&[0; 34]);

        let vendor = b"Metra";
        let comment = b"TITLE=before";
        let mut comments = Vec::new();
        comments.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
        comments.extend_from_slice(vendor);
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

        let mut output = page(13, 0, 0x02, &mapping);
        output.extend_from_slice(&page(13, 1, 0, &comment_packet));
        output
    }

    fn ogg_flac_mapping_comment_fixture() -> Vec<u8> {
        let mut mapping = vec![0x7F, b'F', b'L', b'A', b'C', 1, 0, 0, 2];
        mapping.extend_from_slice(b"fLaC");
        mapping.extend_from_slice(&[0, 0, 0, 34]);
        mapping.extend_from_slice(&[0; 34]);

        let vendor = b"Metra";
        let comment = b"TITLE=before";
        let mut comments = Vec::new();
        comments.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
        comments.extend_from_slice(vendor);
        comments.extend_from_slice(&1_u32.to_le_bytes());
        comments.extend_from_slice(&(comment.len() as u32).to_le_bytes());
        comments.extend_from_slice(comment);
        mapping.push(0x84);
        mapping.extend_from_slice(&[
            ((comments.len() >> 16) & 0xFF) as u8,
            ((comments.len() >> 8) & 0xFF) as u8,
            (comments.len() & 0xFF) as u8,
        ]);
        mapping.extend_from_slice(&comments);
        page(14, 0, 0x02, &mapping)
    }

    fn info(bytes: &[u8]) -> FileInfo {
        FileInfo::new("audio.ogg".into(), bytes.len() as u64, FileFormat::Ogg)
    }

    #[test]
    fn replaces_vorbis_comment_and_recomputes_page_crc() {
        let bytes = vorbis_fixture();
        let rewritten = rewrite_ogg_to_vec(
            &bytes,
            info(&bytes),
            ParseLimits::default(),
            &[OggEdit::SetComment {
                key: "Title".to_owned(),
                value: "edited".to_owned(),
            }],
        )
        .expect("same-size Ogg comment should rewrite");
        let metadata = read_ogg(
            &mut Cursor::new(rewritten.clone()),
            info(&rewritten),
            ParseLimits::default(),
        )
        .expect("rewritten Ogg should parse");
        assert_eq!(
            metadata.find("Ogg:Title").unwrap().display_value(),
            "edited"
        );
        let second_start = rewritten
            .windows(4)
            .enumerate()
            .skip(1)
            .find_map(|(offset, window)| (window == b"OggS").then_some(offset))
            .expect("fixture should contain a second Ogg page");
        let second_page = &rewritten[second_start..];
        assert!(second_page.len() >= 27);
        let segment_count = usize::from(second_page[26]);
        let body_length = second_page[27..27 + segment_count]
            .iter()
            .map(|length| usize::from(*length))
            .sum::<usize>();
        let page_end = second_page.len().min(27 + segment_count + body_length);
        let mut page_bytes = second_page[..page_end].to_vec();
        let actual = page_bytes[22..26].to_vec();
        page_bytes[22..26].fill(0);
        assert_eq!(actual, ogg_crc(&page_bytes).to_le_bytes());
    }

    #[test]
    fn deletes_vorbis_comment_with_lossless_padding() {
        let bytes = vorbis_fixture();
        let rewritten = rewrite_ogg_to_vec(
            &bytes,
            info(&bytes),
            ParseLimits::default(),
            &[OggEdit::DeleteComment {
                key: "Title".to_owned(),
            }],
        )
        .expect("deleting a comment should use packet padding");
        let metadata = read_ogg(
            &mut Cursor::new(rewritten.clone()),
            info(&rewritten),
            ParseLimits::default(),
        )
        .expect("rewritten Ogg should parse");
        assert!(metadata.find("Ogg:Title").is_none());
        assert_eq!(
            metadata
                .find("Ogg:Comment:PADDING")
                .unwrap()
                .display_value()
                .len(),
            4
        );
    }

    #[test]
    fn rejects_comment_growth_that_cannot_fit_the_existing_packet() {
        let bytes = vorbis_fixture();
        let error = rewrite_ogg_to_vec(
            &bytes,
            info(&bytes),
            ParseLimits::default(),
            &[OggEdit::SetComment {
                key: "Title".to_owned(),
                value: "a much longer replacement".to_owned(),
            }],
        )
        .expect_err("a larger packet cannot be rewritten losslessly");
        assert!(error.to_string().contains("larger packet"));
    }

    #[test]
    fn replaces_ogg_flac_comment_block_without_changing_packet_size() {
        let bytes = ogg_flac_fixture();
        let rewritten = rewrite_ogg_to_vec(
            &bytes,
            info(&bytes),
            ParseLimits::default(),
            &[OggEdit::SetComment {
                key: "Title".to_owned(),
                value: "edited".to_owned(),
            }],
        )
        .expect("Ogg-FLAC comment should rewrite losslessly");
        let metadata = read_ogg(
            &mut Cursor::new(rewritten.clone()),
            info(&rewritten),
            ParseLimits::default(),
        )
        .expect("rewritten Ogg-FLAC should parse");
        assert_eq!(
            metadata.find("Ogg:Title").unwrap().display_value(),
            "edited"
        );
        assert_eq!(rewritten.len(), bytes.len());
    }

    #[test]
    fn replaces_ogg_flac_comment_embedded_in_mapping_packet() {
        let bytes = ogg_flac_mapping_comment_fixture();
        let rewritten = rewrite_ogg_to_vec(
            &bytes,
            info(&bytes),
            ParseLimits::default(),
            &[OggEdit::SetComment {
                key: "Title".to_owned(),
                value: "edited".to_owned(),
            }],
        )
        .expect("embedded Ogg-FLAC comment should rewrite losslessly");
        let metadata = read_ogg(
            &mut Cursor::new(rewritten.clone()),
            info(&rewritten),
            ParseLimits::default(),
        )
        .expect("rewritten mapping packet should parse");
        assert_eq!(
            metadata.find("Ogg:Title").unwrap().display_value(),
            "edited"
        );
        assert_eq!(rewritten.len(), bytes.len());
    }
}
