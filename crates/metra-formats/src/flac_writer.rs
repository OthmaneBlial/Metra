use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use metra_core::{FileFormat, FileInfo, MetraError, ParseLimits, Result};

use crate::atomic::atomic_replace;
use crate::flac::read_flac;

/// Lossless FLAC metadata edits for Vorbis Comment key/value pairs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlacEdit {
    SetComment { key: String, value: String },
    DeleteComment { key: String },
}

pub fn rewrite_flac<R: Read + Seek, W: Write>(
    reader: &mut R,
    writer: &mut W,
    file_info: FileInfo,
    limits: ParseLimits,
    edits: &[FlacEdit],
) -> Result<()> {
    read_flac(reader, file_info.clone(), limits)?;
    reader
        .seek(SeekFrom::Start(0))
        .map_err(|source| io_error(&file_info.path, source))?;
    rewrite_flac_stream(reader, writer, &file_info.path, limits, edits)
}

pub fn rewrite_flac_to_vec(
    bytes: &[u8],
    file_info: FileInfo,
    limits: ParseLimits,
    edits: &[FlacEdit],
) -> Result<Vec<u8>> {
    let mut reader = std::io::Cursor::new(bytes);
    let mut output = Vec::new();
    rewrite_flac(&mut reader, &mut output, file_info.clone(), limits, edits)?;
    let validation_info = FileInfo::new(file_info.path, output.len() as u64, FileFormat::Flac);
    read_flac(
        &mut std::io::Cursor::new(output.as_slice()),
        validation_info,
        limits,
    )?;
    Ok(output)
}

pub fn rewrite_flac_path(
    path: impl AsRef<Path>,
    limits: ParseLimits,
    edits: &[FlacEdit],
) -> Result<()> {
    let path = path.as_ref().to_path_buf();
    let source_metadata = fs::metadata(&path).map_err(|source| MetraError::Io {
        path: path.clone(),
        source,
    })?;
    let file_info = FileInfo::new(path.clone(), source_metadata.len(), FileFormat::Flac);
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
        rewrite_flac(&mut input, &mut output, file_info.clone(), limits, edits)?;
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
        read_flac(
            &mut validation,
            FileInfo::new(temp_path.clone(), written_size, FileFormat::Flac),
            limits,
        )?;
        drop(validation);
        drop(input);
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

fn rewrite_flac_stream<R: Read, W: Write>(
    reader: &mut R,
    writer: &mut W,
    path: &Path,
    limits: ParseLimits,
    edits: &[FlacEdit],
) -> Result<()> {
    let action = comment_action(edits, limits)?;
    let mut signature = [0_u8; 4];
    read_exact(reader, &mut signature, path)?;
    if &signature != b"fLaC" {
        return Err(MetraError::InvalidHeader {
            context: "FLAC".to_owned(),
            message: "expected fLaC signature".to_owned(),
        });
    }
    write_all(writer, &signature)?;

    let mut block_count = 0_usize;
    let mut saw_last = false;
    let mut inserted = false;
    while !saw_last {
        if block_count >= limits.max_jpeg_segments {
            return Err(MetraError::ResourceLimitExceeded {
                resource: "FLAC metadata blocks during rewrite".to_owned(),
                limit: limits.max_jpeg_segments,
            });
        }
        let mut header = [0_u8; 4];
        read_exact(reader, &mut header, path)?;
        let is_last = header[0] & 0x80 != 0;
        let block_type = header[0] & 0x7F;
        let length =
            (usize::from(header[1]) << 16) | (usize::from(header[2]) << 8) | usize::from(header[3]);
        if length > limits.max_metadata_bytes {
            return Err(MetraError::ResourceLimitExceeded {
                resource: format!("FLAC block type {block_type} during rewrite"),
                limit: limits.max_metadata_bytes,
            });
        }

        if block_type == 4 {
            let mut data = vec![0_u8; length];
            read_exact(reader, &mut data, path)?;
            let (rewritten, found) = rewrite_vorbis_comments(&data, action.as_ref(), limits)?;
            inserted |= found;
            write_block(writer, is_last, 4, &rewritten)?;
        } else {
            if let Some(CommentAction::Set { key, value }) = action.as_ref()
                && is_last
                && !inserted
            {
                write_block(writer, false, 4, &vorbis_block_with_comment(key, value)?)?;
                inserted = true;
            }
            write_all(writer, &header)?;
            copy_exact(reader, writer, length as u64, path)?;
        }
        saw_last = is_last;
        block_count += 1;
    }
    copy_remainder(reader, writer, path)
}

fn comment_action(edits: &[FlacEdit], limits: ParseLimits) -> Result<Option<CommentAction>> {
    let mut action = None;
    for edit in edits {
        match edit {
            FlacEdit::SetComment { key, value } => {
                let key = validate_key(key)?;
                if value.contains('\0') {
                    return Err(MetraError::WriteFailure {
                        message: "FLAC Vorbis comment values cannot contain NUL".to_owned(),
                    });
                }
                if value.len() > limits.max_value_bytes {
                    return Err(MetraError::ResourceLimitExceeded {
                        resource: "FLAC Vorbis comment value".to_owned(),
                        limit: limits.max_value_bytes,
                    });
                }
                action = Some(CommentAction::Set {
                    key,
                    value: value.as_bytes().to_vec(),
                });
            }
            FlacEdit::DeleteComment { key } => {
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
            message: "FLAC Vorbis comment keys must be printable ASCII without '='".to_owned(),
        });
    }
    Ok(key.to_ascii_uppercase().into_bytes())
}

fn rewrite_vorbis_comments(
    data: &[u8],
    action: Option<&CommentAction>,
    limits: ParseLimits,
) -> Result<(Vec<u8>, bool)> {
    if data.len() < 8 {
        return Err(MetraError::InvalidTag {
            context: "FLAC VORBIS_COMMENT".to_owned(),
            message: "vendor and comment count fields are truncated".to_owned(),
        });
    }
    let vendor_length = usize::try_from(u32::from_le_bytes(
        data[..4].try_into().expect("FLAC vendor length"),
    ))
    .map_err(|_| MetraError::InvalidOffset {
        context: "FLAC vendor length".to_owned(),
        offset: 4,
    })?;
    let vendor_end = 4_usize
        .checked_add(vendor_length)
        .ok_or(MetraError::InvalidOffset {
            context: "FLAC vendor string".to_owned(),
            offset: vendor_length as u64,
        })?;
    let count_end = vendor_end.checked_add(4).ok_or(MetraError::InvalidOffset {
        context: "FLAC comment count".to_owned(),
        offset: vendor_end as u64,
    })?;
    if count_end > data.len() {
        return Err(MetraError::UnexpectedEof {
            context: "FLAC VORBIS_COMMENT vendor".to_owned(),
        });
    }
    let count = u32::from_le_bytes(
        data[vendor_end..count_end]
            .try_into()
            .expect("FLAC comment count"),
    );
    let mut cursor = count_end;
    let mut comments = Vec::new();
    let mut found = false;
    for _ in 0..count {
        let length_end = cursor.checked_add(4).ok_or(MetraError::InvalidOffset {
            context: "FLAC comment length".to_owned(),
            offset: cursor as u64,
        })?;
        if length_end > data.len() {
            return Err(MetraError::UnexpectedEof {
                context: "FLAC VORBIS_COMMENT length".to_owned(),
            });
        }
        let length = usize::try_from(u32::from_le_bytes(
            data[cursor..length_end]
                .try_into()
                .expect("FLAC comment length"),
        ))
        .map_err(|_| MetraError::ResourceLimitExceeded {
            resource: "FLAC Vorbis comment".to_owned(),
            limit: limits.max_value_bytes,
        })?;
        cursor = length_end;
        let end = cursor
            .checked_add(length)
            .ok_or(MetraError::InvalidOffset {
                context: "FLAC comment value".to_owned(),
                offset: cursor as u64,
            })?;
        if end > data.len() {
            return Err(MetraError::UnexpectedEof {
                context: "FLAC VORBIS_COMMENT value".to_owned(),
            });
        }
        let raw = &data[cursor..end];
        let matches = action.is_some_and(|action| {
            let Some(separator) = raw.iter().position(|byte| *byte == b'=') else {
                return false;
            };
            raw[..separator].eq_ignore_ascii_case(action_key(action))
        });
        if matches {
            found = true;
            if let Some(CommentAction::Set { key, value }) = action
                && !comments.iter().any(|comment: &Vec<u8>| {
                    comment
                        .split(|byte| *byte == b'=')
                        .next()
                        .is_some_and(|item| item.eq_ignore_ascii_case(key))
                })
            {
                comments.push(make_comment(key, value));
            }
        } else {
            comments.push(raw.to_vec());
        }
        cursor = end;
    }
    if cursor != data.len() {
        return Err(MetraError::InvalidTag {
            context: "FLAC VORBIS_COMMENT".to_owned(),
            message: "trailing bytes follow the declared comments".to_owned(),
        });
    }
    if let Some(CommentAction::Set { key, value }) = action
        && !found
    {
        comments.push(make_comment(key, value));
        found = true;
    }
    if matches!(action, Some(CommentAction::Delete { .. })) && !found {
        return Ok((data.to_vec(), false));
    }
    let mut output = Vec::new();
    output.extend_from_slice(
        &u32::try_from(vendor_length)
            .unwrap_or(u32::MAX)
            .to_le_bytes(),
    );
    output.extend_from_slice(&data[4..vendor_end]);
    output.extend_from_slice(
        &u32::try_from(comments.len())
            .map_err(|_| MetraError::WriteFailure {
                message: "FLAC comment count overflowed".to_owned(),
            })?
            .to_le_bytes(),
    );
    for comment in comments {
        append_raw_comment(&mut output, &comment)?;
    }
    if output.len() > limits.max_metadata_bytes || output.len() > 0xFF_FFFF {
        return Err(MetraError::ResourceLimitExceeded {
            resource: "FLAC VORBIS_COMMENT block".to_owned(),
            limit: limits.max_metadata_bytes.min(0xFF_FFFF),
        });
    }
    Ok((output, found))
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

fn append_raw_comment(output: &mut Vec<u8>, comment: &[u8]) -> Result<()> {
    let length = u32::try_from(comment.len()).map_err(|_| MetraError::WriteFailure {
        message: "FLAC comment exceeds the 32-bit size limit".to_owned(),
    })?;
    output.extend_from_slice(&length.to_le_bytes());
    output.extend_from_slice(comment);
    Ok(())
}

fn vorbis_block_with_comment(key: &[u8], value: &[u8]) -> Result<Vec<u8>> {
    let mut data = Vec::new();
    data.extend_from_slice(&0_u32.to_le_bytes());
    data.extend_from_slice(&1_u32.to_le_bytes());
    append_raw_comment(&mut data, &make_comment(key, value))?;
    Ok(data)
}

fn write_block<W: Write>(writer: &mut W, is_last: bool, block_type: u8, data: &[u8]) -> Result<()> {
    let length = u32::try_from(data.len()).map_err(|_| MetraError::WriteFailure {
        message: "FLAC metadata block exceeds the 24-bit length limit".to_owned(),
    })?;
    if length > 0xFF_FFFF {
        return Err(MetraError::WriteFailure {
            message: "FLAC metadata block exceeds the 24-bit length limit".to_owned(),
        });
    }
    let header = [
        if is_last {
            0x80 | block_type
        } else {
            block_type
        },
        (length >> 16) as u8,
        (length >> 8) as u8,
        length as u8,
    ];
    write_all(writer, &header)?;
    write_all(writer, data)
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
        .unwrap_or("metadata.flac");
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
        FileInfo::new("editable.flac".into(), size as u64, FileFormat::Flac)
    }

    fn block(last: bool, kind: u8, data: &[u8]) -> Vec<u8> {
        let length = u32::try_from(data.len()).expect("fixture block should fit");
        let mut result = vec![if last { 0x80 | kind } else { kind }];
        result.extend_from_slice(&length.to_be_bytes()[1..]);
        result.extend_from_slice(data);
        result
    }

    fn streaminfo() -> Vec<u8> {
        let mut data = vec![0_u8; 34];
        data[0..2].copy_from_slice(&4096_u16.to_be_bytes());
        data[2..4].copy_from_slice(&4096_u16.to_be_bytes());
        let packed = (44_100_u64 << 44) | (1_u64 << 41) | (15_u64 << 36) | 88_200;
        data[10..18].copy_from_slice(&packed.to_be_bytes());
        data
    }

    fn vorbis_comments(title: &str) -> Vec<u8> {
        let vendor = b"Metra";
        let comments = [
            format!("TITLE={title}"),
            "ARTIST=Metra test artist".to_owned(),
        ];
        let mut data = (vendor.len() as u32).to_le_bytes().to_vec();
        data.extend_from_slice(vendor);
        data.extend_from_slice(&(comments.len() as u32).to_le_bytes());
        for comment in comments {
            data.extend_from_slice(&(comment.len() as u32).to_le_bytes());
            data.extend_from_slice(comment.as_bytes());
        }
        data
    }

    fn flac_with_comments(title: &str) -> Vec<u8> {
        let mut bytes = b"fLaC".to_vec();
        bytes.extend(block(false, 0, &streaminfo()));
        bytes.extend(block(false, 2, &[0xAA, 0xBB]));
        bytes.extend(block(true, 4, &vorbis_comments(title)));
        bytes.extend_from_slice(&[1, 2, 3, 4]);
        bytes
    }

    #[test]
    fn replaces_vorbis_comment_and_preserves_other_blocks_and_frames() {
        let bytes = flac_with_comments("before");
        let output = rewrite_flac_to_vec(
            &bytes,
            info(bytes.len()),
            ParseLimits::default(),
            &[FlacEdit::SetComment {
                key: "Title".to_owned(),
                value: "after".to_owned(),
            }],
        )
        .unwrap();
        let metadata = read_flac(
            &mut Cursor::new(output.clone()),
            info(output.len()),
            ParseLimits::default(),
        )
        .unwrap();
        assert_eq!(
            metadata.find("FLAC:Title").unwrap().display_value(),
            "after"
        );
        assert_eq!(
            metadata.find("FLAC:Artist").unwrap().display_value(),
            "Metra test artist"
        );
        assert!(
            output
                .windows(6)
                .any(|window| { window == [2, 0, 0, 2, 0xAA, 0xBB] })
        );
        assert!(output.ends_with(&[1, 2, 3, 4]));
    }

    #[test]
    fn deletes_and_inserts_vorbis_comments() {
        let bytes = flac_with_comments("before");
        let deleted = rewrite_flac_to_vec(
            &bytes,
            info(bytes.len()),
            ParseLimits::default(),
            &[FlacEdit::DeleteComment {
                key: "TITLE".to_owned(),
            }],
        )
        .unwrap();
        let deleted_metadata = read_flac(
            &mut Cursor::new(deleted.clone()),
            info(deleted.len()),
            ParseLimits::default(),
        )
        .unwrap();
        assert!(deleted_metadata.find("FLAC:Title").is_none());
        assert_eq!(
            deleted_metadata
                .find("FLAC:Artist")
                .unwrap()
                .display_value(),
            "Metra test artist"
        );

        let mut without_comments = b"fLaC".to_vec();
        without_comments.extend(block(false, 0, &streaminfo()));
        without_comments.extend(block(true, 1, &[0, 0, 0, 0]));
        without_comments.extend_from_slice(&[5, 6, 7]);
        let inserted = rewrite_flac_to_vec(
            &without_comments,
            info(without_comments.len()),
            ParseLimits::default(),
            &[FlacEdit::SetComment {
                key: "Artist".to_owned(),
                value: "inserted".to_owned(),
            }],
        )
        .unwrap();
        let inserted_metadata = read_flac(
            &mut Cursor::new(inserted.clone()),
            info(inserted.len()),
            ParseLimits::default(),
        )
        .unwrap();
        assert_eq!(
            inserted_metadata
                .find("FLAC:Artist")
                .unwrap()
                .display_value(),
            "inserted"
        );
        assert!(inserted.ends_with(&[5, 6, 7]));
    }

    #[test]
    fn path_rewrite_validates_before_atomic_replace() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("metra-flac-rewrite-{unique}"));
        fs::create_dir(&directory).unwrap();
        let path = directory.join("atomic.flac");
        fs::write(&path, flac_with_comments("before")).unwrap();

        rewrite_flac_path(
            &path,
            ParseLimits::default(),
            &[FlacEdit::SetComment {
                key: "Title".to_owned(),
                value: "atomic".to_owned(),
            }],
        )
        .unwrap();
        let rewritten = fs::read(&path).unwrap();
        let rewritten_size = rewritten.len() as u64;
        let metadata = read_flac(
            &mut Cursor::new(rewritten),
            FileInfo::new("atomic.flac".into(), rewritten_size, FileFormat::Flac),
            ParseLimits::default(),
        )
        .unwrap();
        assert_eq!(
            metadata.find("FLAC:Title").unwrap().display_value(),
            "atomic"
        );
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn rejects_invalid_vorbis_comment_values() {
        let bytes = flac_with_comments("before");
        let error = rewrite_flac_to_vec(
            &bytes,
            info(bytes.len()),
            ParseLimits::default(),
            &[FlacEdit::SetComment {
                key: "Title".to_owned(),
                value: "bad\0value".to_owned(),
            }],
        )
        .unwrap_err();
        assert!(error.to_string().contains("NUL"));
    }
}
