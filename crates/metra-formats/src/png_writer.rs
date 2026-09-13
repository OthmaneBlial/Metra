use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use metra_core::{FileFormat, FileInfo, MetraError, ParseLimits, Result};

use crate::png::{PNG_SIGNATURE, crc32, read_png};

/// Lossless PNG text edits for uncompressed `tEXt` chunks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PngEdit {
    SetText { keyword: String, value: String },
    DeleteText { keyword: String },
}

pub fn rewrite_png<R: Read + Seek, W: Write>(
    reader: &mut R,
    writer: &mut W,
    file_info: FileInfo,
    limits: ParseLimits,
    edits: &[PngEdit],
) -> Result<()> {
    read_png(reader, file_info.clone(), limits)?;
    reader
        .seek(SeekFrom::Start(0))
        .map_err(|source| write_io_error(&file_info.path, source))?;
    rewrite_png_stream(reader, writer, &file_info.path, limits, edits)
}

pub fn rewrite_png_to_vec(
    bytes: &[u8],
    file_info: FileInfo,
    limits: ParseLimits,
    edits: &[PngEdit],
) -> Result<Vec<u8>> {
    let mut reader = std::io::Cursor::new(bytes);
    let mut output = Vec::new();
    rewrite_png(&mut reader, &mut output, file_info.clone(), limits, edits)?;
    let validation_info = FileInfo::new(file_info.path, output.len() as u64, FileFormat::Png);
    read_png(
        &mut std::io::Cursor::new(output.as_slice()),
        validation_info,
        limits,
    )?;
    Ok(output)
}

pub fn rewrite_png_path(
    path: impl AsRef<Path>,
    limits: ParseLimits,
    edits: &[PngEdit],
) -> Result<()> {
    let path = path.as_ref().to_path_buf();
    let source_metadata = fs::metadata(&path).map_err(|source| MetraError::Io {
        path: path.clone(),
        source,
    })?;
    let file_info = FileInfo::new(path.clone(), source_metadata.len(), FileFormat::Png);
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
        rewrite_png(&mut input, &mut output, file_info.clone(), limits, edits)?;
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
        let written_info = FileInfo::new(temp_path.clone(), written_size, FileFormat::Png);
        read_png(&mut validation, written_info, limits)?;
        fs::set_permissions(&temp_path, source_metadata.permissions()).map_err(|source| {
            MetraError::WriteFailure {
                message: format!(
                    "cannot preserve permissions on {}: {source}",
                    temp_path.display()
                ),
            }
        })?;
        fs::rename(&temp_path, &path).map_err(|source| MetraError::WriteFailure {
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
enum TextAction {
    Set { keyword: Vec<u8>, value: Vec<u8> },
    Delete { keyword: Vec<u8> },
}

fn rewrite_png_stream<R: Read, W: Write>(
    reader: &mut R,
    writer: &mut W,
    path: &Path,
    limits: ParseLimits,
    edits: &[PngEdit],
) -> Result<()> {
    let action = text_action(edits, limits)?;
    let mut signature = [0_u8; 8];
    read_exact(reader, &mut signature, path)?;
    if &signature != PNG_SIGNATURE {
        return Err(MetraError::InvalidHeader {
            context: "PNG".to_owned(),
            message: "missing PNG signature".to_owned(),
        });
    }
    write_all(writer, PNG_SIGNATURE)?;
    let mut chunks = 0_usize;
    let mut text_bytes = 0_usize;
    let mut inserted = false;

    loop {
        if chunks >= limits.max_jpeg_segments {
            return Err(MetraError::ResourceLimitExceeded {
                resource: "PNG chunks during rewrite".to_owned(),
                limit: limits.max_jpeg_segments,
            });
        }
        let mut header = [0_u8; 8];
        read_exact(reader, &mut header, path)?;
        let data_length = u64::from(u32::from_be_bytes(
            header[..4].try_into().expect("PNG length is four bytes"),
        ));
        let chunk_type: [u8; 4] = header[4..8]
            .try_into()
            .expect("PNG chunk type is four bytes");
        if &chunk_type == b"tEXt" {
            let length =
                usize::try_from(data_length).map_err(|_| MetraError::ResourceLimitExceeded {
                    resource: "PNG tEXt chunk during rewrite".to_owned(),
                    limit: limits.max_metadata_bytes,
                })?;
            text_bytes = text_bytes.saturating_add(length);
            if text_bytes > limits.max_metadata_bytes {
                return Err(MetraError::ResourceLimitExceeded {
                    resource: "PNG text metadata during rewrite".to_owned(),
                    limit: limits.max_metadata_bytes,
                });
            }
            let mut data = vec![0_u8; length];
            read_exact(reader, &mut data, path)?;
            let mut crc = [0_u8; 4];
            read_exact(reader, &mut crc, path)?;
            let keyword = text_keyword(&data);
            if let Some(action) = action.as_ref()
                && keyword.is_some_and(|keyword| action_matches(action, keyword))
            {
                match action {
                    TextAction::Set { keyword, value } if !inserted => {
                        write_text_chunk(writer, keyword, value)?;
                        inserted = true;
                    }
                    TextAction::Set { .. } | TextAction::Delete { .. } => {}
                }
            } else {
                write_all(writer, &header)?;
                write_all(writer, &data)?;
                write_all(writer, &crc)?;
            }
        } else if &chunk_type == b"IEND" {
            if let Some(TextAction::Set { keyword, value }) = action.as_ref()
                && !inserted
            {
                write_text_chunk(writer, keyword, value)?;
            }
            write_all(writer, &header)?;
            copy_exact(reader, writer, data_length, path)?;
            copy_exact(reader, writer, 4, path)?;
            return Ok(());
        } else {
            write_all(writer, &header)?;
            copy_exact(reader, writer, data_length, path)?;
            copy_exact(reader, writer, 4, path)?;
        }
        chunks += 1;
    }
}

fn text_action(edits: &[PngEdit], limits: ParseLimits) -> Result<Option<TextAction>> {
    let mut action = None;
    for edit in edits {
        match edit {
            PngEdit::SetText { keyword, value } => {
                let keyword = validate_keyword(keyword)?;
                let value = validate_value(value)?;
                let data_length = keyword
                    .len()
                    .checked_add(1)
                    .and_then(|length| length.checked_add(value.len()))
                    .ok_or_else(|| MetraError::WriteFailure {
                        message: "PNG text chunk length overflowed".to_owned(),
                    })?;
                if data_length > limits.max_metadata_bytes {
                    return Err(MetraError::ResourceLimitExceeded {
                        resource: "PNG text metadata".to_owned(),
                        limit: limits.max_metadata_bytes,
                    });
                }
                if u32::try_from(data_length).is_err() {
                    return Err(MetraError::WriteFailure {
                        message: "PNG text chunk exceeds the 32-bit length limit".to_owned(),
                    });
                }
                action = Some(TextAction::Set { keyword, value });
            }
            PngEdit::DeleteText { keyword } => {
                action = Some(TextAction::Delete {
                    keyword: validate_keyword(keyword)?,
                });
            }
        }
    }
    Ok(action)
}

fn validate_keyword(keyword: &str) -> Result<Vec<u8>> {
    if keyword.is_empty() || keyword.len() > 79 || !keyword.is_ascii() || keyword.contains('\0') {
        return Err(MetraError::WriteFailure {
            message: "PNG tEXt keywords must contain 1-79 ASCII bytes without NUL".to_owned(),
        });
    }
    Ok(keyword.as_bytes().to_vec())
}

fn validate_value(value: &str) -> Result<Vec<u8>> {
    if value.contains('\0') {
        return Err(MetraError::WriteFailure {
            message: "PNG tEXt values cannot contain NUL".to_owned(),
        });
    }
    Ok(value.as_bytes().to_vec())
}

fn text_keyword(data: &[u8]) -> Option<&[u8]> {
    let separator = data.iter().position(|byte| *byte == 0)?;
    Some(&data[..separator])
}

fn action_matches(action: &TextAction, keyword: &[u8]) -> bool {
    match action {
        TextAction::Set {
            keyword: target, ..
        }
        | TextAction::Delete { keyword: target } => target == keyword,
    }
}

fn write_text_chunk<W: Write>(writer: &mut W, keyword: &[u8], value: &[u8]) -> Result<()> {
    let data_length = keyword.len() + 1 + value.len();
    let length = u32::try_from(data_length).map_err(|_| MetraError::WriteFailure {
        message: "PNG text chunk exceeds the 32-bit length limit".to_owned(),
    })?;
    let chunk_type = *b"tEXt";
    write_all(writer, &length.to_be_bytes())?;
    write_all(writer, &chunk_type)?;
    write_all(writer, keyword)?;
    write_all(writer, &[0])?;
    write_all(writer, value)?;
    write_all(
        writer,
        &crc32(&chunk_type, &[keyword, &[0], value].concat()).to_be_bytes(),
    )
}

fn read_exact<R: Read>(reader: &mut R, bytes: &mut [u8], path: &Path) -> Result<()> {
    reader
        .read_exact(bytes)
        .map_err(|source| write_io_error(path, source))
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
            .map_err(|source| write_io_error(path, source))?;
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

fn write_io_error(path: &Path, source: std::io::Error) -> MetraError {
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
        .unwrap_or("metadata.png");
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
        FileInfo::new("editable.png".into(), size as u64, FileFormat::Png)
    }

    fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&(data.len() as u32).to_be_bytes());
        bytes.extend_from_slice(kind);
        bytes.extend_from_slice(data);
        bytes.extend_from_slice(&crc32(kind, data).to_be_bytes());
        bytes
    }

    fn png_with_text() -> Vec<u8> {
        let mut bytes = PNG_SIGNATURE.to_vec();
        bytes.extend_from_slice(&chunk(b"IHDR", &[0; 13]));
        bytes.extend_from_slice(&chunk(b"tEXt", b"Comment\0before"));
        bytes.extend_from_slice(&chunk(b"IDAT", &[1, 2, 3, 4]));
        bytes.extend_from_slice(&chunk(b"IEND", &[]));
        bytes
    }

    #[test]
    fn replaces_text_and_preserves_image_chunks() {
        let bytes = png_with_text();
        let metadata = info(bytes.len());
        let output = rewrite_png_to_vec(
            &bytes,
            metadata.clone(),
            ParseLimits::default(),
            &[PngEdit::SetText {
                keyword: "Comment".to_owned(),
                value: "after".to_owned(),
            }],
        )
        .unwrap();
        assert!(
            output
                .windows(12)
                .any(|window| window == [0, 0, 0, 4, b'I', b'D', b'A', b'T', 1, 2, 3, 4])
        );
        let parsed = read_png(
            &mut Cursor::new(output.clone()),
            FileInfo::new("editable.png".into(), output.len() as u64, FileFormat::Png),
            ParseLimits::default(),
        )
        .unwrap();
        assert_eq!(
            parsed.find("PNG:Text:Comment").unwrap().display_value(),
            "after"
        );
    }

    #[test]
    fn deletes_or_inserts_text_chunks() {
        let bytes = png_with_text();
        let deleted = rewrite_png_to_vec(
            &bytes,
            info(bytes.len()),
            ParseLimits::default(),
            &[PngEdit::DeleteText {
                keyword: "Comment".to_owned(),
            }],
        )
        .unwrap();
        let deleted_metadata = read_png(
            &mut Cursor::new(deleted.clone()),
            FileInfo::new("editable.png".into(), deleted.len() as u64, FileFormat::Png),
            ParseLimits::default(),
        )
        .unwrap();
        assert!(deleted_metadata.find("PNG:Text:Comment").is_none());

        let mut without_text = PNG_SIGNATURE.to_vec();
        without_text.extend_from_slice(&chunk(b"IHDR", &[0; 13]));
        without_text.extend_from_slice(&chunk(b"IEND", &[]));
        let inserted = rewrite_png_to_vec(
            &without_text,
            info(without_text.len()),
            ParseLimits::default(),
            &[PngEdit::SetText {
                keyword: "Title".to_owned(),
                value: "inserted".to_owned(),
            }],
        )
        .unwrap();
        let inserted_metadata = read_png(
            &mut Cursor::new(inserted.clone()),
            FileInfo::new(
                "editable.png".into(),
                inserted.len() as u64,
                FileFormat::Png,
            ),
            ParseLimits::default(),
        )
        .unwrap();
        assert_eq!(
            inserted_metadata
                .find("PNG:Text:Title")
                .unwrap()
                .display_value(),
            "inserted"
        );
    }

    #[test]
    fn rejects_invalid_text_values() {
        let bytes = png_with_text();
        let error = rewrite_png_to_vec(
            &bytes,
            info(bytes.len()),
            ParseLimits::default(),
            &[PngEdit::SetText {
                keyword: "bad\0keyword".to_owned(),
                value: "value".to_owned(),
            }],
        )
        .unwrap_err();
        assert!(error.to_string().contains("keywords"));
    }
}
