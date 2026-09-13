use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use metra_core::{FileFormat, FileInfo, MetraError, ParseLimits, Result};

use crate::atomic::atomic_replace;
use crate::id3::read_mp3;

/// Narrow, lossless edits for common ID3v2 text and comment frames.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mp3Edit {
    SetText { name: String, value: String },
    DeleteText { name: String },
    SetComment(String),
    DeleteComments,
}

pub fn rewrite_mp3<R: Read + Seek, W: Write>(
    reader: &mut R,
    writer: &mut W,
    file_info: FileInfo,
    limits: ParseLimits,
    edits: &[Mp3Edit],
) -> Result<()> {
    read_mp3(reader, file_info.clone(), limits)?;
    reader
        .seek(SeekFrom::Start(0))
        .map_err(|source| io_error(&file_info.path, source))?;
    rewrite_mp3_stream(reader, writer, &file_info.path, limits, edits)
}

pub fn rewrite_mp3_to_vec(
    bytes: &[u8],
    file_info: FileInfo,
    limits: ParseLimits,
    edits: &[Mp3Edit],
) -> Result<Vec<u8>> {
    let mut reader = std::io::Cursor::new(bytes);
    let mut output = Vec::new();
    rewrite_mp3(&mut reader, &mut output, file_info.clone(), limits, edits)?;
    let output_size = output.len() as u64;
    read_mp3(
        &mut std::io::Cursor::new(output.as_slice()),
        FileInfo::new(file_info.path, output_size, FileFormat::Mp3),
        limits,
    )?;
    Ok(output)
}

pub fn rewrite_mp3_path(
    path: impl AsRef<Path>,
    limits: ParseLimits,
    edits: &[Mp3Edit],
) -> Result<()> {
    let path = path.as_ref().to_path_buf();
    let source_metadata = fs::metadata(&path).map_err(|source| MetraError::Io {
        path: path.clone(),
        source,
    })?;
    let file_info = FileInfo::new(path.clone(), source_metadata.len(), FileFormat::Mp3);
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
        rewrite_mp3(&mut input, &mut output, file_info.clone(), limits, edits)?;
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
        read_mp3(
            &mut validation,
            FileInfo::new(temp_path.clone(), written_size, FileFormat::Mp3),
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
struct Frame {
    id: Vec<u8>,
    flags: [u8; 2],
    payload: Vec<u8>,
}

#[derive(Debug)]
enum ResolvedEdit {
    Set {
        name: String,
        frame_id: Vec<u8>,
        payload: Vec<u8>,
    },
    Delete {
        name: String,
        frame_id: Vec<u8>,
    },
}

fn rewrite_mp3_stream<R: Read, W: Write>(
    reader: &mut R,
    writer: &mut W,
    path: &Path,
    limits: ParseLimits,
    edits: &[Mp3Edit],
) -> Result<()> {
    let mut header = [0_u8; 10];
    read_exact(reader, &mut header, path)?;
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
    if flags & 0x80 != 0 {
        return Err(MetraError::WriteFailure {
            message: "ID3 unsynchronization is not writable yet".to_owned(),
        });
    }
    if flags & 0x40 != 0 {
        return Err(MetraError::WriteFailure {
            message: "ID3 extended headers are not writable yet".to_owned(),
        });
    }
    if version == 4 && flags & 0x10 != 0 {
        return Err(MetraError::WriteFailure {
            message: "ID3v2.4 footers are not writable yet".to_owned(),
        });
    }

    let tag_size =
        usize::try_from(parse_synchsafe(&header[6..10], "ID3v2 tag size")?).map_err(|_| {
            MetraError::ResourceLimitExceeded {
                resource: "ID3v2 tag".to_owned(),
                limit: limits.max_metadata_bytes,
            }
        })?;
    if tag_size > limits.max_metadata_bytes {
        return Err(MetraError::ResourceLimitExceeded {
            resource: "ID3v2 tag".to_owned(),
            limit: limits.max_metadata_bytes,
        });
    }
    let mut body = vec![0_u8; tag_size];
    read_exact(reader, &mut body, path)?;

    let actions = resolve_edits(version, limits, edits)?;
    let (frames, padding) = parse_frames_for_rewrite(&body, version, limits)?;
    let mut rewritten_body = Vec::with_capacity(body.len());
    let mut found = vec![false; actions.len()];
    for frame in frames {
        let action = actions
            .iter()
            .enumerate()
            .find(|(_, action)| action_frame_id(action) == frame.id.as_slice());
        let Some((index, action)) = action else {
            append_frame(
                &mut rewritten_body,
                version,
                &frame.id,
                frame.flags,
                &frame.payload,
            )?;
            continue;
        };
        found[index] = true;
        match action {
            ResolvedEdit::Set { payload, .. } => {
                if frame.flags != [0, 0] {
                    return Err(MetraError::WriteFailure {
                        message: format!(
                            "ID3 frame {} has flags that prevent a safe rewrite",
                            display_frame_id(&frame.id)
                        ),
                    });
                }
                append_frame(&mut rewritten_body, version, &frame.id, [0, 0], payload)?;
            }
            ResolvedEdit::Delete { .. } => {}
        }
    }
    for (index, action) in actions.iter().enumerate() {
        if found[index] {
            continue;
        }
        if let ResolvedEdit::Set {
            frame_id, payload, ..
        } = action
        {
            append_frame(&mut rewritten_body, version, frame_id, [0, 0], payload)?;
        }
    }
    rewritten_body.extend_from_slice(&padding);
    if rewritten_body.len() > limits.max_metadata_bytes {
        return Err(MetraError::ResourceLimitExceeded {
            resource: "rewritten ID3v2 tag".to_owned(),
            limit: limits.max_metadata_bytes,
        });
    }
    if rewritten_body.len() > 0x0FFF_FFFF {
        return Err(MetraError::WriteFailure {
            message: "rewritten ID3v2 tag exceeds the synchsafe size limit".to_owned(),
        });
    }
    header[6..10].copy_from_slice(&synchsafe(rewritten_body.len())?);
    write_all(writer, &header)?;
    write_all(writer, &rewritten_body)?;
    copy_remainder(reader, writer, path)
}

fn resolve_edits(version: u8, limits: ParseLimits, edits: &[Mp3Edit]) -> Result<Vec<ResolvedEdit>> {
    let mut actions = Vec::new();
    for edit in edits {
        let action = match edit {
            Mp3Edit::SetText { name, value } => {
                let frame_id =
                    text_frame_id(name, version).ok_or_else(|| unsupported_name(name))?;
                ResolvedEdit::Set {
                    name: name.clone(),
                    frame_id,
                    payload: encode_text_payload(version, value, limits)?,
                }
            }
            Mp3Edit::DeleteText { name } => ResolvedEdit::Delete {
                name: name.clone(),
                frame_id: text_frame_id(name, version).ok_or_else(|| unsupported_name(name))?,
            },
            Mp3Edit::SetComment(value) => ResolvedEdit::Set {
                name: "Comment".to_owned(),
                frame_id: comment_frame_id(version),
                payload: encode_comment_payload(version, value, limits)?,
            },
            Mp3Edit::DeleteComments => ResolvedEdit::Delete {
                name: "Comment".to_owned(),
                frame_id: comment_frame_id(version),
            },
        };
        let target_name = action_name(&action);
        actions.retain(|existing| action_name(existing) != target_name);
        actions.push(action);
    }
    Ok(actions)
}

fn action_name(action: &ResolvedEdit) -> &str {
    match action {
        ResolvedEdit::Set { name, .. } | ResolvedEdit::Delete { name, .. } => name,
    }
}

fn action_frame_id(action: &ResolvedEdit) -> &[u8] {
    match action {
        ResolvedEdit::Set { frame_id, .. } | ResolvedEdit::Delete { frame_id, .. } => frame_id,
    }
}

fn unsupported_name(name: &str) -> MetraError {
    MetraError::WriteFailure {
        message: format!("unsupported ID3 writable text field {name}"),
    }
}

fn text_frame_id(name: &str, version: u8) -> Option<Vec<u8>> {
    let id = match version {
        2 => match name {
            "Title" => b"TT2".as_slice(),
            "Artist" => b"TP1".as_slice(),
            "AlbumArtist" => b"TP2".as_slice(),
            "Album" => b"TAL".as_slice(),
            "RecordingDate" => b"TYE".as_slice(),
            "Genre" => b"TCO".as_slice(),
            "TrackNumber" => b"TRK".as_slice(),
            "DiscNumber" => b"TPA".as_slice(),
            "Composer" => b"TCM".as_slice(),
            "Copyright" => b"TCR".as_slice(),
            "Publisher" => b"TPB".as_slice(),
            "EncodedBy" => b"TEN".as_slice(),
            "EncoderSettings" => b"TSS".as_slice(),
            "InitialKey" => b"TKE".as_slice(),
            "Language" => b"TLA".as_slice(),
            "ContentGroup" => b"TT1".as_slice(),
            "Subtitle" => b"TT3".as_slice(),
            "MediaType" => b"TMT".as_slice(),
            _ => return None,
        },
        3 => match name {
            "Title" => b"TIT2".as_slice(),
            "Artist" => b"TPE1".as_slice(),
            "AlbumArtist" => b"TPE2".as_slice(),
            "Album" => b"TALB".as_slice(),
            "RecordingDate" => b"TYER".as_slice(),
            "Genre" => b"TCON".as_slice(),
            "TrackNumber" => b"TRCK".as_slice(),
            "DiscNumber" => b"TPOS".as_slice(),
            "Composer" => b"TCOM".as_slice(),
            "Copyright" => b"TCOP".as_slice(),
            "Publisher" => b"TPUB".as_slice(),
            "EncodedBy" => b"TENC".as_slice(),
            "EncoderSettings" => b"TSSE".as_slice(),
            "InitialKey" => b"TKEY".as_slice(),
            "Language" => b"TLAN".as_slice(),
            "ContentGroup" => b"TT1".as_slice(),
            "Subtitle" => b"TT3".as_slice(),
            "MediaType" => b"TMED".as_slice(),
            _ => return None,
        },
        4 => match name {
            "Title" => b"TIT2".as_slice(),
            "Artist" => b"TPE1".as_slice(),
            "AlbumArtist" => b"TPE2".as_slice(),
            "Album" => b"TALB".as_slice(),
            "RecordingDate" => b"TDRC".as_slice(),
            "Genre" => b"TCON".as_slice(),
            "TrackNumber" => b"TRCK".as_slice(),
            "DiscNumber" => b"TPOS".as_slice(),
            "Composer" => b"TCOM".as_slice(),
            "BPM" => b"TBPM".as_slice(),
            "DurationMilliseconds" => b"TLEN".as_slice(),
            "Copyright" => b"TCOP".as_slice(),
            "Publisher" => b"TPUB".as_slice(),
            "EncodedBy" => b"TENC".as_slice(),
            "EncoderSettings" => b"TSSE".as_slice(),
            "AlbumSortOrder" => b"TSOA".as_slice(),
            "ArtistSortOrder" => b"TSOP".as_slice(),
            "TitleSortOrder" => b"TSOT".as_slice(),
            "OriginalReleaseDate" => b"TDOR".as_slice(),
            "ReleaseDate" => b"TDRL".as_slice(),
            "InitialKey" => b"TKEY".as_slice(),
            "Language" => b"TLAN".as_slice(),
            "ContentGroup" => b"TT1".as_slice(),
            "Subtitle" => b"TT3".as_slice(),
            "FileType" => b"TFLT".as_slice(),
            "MediaType" => b"TMED".as_slice(),
            _ => return None,
        },
        _ => return None,
    };
    Some(id.to_vec())
}

fn comment_frame_id(version: u8) -> Vec<u8> {
    if version == 2 {
        b"COM".to_vec()
    } else {
        b"COMM".to_vec()
    }
}

fn encode_text_payload(version: u8, value: &str, limits: ParseLimits) -> Result<Vec<u8>> {
    if value.contains('\0') {
        return Err(MetraError::WriteFailure {
            message: "ID3 text values cannot contain NUL".to_owned(),
        });
    }
    let mut payload = if version == 4 {
        let mut bytes = Vec::with_capacity(value.len() + 1);
        bytes.push(3);
        bytes.extend_from_slice(value.as_bytes());
        bytes
    } else {
        encode_utf16_payload(value)
    };
    if payload.len() > limits.max_value_bytes {
        return Err(MetraError::ResourceLimitExceeded {
            resource: "ID3 text value".to_owned(),
            limit: limits.max_value_bytes,
        });
    }
    Ok(std::mem::take(&mut payload))
}

fn encode_comment_payload(version: u8, value: &str, limits: ParseLimits) -> Result<Vec<u8>> {
    if value.contains('\0') {
        return Err(MetraError::WriteFailure {
            message: "ID3 comment values cannot contain NUL".to_owned(),
        });
    }
    let mut payload = Vec::new();
    if version == 4 {
        payload.extend_from_slice(&[3, b'e', b'n', b'g', 0]);
        payload.extend_from_slice(value.as_bytes());
    } else {
        payload.extend_from_slice(&[1, b'e', b'n', b'g', 0, 0]);
        payload.extend_from_slice(&encode_utf16_payload(value)[1..]);
    }
    if payload.len() > limits.max_value_bytes {
        return Err(MetraError::ResourceLimitExceeded {
            resource: "ID3 comment value".to_owned(),
            limit: limits.max_value_bytes,
        });
    }
    Ok(payload)
}

fn encode_utf16_payload(value: &str) -> Vec<u8> {
    let mut payload = vec![1, 0xFF, 0xFE];
    for unit in value.encode_utf16() {
        payload.extend_from_slice(&unit.to_le_bytes());
    }
    payload
}

fn parse_frames_for_rewrite(
    body: &[u8],
    version: u8,
    limits: ParseLimits,
) -> Result<(Vec<Frame>, Vec<u8>)> {
    let (id_length, header_length) = if version == 2 {
        (3_usize, 6_usize)
    } else {
        (4, 10)
    };
    let mut cursor = 0_usize;
    let mut frames = Vec::new();
    while cursor < body.len() {
        if body[cursor] == 0 {
            if body[cursor..].iter().any(|byte| *byte != 0) {
                return Err(MetraError::InvalidTag {
                    context: "ID3v2 padding".to_owned(),
                    message: "non-zero bytes follow ID3 padding".to_owned(),
                });
            }
            return Ok((frames, body[cursor..].to_vec()));
        }
        if frames.len() >= limits.max_jpeg_segments {
            return Err(MetraError::ResourceLimitExceeded {
                resource: "ID3 frames during rewrite".to_owned(),
                limit: limits.max_jpeg_segments,
            });
        }
        if body.len().saturating_sub(cursor) < header_length {
            return Err(MetraError::UnexpectedEof {
                context: "ID3v2 frame header".to_owned(),
            });
        }
        let id = body[cursor..cursor + id_length].to_vec();
        if !id.iter().all(|byte| byte.is_ascii_alphanumeric()) {
            return Err(MetraError::InvalidTag {
                context: "ID3v2 frame identifier".to_owned(),
                message: format!("invalid frame identifier {}", display_frame_id(&id)),
            });
        }
        let size = if version == 2 {
            (u32::from(body[cursor + 3]) << 16)
                | (u32::from(body[cursor + 4]) << 8)
                | u32::from(body[cursor + 5])
        } else if version == 4 {
            parse_synchsafe(&body[cursor + 4..cursor + 8], "ID3v2.4 frame size")?
        } else {
            u32::from_be_bytes(
                body[cursor + 4..cursor + 8]
                    .try_into()
                    .expect("ID3v2.3 frame size"),
            )
        };
        let size = usize::try_from(size).map_err(|_| MetraError::InvalidTag {
            context: "ID3v2 frame size".to_owned(),
            message: "frame size does not fit in memory".to_owned(),
        })?;
        let payload_start = cursor + header_length;
        let payload_end = payload_start
            .checked_add(size)
            .ok_or(MetraError::InvalidOffset {
                context: "ID3v2 frame payload".to_owned(),
                offset: payload_start as u64,
            })?;
        if payload_end > body.len() {
            return Err(MetraError::UnexpectedEof {
                context: "ID3v2 frame payload".to_owned(),
            });
        }
        let flags = if version == 2 {
            [0, 0]
        } else {
            [body[cursor + 8], body[cursor + 9]]
        };
        frames.push(Frame {
            id,
            flags,
            payload: body[payload_start..payload_end].to_vec(),
        });
        cursor = payload_end;
    }
    Ok((frames, Vec::new()))
}

fn append_frame(
    output: &mut Vec<u8>,
    version: u8,
    id: &[u8],
    flags: [u8; 2],
    payload: &[u8],
) -> Result<()> {
    let expected_id_length = if version == 2 { 3 } else { 4 };
    if id.len() != expected_id_length {
        return Err(MetraError::WriteFailure {
            message: "ID3 frame identifier has the wrong length for its version".to_owned(),
        });
    }
    let size = u32::try_from(payload.len()).map_err(|_| MetraError::WriteFailure {
        message: "ID3 frame payload exceeds the 32-bit size limit".to_owned(),
    })?;
    output.extend_from_slice(id);
    if version == 2 {
        if size > 0x00FF_FFFF {
            return Err(MetraError::WriteFailure {
                message: "ID3v2.2 frame payload exceeds the 24-bit size limit".to_owned(),
            });
        }
        output.extend_from_slice(&size.to_be_bytes()[1..]);
    } else if version == 4 {
        if size > 0x0FFF_FFFF {
            return Err(MetraError::WriteFailure {
                message: "ID3v2.4 frame payload exceeds the synchsafe size limit".to_owned(),
            });
        }
        output.extend_from_slice(&synchsafe(size as usize)?);
        output.extend_from_slice(&flags);
    } else {
        output.extend_from_slice(&size.to_be_bytes());
        output.extend_from_slice(&flags);
    }
    output.extend_from_slice(payload);
    Ok(())
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

fn synchsafe(value: usize) -> Result<[u8; 4]> {
    if value > 0x0FFF_FFFF {
        return Err(MetraError::WriteFailure {
            message: "value exceeds the ID3 synchsafe size limit".to_owned(),
        });
    }
    Ok([
        ((value >> 21) & 0x7F) as u8,
        ((value >> 14) & 0x7F) as u8,
        ((value >> 7) & 0x7F) as u8,
        (value & 0x7F) as u8,
    ])
}

fn display_frame_id(id: &[u8]) -> String {
    String::from_utf8_lossy(id).into_owned()
}

fn read_exact<R: Read>(reader: &mut R, bytes: &mut [u8], path: &Path) -> Result<()> {
    reader
        .read_exact(bytes)
        .map_err(|source| io_error(path, source))
}

fn copy_remainder<R: Read, W: Write>(reader: &mut R, writer: &mut W, path: &Path) -> Result<()> {
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|source| io_error(path, source))?;
        if read == 0 {
            return Ok(());
        }
        write_all(writer, &buffer[..read])?;
    }
}

fn write_all<W: Write>(writer: &mut W, bytes: &[u8]) -> Result<()> {
    writer
        .write_all(bytes)
        .map_err(|source| MetraError::WriteFailure {
            message: source.to_string(),
        })
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

fn temporary_path(path: &Path) -> Result<PathBuf> {
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("metadata.mp3");
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| MetraError::WriteFailure {
            message: format!("cannot create temporary name: {error}"),
        })?
        .as_nanos();
    Ok(path.with_file_name(format!(
        ".{filename}.metra-{}-{timestamp}.tmp",
        std::process::id()
    )))
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;
    use crate::id3::read_mp3;

    fn info(size: usize) -> FileInfo {
        FileInfo::new("editable.mp3".into(), size as u64, FileFormat::Mp3)
    }

    fn frame(id: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut result = id.to_vec();
        result.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        result.extend_from_slice(&[0, 0]);
        result.extend_from_slice(payload);
        result
    }

    fn synchsafe_fixture(value: usize) -> [u8; 4] {
        [
            ((value >> 21) & 0x7F) as u8,
            ((value >> 14) & 0x7F) as u8,
            ((value >> 7) & 0x7F) as u8,
            (value & 0x7F) as u8,
        ]
    }

    fn id3v24_file(frames: &[u8]) -> Vec<u8> {
        let mut result = b"ID3".to_vec();
        result.extend_from_slice(&[4, 0, 0]);
        result.extend_from_slice(&synchsafe_fixture(frames.len()));
        result.extend_from_slice(frames);
        result.extend_from_slice(&[0xFF, 0xFB, 0x90, 0x64, 9, 8, 7]);
        result
    }

    fn id3v23_file(frames: &[u8]) -> Vec<u8> {
        let mut result = b"ID3".to_vec();
        result.extend_from_slice(&[3, 0, 0]);
        result.extend_from_slice(&synchsafe_fixture(frames.len()));
        result.extend_from_slice(frames);
        result.extend_from_slice(&[0xFF, 0xFB, 0x90, 0x64, 6, 5]);
        result
    }

    #[test]
    fn replaces_text_and_preserves_other_frames_and_audio() {
        let mut frames = frame(b"TIT2", &[3, b'b', b'e', b'f', b'o', b'r', b'e']);
        frames.extend(frame(b"TPE1", &[3, b'A', b'r', b't', b'i', b's', b't']));
        frames.extend(frame(b"PRIV", &[1, 2, 3, 4]));
        frames.extend_from_slice(&[0; 8]);
        let bytes = id3v24_file(&frames);
        let output = rewrite_mp3_to_vec(
            &bytes,
            info(bytes.len()),
            ParseLimits::default(),
            &[Mp3Edit::SetText {
                name: "Title".to_owned(),
                value: "after".to_owned(),
            }],
        )
        .unwrap();
        let metadata = read_mp3(
            &mut Cursor::new(output.clone()),
            info(output.len()),
            ParseLimits::default(),
        )
        .unwrap();
        assert_eq!(metadata.find("ID3:Title").unwrap().display_value(), "after");
        assert_eq!(
            metadata.find("ID3:Artist").unwrap().display_value(),
            "Artist"
        );
        assert!(
            output
                .windows(14)
                .any(|window| { window == [b'P', b'R', b'I', b'V', 0, 0, 0, 4, 0, 0, 1, 2, 3, 4] })
        );
        assert!(output.ends_with(&[0xFF, 0xFB, 0x90, 0x64, 9, 8, 7]));
    }

    #[test]
    fn deletes_and_inserts_text_and_comments() {
        let frames = frame(b"TIT2", &[3, b'b', b'e', b'f', b'o', b'r', b'e']);
        let bytes = id3v24_file(&frames);
        let deleted = rewrite_mp3_to_vec(
            &bytes,
            info(bytes.len()),
            ParseLimits::default(),
            &[Mp3Edit::DeleteText {
                name: "Title".to_owned(),
            }],
        )
        .unwrap();
        let deleted_metadata = read_mp3(
            &mut Cursor::new(deleted.clone()),
            info(deleted.len()),
            ParseLimits::default(),
        )
        .unwrap();
        assert!(deleted_metadata.find("ID3:Title").is_none());

        let inserted = rewrite_mp3_to_vec(
            &deleted,
            info(deleted.len()),
            ParseLimits::default(),
            &[
                Mp3Edit::SetText {
                    name: "Album".to_owned(),
                    value: "Metra".to_owned(),
                },
                Mp3Edit::SetComment("reviewed".to_owned()),
            ],
        )
        .unwrap();
        let inserted_size = inserted.len();
        let inserted_metadata = read_mp3(
            &mut Cursor::new(inserted),
            info(inserted_size),
            ParseLimits::default(),
        )
        .unwrap();
        assert_eq!(
            inserted_metadata.find("ID3:Album").unwrap().display_value(),
            "Metra"
        );
        assert_eq!(
            inserted_metadata
                .find("ID3:Comment")
                .unwrap()
                .display_value(),
            "reviewed"
        );
    }

    #[test]
    fn encodes_unicode_for_id3v23() {
        let frames = frame(b"TIT2", &[1, 0xFF, 0xFE, b'b', 0, b'e', 0]);
        let bytes = id3v23_file(&frames);
        let output = rewrite_mp3_to_vec(
            &bytes,
            info(bytes.len()),
            ParseLimits::default(),
            &[Mp3Edit::SetText {
                name: "Title".to_owned(),
                value: "été".to_owned(),
            }],
        )
        .unwrap();
        let metadata = read_mp3(
            &mut Cursor::new(output),
            info(bytes.len()),
            ParseLimits::default(),
        )
        .unwrap();
        assert_eq!(metadata.find("ID3:Title").unwrap().display_value(), "été");
    }

    #[test]
    fn rejects_unsupported_tag_flags_before_writing() {
        let mut bytes = id3v24_file(&frame(b"TIT2", &[3, b'b', b'e', b'f', b'o', b'r', b'e']));
        bytes[5] = 0x80;
        let error = rewrite_mp3_to_vec(
            &bytes,
            info(bytes.len()),
            ParseLimits::default(),
            &[Mp3Edit::SetText {
                name: "Title".to_owned(),
                value: "after".to_owned(),
            }],
        )
        .unwrap_err();
        assert!(error.to_string().contains("unsynchronization"));
    }

    #[test]
    fn path_rewrite_validates_before_atomic_replace() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("metra-mp3-rewrite-{unique}"));
        fs::create_dir(&directory).unwrap();
        let path = directory.join("atomic.mp3");
        fs::write(
            &path,
            id3v24_file(&frame(b"TIT2", &[3, b'b', b'e', b'f', b'o', b'r', b'e'])),
        )
        .unwrap();

        rewrite_mp3_path(
            &path,
            ParseLimits::default(),
            &[Mp3Edit::SetText {
                name: "Title".to_owned(),
                value: "atomic".to_owned(),
            }],
        )
        .unwrap();
        let rewritten = fs::read(&path).unwrap();
        let rewritten_size = rewritten.len() as u64;
        let metadata = read_mp3(
            &mut Cursor::new(rewritten),
            FileInfo::new("atomic.mp3".into(), rewritten_size, FileFormat::Mp3),
            ParseLimits::default(),
        )
        .unwrap();
        assert_eq!(
            metadata.find("ID3:Title").unwrap().display_value(),
            "atomic"
        );
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        fs::remove_dir_all(directory).unwrap();
    }
}
