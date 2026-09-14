use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use metra_core::{FileFormat, FileInfo, MetraError, ParseLimits, Result, TagValue};

use crate::atomic::atomic_replace;
use crate::wav::read_wav;

/// Lossless WAV metadata edits for bounded `LIST/INFO` and Broadcast Wave
/// `bext` fields. BWF edits only touch existing chunks and fixed fields (with
/// `CodingHistory` rebuilt inside its existing chunk).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WavEdit {
    SetInfo { name: String, value: String },
    DeleteInfo { name: String },
    SetBext { name: String, value: String },
    DeleteBext { name: String },
}

pub fn rewrite_wav<R: Read + Seek, W: Write + Seek>(
    reader: &mut R,
    writer: &mut W,
    file_info: FileInfo,
    limits: ParseLimits,
    edits: &[WavEdit],
) -> Result<()> {
    read_wav(reader, file_info.clone(), limits)?;
    reader
        .seek(SeekFrom::Start(0))
        .map_err(|source| io_error(&file_info.path, source))?;
    rewrite_wav_stream(
        reader,
        writer,
        &file_info.path,
        file_info.size,
        limits,
        edits,
    )
}

pub fn rewrite_wav_to_vec(
    bytes: &[u8],
    file_info: FileInfo,
    limits: ParseLimits,
    edits: &[WavEdit],
) -> Result<Vec<u8>> {
    let mut reader = std::io::Cursor::new(bytes);
    let mut output = std::io::Cursor::new(Vec::new());
    rewrite_wav(&mut reader, &mut output, file_info.clone(), limits, edits)?;
    let output = output.into_inner();
    let validation_info = FileInfo::new(file_info.path, output.len() as u64, FileFormat::Wav);
    read_wav(
        &mut std::io::Cursor::new(output.as_slice()),
        validation_info,
        limits,
    )?;
    Ok(output)
}

pub fn rewrite_wav_path(
    path: impl AsRef<Path>,
    limits: ParseLimits,
    edits: &[WavEdit],
) -> Result<()> {
    let path = path.as_ref().to_path_buf();
    let source_metadata = fs::metadata(&path).map_err(|source| MetraError::Io {
        path: path.clone(),
        source,
    })?;
    let file_info = FileInfo::new(path.clone(), source_metadata.len(), FileFormat::Wav);
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
        rewrite_wav(&mut input, &mut output, file_info.clone(), limits, edits)?;
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
        let written_info = FileInfo::new(temp_path.clone(), written_size, FileFormat::Wav);
        read_wav(&mut validation, written_info, limits)?;
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
enum InfoAction {
    Set { kind: [u8; 4], value: Vec<u8> },
    Delete { kind: [u8; 4] },
}

#[derive(Debug)]
enum BextAction {
    Set { name: String, value: Vec<u8> },
    Delete { name: String },
}

fn rewrite_wav_stream<R: Read, W: Write + Seek>(
    reader: &mut R,
    writer: &mut W,
    path: &Path,
    file_length: u64,
    limits: ParseLimits,
    edits: &[WavEdit],
) -> Result<()> {
    let action = info_action(edits, limits)?;
    let bext_actions = bext_actions(edits, limits)?;
    let mut header = [0_u8; 12];
    read_exact(reader, &mut header, path)?;
    if &header[..4] != b"RIFF" || &header[8..12] != b"WAVE" {
        return Err(MetraError::InvalidHeader {
            context: "WAV".to_owned(),
            message: "expected RIFF/WAVE signature".to_owned(),
        });
    }
    let declared_size = u64::from(u32::from_le_bytes(
        header[4..8].try_into().expect("RIFF size"),
    ));
    let declared_end = 8_u64
        .checked_add(declared_size)
        .ok_or(MetraError::InvalidOffset {
            context: "WAV RIFF boundary".to_owned(),
            offset: declared_size,
        })?;
    if declared_end > file_length {
        return Err(MetraError::UnexpectedEof {
            context: "WAV RIFF body".to_owned(),
        });
    }
    header[4..8].copy_from_slice(&0_u32.to_le_bytes());
    write_all(writer, &header)?;

    let mut input_offset = 12_u64;
    let mut chunk_count = 0_usize;
    let mut inserted = false;
    let mut bext_found = false;
    while input_offset < declared_end {
        if chunk_count >= limits.max_jpeg_segments {
            return Err(MetraError::ResourceLimitExceeded {
                resource: "WAV chunks during rewrite".to_owned(),
                limit: limits.max_jpeg_segments,
            });
        }
        if declared_end - input_offset < 8 {
            return Err(MetraError::UnexpectedEof {
                context: "WAV chunk header".to_owned(),
            });
        }
        let mut chunk_header = [0_u8; 8];
        read_exact(reader, &mut chunk_header, path)?;
        let kind: [u8; 4] = chunk_header[..4]
            .try_into()
            .expect("WAV chunk id is four bytes");
        let length = u64::from(u32::from_le_bytes(
            chunk_header[4..8].try_into().expect("WAV chunk length"),
        ));
        let data_end = input_offset
            .checked_add(8)
            .and_then(|offset| offset.checked_add(length))
            .ok_or(MetraError::InvalidOffset {
                context: format!("WAV {} chunk", fourcc(&kind)),
                offset: input_offset,
            })?;
        let padded_end = data_end
            .checked_add(length & 1)
            .ok_or(MetraError::InvalidOffset {
                context: format!("WAV {} padding", fourcc(&kind)),
                offset: data_end,
            })?;
        if padded_end > declared_end {
            return Err(MetraError::UnexpectedEof {
                context: format!("WAV {} chunk", fourcc(&kind)),
            });
        }

        if &kind == b"LIST" {
            let length =
                usize::try_from(length).map_err(|_| MetraError::ResourceLimitExceeded {
                    resource: "WAV LIST chunk during rewrite".to_owned(),
                    limit: limits.max_metadata_bytes,
                })?;
            if length > limits.max_metadata_bytes {
                return Err(MetraError::ResourceLimitExceeded {
                    resource: "WAV LIST chunk during rewrite".to_owned(),
                    limit: limits.max_metadata_bytes,
                });
            }
            let mut data = vec![0_u8; length];
            read_exact(reader, &mut data, path)?;
            if length % 2 == 1 {
                let mut padding = [0_u8; 1];
                read_exact(reader, &mut padding, path)?;
            }
            let (rewritten, found) = rewrite_list_info(&data, action.as_ref(), limits)?;
            inserted |= found;
            write_chunk(writer, &kind, &rewritten)?;
        } else if &kind == b"bext" && !bext_actions.is_empty() {
            let length =
                usize::try_from(length).map_err(|_| MetraError::ResourceLimitExceeded {
                    resource: "WAV bext chunk during rewrite".to_owned(),
                    limit: limits.max_metadata_bytes,
                })?;
            if length > limits.max_metadata_bytes {
                return Err(MetraError::ResourceLimitExceeded {
                    resource: "WAV bext chunk during rewrite".to_owned(),
                    limit: limits.max_metadata_bytes,
                });
            }
            let mut data = vec![0_u8; length];
            read_exact(reader, &mut data, path)?;
            if length % 2 == 1 {
                let mut padding = [0_u8; 1];
                read_exact(reader, &mut padding, path)?;
            }
            let rewritten = rewrite_bext(&data, &bext_actions, limits)?;
            bext_found = true;
            write_chunk(writer, &kind, &rewritten)?;
        } else {
            write_all(writer, &chunk_header)?;
            copy_exact(reader, writer, length, path)?;
            copy_exact(reader, writer, length & 1, path)?;
        }
        input_offset = padded_end;
        chunk_count += 1;
    }

    if let Some(InfoAction::Set { kind, value }) = action.as_ref()
        && !inserted
    {
        let data = info_list_with_entry(kind, value)?;
        write_chunk(writer, b"LIST", &data)?;
    }
    if !bext_actions.is_empty() && !bext_found {
        return Err(MetraError::WriteFailure {
            message: "WAV bext edits require an existing bext chunk".to_owned(),
        });
    }

    let riff_output_end = writer
        .stream_position()
        .map_err(|source| write_io_error(path, source))?;
    let riff_size = riff_output_end
        .checked_sub(8)
        .and_then(|size| u32::try_from(size).ok())
        .ok_or(MetraError::WriteFailure {
            message: "rewritten WAV exceeds the RIFF 32-bit size limit".to_owned(),
        })?;
    writer
        .seek(SeekFrom::Start(4))
        .map_err(|source| write_io_error(path, source))?;
    write_all(writer, &riff_size.to_le_bytes())?;
    writer
        .seek(SeekFrom::Start(riff_output_end))
        .map_err(|source| write_io_error(path, source))?;
    copy_exact(reader, writer, file_length - declared_end, path)
}

fn info_action(edits: &[WavEdit], limits: ParseLimits) -> Result<Option<InfoAction>> {
    let mut action = None;
    for edit in edits {
        match edit {
            WavEdit::SetInfo { name, value } => {
                let kind = info_kind(name).ok_or_else(|| MetraError::WriteFailure {
                    message: format!("unsupported WAV LIST/INFO field {name}"),
                })?;
                if value.contains('\0') {
                    return Err(MetraError::WriteFailure {
                        message: "WAV INFO values cannot contain NUL".to_owned(),
                    });
                }
                if value.len() > limits.max_value_bytes {
                    return Err(MetraError::ResourceLimitExceeded {
                        resource: "WAV INFO value".to_owned(),
                        limit: limits.max_value_bytes,
                    });
                }
                action = Some(InfoAction::Set {
                    kind,
                    value: value.as_bytes().iter().copied().chain([0]).collect(),
                });
            }
            WavEdit::DeleteInfo { name } => {
                let kind = info_kind(name).ok_or_else(|| MetraError::WriteFailure {
                    message: format!("unsupported WAV LIST/INFO field {name}"),
                })?;
                action = Some(InfoAction::Delete { kind });
            }
            WavEdit::SetBext { .. } | WavEdit::DeleteBext { .. } => {}
        }
    }
    Ok(action)
}

fn bext_actions(edits: &[WavEdit], limits: ParseLimits) -> Result<Vec<BextAction>> {
    edits
        .iter()
        .filter_map(|edit| match edit {
            WavEdit::SetBext { name, value } => Some(encode_bext_value(name, value, limits).map(
                |value| BextAction::Set {
                    name: name.clone(),
                    value,
                },
            )),
            WavEdit::DeleteBext { name } => {
                Some(validate_bext_name(name).map(|()| BextAction::Delete { name: name.clone() }))
            }
            WavEdit::SetInfo { .. } | WavEdit::DeleteInfo { .. } => None,
        })
        .collect()
}

fn encode_bext_value(name: &str, value: &str, limits: ParseLimits) -> Result<Vec<u8>> {
    match name {
        "Description" => encode_bext_text(name, value, 256),
        "Originator" => encode_bext_text(name, value, 32),
        "OriginatorReference" => encode_bext_text(name, value, 32),
        "DateTimeOriginal" => encode_bext_datetime(value),
        "TimeReference" => value
            .trim()
            .parse::<u64>()
            .map(|value| value.to_le_bytes().to_vec())
            .map_err(|_| invalid_bext_value(name, "expected an unsigned 64-bit sample count")),
        "BWFVersion" => value
            .trim()
            .parse::<u16>()
            .map(|value| value.to_le_bytes().to_vec())
            .map_err(|_| invalid_bext_value(name, "expected an unsigned 16-bit version")),
        "BWF_UMID" => decode_bext_hex(name, value),
        "CodingHistory" => {
            if value.contains('\0') || !value.is_ascii() {
                return Err(invalid_bext_value(
                    name,
                    "value must be ASCII and cannot contain NUL",
                ));
            }
            let length = value
                .len()
                .checked_add(1)
                .ok_or_else(|| invalid_bext_value(name, "value length overflows"))?;
            if length > limits.max_value_bytes {
                return Err(MetraError::ResourceLimitExceeded {
                    resource: format!("WAV bext {name} value"),
                    limit: limits.max_value_bytes,
                });
            }
            let mut bytes = value.as_bytes().to_vec();
            bytes.push(0);
            Ok(bytes)
        }
        _ => Err(invalid_bext_value(name, "unsupported Broadcast Wave field")),
    }
}

fn encode_bext_text(name: &str, value: &str, width: usize) -> Result<Vec<u8>> {
    if value.contains('\0') || !value.is_ascii() {
        return Err(invalid_bext_value(
            name,
            "value must be ASCII and cannot contain NUL",
        ));
    }
    if value.len() > width {
        return Err(invalid_bext_value(
            name,
            "value exceeds the fixed field width",
        ));
    }
    let mut encoded = vec![0_u8; width];
    encoded[..value.len()].copy_from_slice(value.as_bytes());
    Ok(encoded)
}

fn encode_bext_datetime(value: &str) -> Result<Vec<u8>> {
    let parsed = crate::wav::parse_bwf_datetime(value).ok_or_else(|| {
        invalid_bext_value(
            "DateTimeOriginal",
            "expected a valid YYYY:MM:DD HH:MM:SS date-time",
        )
    })?;
    let TagValue::DateTime {
        year,
        month,
        day,
        hour,
        minute,
        second,
        ..
    } = parsed
    else {
        return Err(invalid_bext_value(
            "DateTimeOriginal",
            "expected a valid date-time",
        ));
    };
    Ok(format!("{year:04}-{month:02}-{day:02}{hour:02}:{minute:02}:{second:02}").into_bytes())
}

fn decode_bext_hex(name: &str, value: &str) -> Result<Vec<u8>> {
    let value = value.trim();
    if value.len() > 128 || !value.len().is_multiple_of(2) {
        return Err(invalid_bext_value(
            name,
            "expected at most 64 bytes written as an even-length hexadecimal value",
        ));
    }
    if !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(invalid_bext_value(name, "expected hexadecimal digits"));
    }
    let mut encoded = vec![0_u8; 64];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        encoded[index] = (hex_nibble(pair[0]) << 4) | hex_nibble(pair[1]);
    }
    Ok(encoded)
}

fn hex_nibble(byte: u8) -> u8 {
    match byte {
        b'0'..=b'9' => byte - b'0',
        b'a'..=b'f' => byte - b'a' + 10,
        b'A'..=b'F' => byte - b'A' + 10,
        _ => unreachable!("hexadecimal input is validated before conversion"),
    }
}

fn validate_bext_name(name: &str) -> Result<()> {
    if matches!(
        name,
        "Description"
            | "Originator"
            | "OriginatorReference"
            | "DateTimeOriginal"
            | "TimeReference"
            | "BWFVersion"
            | "BWF_UMID"
            | "CodingHistory"
    ) {
        Ok(())
    } else {
        Err(invalid_bext_value(name, "unsupported Broadcast Wave field"))
    }
}

fn invalid_bext_value(name: &str, message: &str) -> MetraError {
    MetraError::WriteFailure {
        message: format!("invalid WAV bext {name}: {message}"),
    }
}

fn bext_fixed_field(name: &str) -> Option<(usize, usize)> {
    Some(match name {
        "Description" => (0, 256),
        "Originator" => (256, 32),
        "OriginatorReference" => (288, 32),
        "DateTimeOriginal" => (320, 18),
        "TimeReference" => (338, 8),
        "BWFVersion" => (346, 2),
        "BWF_UMID" => (348, 64),
        _ => return None,
    })
}

fn rewrite_bext(data: &[u8], actions: &[BextAction], limits: ParseLimits) -> Result<Vec<u8>> {
    let mut result = data.to_vec();
    for action in actions {
        match action {
            BextAction::Set { name, value } => {
                if name == "CodingHistory" {
                    if result.len() < 602 {
                        return Err(MetraError::UnexpectedEof {
                            context: "WAV bext CodingHistory base".to_owned(),
                        });
                    }
                    result.truncate(602);
                    result.extend_from_slice(value);
                } else {
                    let (start, width) = bext_fixed_field(name).ok_or_else(|| {
                        invalid_bext_value(name, "unsupported Broadcast Wave field")
                    })?;
                    let end = start
                        .checked_add(width)
                        .ok_or_else(|| invalid_bext_value(name, "fixed field range overflows"))?;
                    if result.len() < end {
                        return Err(MetraError::UnexpectedEof {
                            context: format!("WAV bext {name} field"),
                        });
                    }
                    if value.len() != width {
                        return Err(invalid_bext_value(
                            name,
                            "encoded value has the wrong width",
                        ));
                    }
                    result[start..end].copy_from_slice(value);
                }
            }
            BextAction::Delete { name } => {
                if name == "CodingHistory" {
                    if result.len() < 602 {
                        return Err(MetraError::UnexpectedEof {
                            context: "WAV bext CodingHistory base".to_owned(),
                        });
                    }
                    result.truncate(602);
                } else {
                    let (start, width) = bext_fixed_field(name).ok_or_else(|| {
                        invalid_bext_value(name, "unsupported Broadcast Wave field")
                    })?;
                    let end = start
                        .checked_add(width)
                        .ok_or_else(|| invalid_bext_value(name, "fixed field range overflows"))?;
                    if result.len() < end {
                        return Err(MetraError::UnexpectedEof {
                            context: format!("WAV bext {name} field"),
                        });
                    }
                    result[start..end].fill(0);
                }
            }
        }
        if result.len() > limits.max_metadata_bytes {
            return Err(MetraError::ResourceLimitExceeded {
                resource: "WAV bext metadata".to_owned(),
                limit: limits.max_metadata_bytes,
            });
        }
    }
    Ok(result)
}

fn rewrite_list_info(
    data: &[u8],
    action: Option<&InfoAction>,
    limits: ParseLimits,
) -> Result<(Vec<u8>, bool)> {
    let Some(action) = action else {
        return Ok((data.to_vec(), false));
    };
    if data.len() < 4 || &data[..4] != b"INFO" {
        return Ok((data.to_vec(), false));
    }
    let mut result = data[..4].to_vec();
    let mut cursor = 4_usize;
    let mut found = false;
    while cursor < data.len() {
        if data.len() - cursor < 8 {
            return Err(MetraError::InvalidTag {
                context: "WAV LIST/INFO".to_owned(),
                message: "subchunk header is truncated".to_owned(),
            });
        }
        let kind: [u8; 4] = data[cursor..cursor + 4]
            .try_into()
            .expect("WAV INFO id is four bytes");
        let length = usize::try_from(u32::from_le_bytes(
            data[cursor + 4..cursor + 8]
                .try_into()
                .expect("WAV INFO length"),
        ))
        .map_err(|_| MetraError::ResourceLimitExceeded {
            resource: "WAV INFO value".to_owned(),
            limit: limits.max_value_bytes,
        })?;
        let value_start = cursor + 8;
        let value_end = value_start
            .checked_add(length)
            .ok_or(MetraError::InvalidOffset {
                context: "WAV LIST/INFO value".to_owned(),
                offset: value_start as u64,
            })?;
        let padded_end = value_end
            .checked_add(length & 1)
            .ok_or(MetraError::InvalidOffset {
                context: "WAV LIST/INFO padding".to_owned(),
                offset: value_end as u64,
            })?;
        if padded_end > data.len() {
            return Err(MetraError::InvalidTag {
                context: "WAV LIST/INFO".to_owned(),
                message: "subchunk value exceeds its LIST boundary".to_owned(),
            });
        }

        if action_kind(action) == kind {
            found = true;
            if let InfoAction::Set { kind, value } = action {
                append_info_entry(&mut result, kind, value)?;
            }
        } else {
            result.extend_from_slice(&data[cursor..padded_end]);
        }
        cursor = padded_end;
    }
    if let InfoAction::Set { kind, value } = action
        && !found
    {
        append_info_entry(&mut result, kind, value)?;
        found = true;
    }
    if result.len() > limits.max_metadata_bytes {
        return Err(MetraError::ResourceLimitExceeded {
            resource: "WAV LIST/INFO metadata".to_owned(),
            limit: limits.max_metadata_bytes,
        });
    }
    Ok((result, found))
}

fn action_kind(action: &InfoAction) -> [u8; 4] {
    match action {
        InfoAction::Set { kind, .. } | InfoAction::Delete { kind } => *kind,
    }
}

fn info_list_with_entry(kind: &[u8; 4], value: &[u8]) -> Result<Vec<u8>> {
    let mut data = b"INFO".to_vec();
    append_info_entry(&mut data, kind, value)?;
    Ok(data)
}

fn append_info_entry(output: &mut Vec<u8>, kind: &[u8; 4], value: &[u8]) -> Result<()> {
    let length = u32::try_from(value.len()).map_err(|_| MetraError::WriteFailure {
        message: "WAV INFO value exceeds the 32-bit size limit".to_owned(),
    })?;
    output.extend_from_slice(kind);
    output.extend_from_slice(&length.to_le_bytes());
    output.extend_from_slice(value);
    if value.len() % 2 == 1 {
        output.push(0);
    }
    Ok(())
}

fn write_chunk<W: Write>(writer: &mut W, kind: &[u8; 4], data: &[u8]) -> Result<()> {
    let length = u32::try_from(data.len()).map_err(|_| MetraError::WriteFailure {
        message: format!("WAV {} chunk exceeds the 32-bit size limit", fourcc(kind)),
    })?;
    write_all(writer, kind)?;
    write_all(writer, &length.to_le_bytes())?;
    write_all(writer, data)?;
    if data.len() % 2 == 1 {
        write_all(writer, &[0])?;
    }
    Ok(())
}

pub(crate) fn info_kind(name: &str) -> Option<[u8; 4]> {
    Some(match name {
        "Title" => *b"INAM",
        "Artist" => *b"IART",
        "Product" => *b"IPRD",
        "Comment" => *b"ICMT",
        "CreationDate" => *b"ICRD",
        "Genre" => *b"IGNR",
        "Engineer" => *b"IENG",
        "Software" => *b"ISFT",
        "Copyright" => *b"ICOP",
        "Technician" => *b"ITCH",
        "Subject" => *b"ISBJ",
        "Source" => *b"ISRC",
        _ => return None,
    })
}

fn fourcc(bytes: &[u8; 4]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn read_exact<R: Read>(reader: &mut R, bytes: &mut [u8], path: &Path) -> Result<()> {
    reader
        .read_exact(bytes)
        .map_err(|source| io_error(path, source))
}

fn copy_exact<R: Read, W: Write>(
    reader: &mut R,
    writer: &mut W,
    mut length: u64,
    path: &Path,
) -> Result<()> {
    let mut buffer = [0_u8; 64 * 1024];
    while length > 0 {
        let requested = usize::try_from(length)
            .unwrap_or(buffer.len())
            .min(buffer.len());
        reader
            .read_exact(&mut buffer[..requested])
            .map_err(|source| io_error(path, source))?;
        write_all(writer, &buffer[..requested])?;
        length -= requested as u64;
    }
    Ok(())
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

fn write_io_error(path: &Path, source: std::io::Error) -> MetraError {
    MetraError::WriteFailure {
        message: format!("{}: {source}", path.display()),
    }
}

fn temporary_path(path: &Path) -> Result<PathBuf> {
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("metadata.wav");
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

    fn info(size: usize) -> FileInfo {
        FileInfo::new("editable.wav".into(), size as u64, FileFormat::Wav)
    }

    fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut result = kind.to_vec();
        result.extend_from_slice(&(data.len() as u32).to_le_bytes());
        result.extend_from_slice(data);
        if !data.len().is_multiple_of(2) {
            result.push(0);
        }
        result
    }

    fn wav_with_info(title: &str) -> Vec<u8> {
        let mut fmt = Vec::new();
        fmt.extend_from_slice(&1_u16.to_le_bytes());
        fmt.extend_from_slice(&1_u16.to_le_bytes());
        fmt.extend_from_slice(&8_000_u32.to_le_bytes());
        fmt.extend_from_slice(&8_000_u32.to_le_bytes());
        fmt.extend_from_slice(&1_u16.to_le_bytes());
        fmt.extend_from_slice(&8_u16.to_le_bytes());
        let mut list = b"INFO".to_vec();
        list.extend(chunk(b"INAM", format!("{title}\0").as_bytes()));
        let mut body = chunk(b"fmt ", &fmt);
        body.extend(chunk(b"LIST", &list));
        body.extend(chunk(b"data", &[9, 8, 7, 6]));
        let mut bytes = b"RIFF".to_vec();
        bytes.extend_from_slice(&((4 + body.len()) as u32).to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        bytes.extend(body);
        bytes
    }

    fn wav_with_bext() -> Vec<u8> {
        let mut fmt = Vec::new();
        fmt.extend_from_slice(&1_u16.to_le_bytes());
        fmt.extend_from_slice(&1_u16.to_le_bytes());
        fmt.extend_from_slice(&48_000_u32.to_le_bytes());
        fmt.extend_from_slice(&48_000_u32.to_le_bytes());
        fmt.extend_from_slice(&1_u16.to_le_bytes());
        fmt.extend_from_slice(&8_u16.to_le_bytes());

        let mut bext = vec![0_u8; 602];
        bext[..13].copy_from_slice(b"Original take");
        bext[256..266].copy_from_slice(b"Metra crew");
        bext[320..330].copy_from_slice(b"2026-09-14");
        bext[330..338].copy_from_slice(b"12:34:56");
        bext[338..346].copy_from_slice(&17_u64.to_le_bytes());
        bext[346..348].copy_from_slice(&1_u16.to_le_bytes());
        bext.extend_from_slice(b"A=PCM,F=48000,W=8,M=mono\0");

        let mut body = chunk(b"fmt ", &fmt);
        body.extend(chunk(b"bext", &bext));
        body.extend(chunk(b"data", &[9, 8, 7, 6]));
        let mut bytes = b"RIFF".to_vec();
        bytes.extend_from_slice(&((4 + body.len()) as u32).to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        bytes.extend(body);
        bytes
    }

    #[test]
    fn replaces_info_and_preserves_audio_chunks() {
        let bytes = wav_with_info("before");
        let output = rewrite_wav_to_vec(
            &bytes,
            info(bytes.len()),
            ParseLimits::default(),
            &[WavEdit::SetInfo {
                name: "Title".to_owned(),
                value: "after".to_owned(),
            }],
        )
        .unwrap();
        let parsed = read_wav(
            &mut Cursor::new(output.clone()),
            info(output.len()),
            ParseLimits::default(),
        )
        .unwrap();
        assert_eq!(parsed.find("WAV:Title").unwrap().display_value(), "after");
        assert!(
            output
                .windows(12)
                .any(|window| window == [b'd', b'a', b't', b'a', 4, 0, 0, 0, 9, 8, 7, 6])
        );
    }

    #[test]
    fn deletes_and_inserts_info_fields() {
        let bytes = wav_with_info("before");
        let deleted = rewrite_wav_to_vec(
            &bytes,
            info(bytes.len()),
            ParseLimits::default(),
            &[WavEdit::DeleteInfo {
                name: "Title".to_owned(),
            }],
        )
        .unwrap();
        let deleted_metadata = read_wav(
            &mut Cursor::new(deleted.clone()),
            info(deleted.len()),
            ParseLimits::default(),
        )
        .unwrap();
        assert!(deleted_metadata.find("WAV:Title").is_none());

        let inserted = rewrite_wav_to_vec(
            &deleted,
            info(deleted.len()),
            ParseLimits::default(),
            &[WavEdit::SetInfo {
                name: "Artist".to_owned(),
                value: "Metra".to_owned(),
            }],
        )
        .unwrap();
        let inserted_metadata = read_wav(
            &mut Cursor::new(inserted.clone()),
            info(inserted.len()),
            ParseLimits::default(),
        )
        .unwrap();
        assert_eq!(
            inserted_metadata
                .find("WAV:Artist")
                .unwrap()
                .display_value(),
            "Metra"
        );
    }

    #[test]
    fn rewrites_existing_bext_fields_without_touching_audio() {
        let bytes = wav_with_bext();
        let output = rewrite_wav_to_vec(
            &bytes,
            info(bytes.len()),
            ParseLimits::default(),
            &[
                WavEdit::SetBext {
                    name: "Description".to_owned(),
                    value: "Edited take".to_owned(),
                },
                WavEdit::SetBext {
                    name: "DateTimeOriginal".to_owned(),
                    value: "2026:09:15 01:02:03".to_owned(),
                },
                WavEdit::SetBext {
                    name: "TimeReference".to_owned(),
                    value: "8589934595".to_owned(),
                },
                WavEdit::SetBext {
                    name: "BWFVersion".to_owned(),
                    value: "2".to_owned(),
                },
                WavEdit::SetBext {
                    name: "BWF_UMID".to_owned(),
                    value: "CD".repeat(32),
                },
                WavEdit::SetBext {
                    name: "CodingHistory".to_owned(),
                    value: "A=PCM,F=96000,W=24,M=stereo".to_owned(),
                },
            ],
        )
        .unwrap();
        let parsed = read_wav(
            &mut Cursor::new(output.clone()),
            info(output.len()),
            ParseLimits::default(),
        )
        .unwrap();
        assert_eq!(
            parsed.find("WAV:Description").unwrap().display_value(),
            "Edited take"
        );
        assert_eq!(
            parsed.find("WAV:DateTimeOriginal").unwrap().display_value(),
            "2026:09:15 01:02:03"
        );
        assert_eq!(
            parsed.find("WAV:TimeReference").unwrap().display_value(),
            "8589934595"
        );
        assert_eq!(parsed.find("WAV:BWFVersion").unwrap().display_value(), "2");
        assert_eq!(
            parsed.find("WAV:BWF_UMID").unwrap().display_value(),
            "CD".repeat(32)
        );
        assert_eq!(
            parsed.find("WAV:CodingHistory").unwrap().display_value(),
            "A=PCM,F=96000,W=24,M=stereo"
        );
        assert!(
            output
                .windows(12)
                .any(|window| window == [b'd', b'a', b't', b'a', 4, 0, 0, 0, 9, 8, 7, 6])
        );
    }

    #[test]
    fn deletes_bext_fields_and_rejects_invalid_values() {
        let bytes = wav_with_bext();
        let deleted = rewrite_wav_to_vec(
            &bytes,
            info(bytes.len()),
            ParseLimits::default(),
            &[
                WavEdit::DeleteBext {
                    name: "Description".to_owned(),
                },
                WavEdit::DeleteBext {
                    name: "DateTimeOriginal".to_owned(),
                },
                WavEdit::DeleteBext {
                    name: "CodingHistory".to_owned(),
                },
            ],
        )
        .unwrap();
        let parsed = read_wav(
            &mut Cursor::new(deleted.clone()),
            info(deleted.len()),
            ParseLimits::default(),
        )
        .unwrap();
        assert!(parsed.find("WAV:Description").is_none());
        assert!(parsed.find("WAV:DateTimeOriginal").is_none());
        assert!(parsed.find("WAV:CodingHistory").is_none());

        let invalid = rewrite_wav_to_vec(
            &bytes,
            info(bytes.len()),
            ParseLimits::default(),
            &[WavEdit::SetBext {
                name: "TimeReference".to_owned(),
                value: "not-a-number".to_owned(),
            }],
        )
        .unwrap_err();
        assert!(invalid.to_string().contains("TimeReference"));

        let missing = rewrite_wav_to_vec(
            &wav_with_info("before"),
            info(wav_with_info("before").len()),
            ParseLimits::default(),
            &[WavEdit::SetBext {
                name: "Description".to_owned(),
                value: "new".to_owned(),
            }],
        )
        .unwrap_err();
        assert!(missing.to_string().contains("existing bext"));
    }

    #[test]
    fn rejects_unsupported_info_names_and_nul_values() {
        let bytes = wav_with_info("before");
        let error = rewrite_wav_to_vec(
            &bytes,
            info(bytes.len()),
            ParseLimits::default(),
            &[WavEdit::SetInfo {
                name: "Unknown".to_owned(),
                value: "value".to_owned(),
            }],
        )
        .unwrap_err();
        assert!(error.to_string().contains("unsupported WAV"));

        let error = rewrite_wav_to_vec(
            &bytes,
            info(bytes.len()),
            ParseLimits::default(),
            &[WavEdit::SetInfo {
                name: "Title".to_owned(),
                value: "bad\0value".to_owned(),
            }],
        )
        .unwrap_err();
        assert!(error.to_string().contains("NUL"));
    }
}
