use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use metra_core::{FileFormat, FileInfo, MetraError, ParseLimits, Result};

use crate::atomic::atomic_replace;
use crate::gif::read_gif;

/// Lossless GIF comment-extension edits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GifEdit {
    SetComment(String),
    DeleteComments,
}

pub fn rewrite_gif<R: Read + Seek, W: Write>(
    reader: &mut R,
    writer: &mut W,
    file_info: FileInfo,
    limits: ParseLimits,
    edits: &[GifEdit],
) -> Result<()> {
    read_gif(reader, file_info.clone(), limits)?;
    reader
        .seek(SeekFrom::Start(0))
        .map_err(|source| io_error(&file_info.path, source))?;
    rewrite_gif_stream(reader, writer, &file_info.path, limits, edits)
}

pub fn rewrite_gif_to_vec(
    bytes: &[u8],
    file_info: FileInfo,
    limits: ParseLimits,
    edits: &[GifEdit],
) -> Result<Vec<u8>> {
    let mut reader = std::io::Cursor::new(bytes);
    let mut output = Vec::new();
    rewrite_gif(&mut reader, &mut output, file_info.clone(), limits, edits)?;
    let output_size = output.len() as u64;
    read_gif(
        &mut std::io::Cursor::new(output.as_slice()),
        FileInfo::new(file_info.path, output_size, FileFormat::Gif),
        limits,
    )?;
    Ok(output)
}

pub fn rewrite_gif_path(
    path: impl AsRef<Path>,
    limits: ParseLimits,
    edits: &[GifEdit],
) -> Result<()> {
    let path = path.as_ref().to_path_buf();
    let source_metadata = fs::metadata(&path).map_err(|source| MetraError::Io {
        path: path.clone(),
        source,
    })?;
    let file_info = FileInfo::new(path.clone(), source_metadata.len(), FileFormat::Gif);
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
        rewrite_gif(&mut input, &mut output, file_info.clone(), limits, edits)?;
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
        read_gif(
            &mut validation,
            FileInfo::new(temp_path.clone(), written_size, FileFormat::Gif),
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

fn rewrite_gif_stream<R: Read, W: Write>(
    reader: &mut R,
    writer: &mut W,
    path: &Path,
    limits: ParseLimits,
    edits: &[GifEdit],
) -> Result<()> {
    let action = comment_action(edits, limits)?;
    let mut header = [0_u8; 6];
    read_exact(reader, &mut header, path)?;
    if &header != b"GIF87a" && &header != b"GIF89a" {
        return Err(MetraError::InvalidHeader {
            context: "GIF".to_owned(),
            message: "expected GIF87a or GIF89a header".to_owned(),
        });
    }
    write_all(writer, &header)?;

    let mut descriptor = [0_u8; 7];
    read_exact(reader, &mut descriptor, path)?;
    write_all(writer, &descriptor)?;
    let mut metadata_bytes = 0_usize;
    copy_color_table(reader, writer, descriptor[4], path)?;

    let mut block_count = 0_usize;
    let mut inserted = false;
    loop {
        if block_count >= limits.max_jpeg_segments {
            return Err(MetraError::ResourceLimitExceeded {
                resource: "GIF blocks during rewrite".to_owned(),
                limit: limits.max_jpeg_segments,
            });
        }
        let mut introducer = [0_u8; 1];
        read_exact(reader, &mut introducer, path)?;
        match introducer[0] {
            0x3B => {
                if let Some(GifAction::Set(comment)) = action.as_ref()
                    && !inserted
                {
                    write_comment(writer, comment)?;
                }
                write_all(writer, &introducer)?;
                copy_remainder(reader, writer, path)?;
                return Ok(());
            }
            0x2C => {
                write_all(writer, &introducer)?;
                copy_image(reader, writer, path)?;
            }
            0x21 => {
                let mut label = [0_u8; 1];
                read_exact(reader, &mut label, path)?;
                if label[0] == 0xFE
                    && let Some(action) = action.as_ref()
                {
                    consume_sub_blocks(reader, path, &mut metadata_bytes, limits)?;
                    match action {
                        GifAction::Set(comment) if !inserted => {
                            write_comment(writer, comment)?;
                            inserted = true;
                        }
                        GifAction::Set(_) | GifAction::Delete => {}
                    }
                } else {
                    write_all(writer, &introducer)?;
                    write_all(writer, &label)?;
                    copy_sub_blocks(reader, writer, path, &mut metadata_bytes, limits)?;
                }
            }
            other => {
                return Err(MetraError::InvalidTag {
                    context: "GIF block".to_owned(),
                    message: format!("unknown block introducer 0x{other:02X}"),
                });
            }
        }
        block_count += 1;
    }
}

#[derive(Debug)]
enum GifAction {
    Set(Vec<u8>),
    Delete,
}

fn comment_action(edits: &[GifEdit], limits: ParseLimits) -> Result<Option<GifAction>> {
    let mut action = None;
    for edit in edits {
        action = Some(match edit {
            GifEdit::SetComment(comment) => {
                let bytes = comment.as_bytes().to_vec();
                if bytes.len() > limits.max_value_bytes {
                    return Err(MetraError::ResourceLimitExceeded {
                        resource: "GIF comment".to_owned(),
                        limit: limits.max_value_bytes,
                    });
                }
                GifAction::Set(bytes)
            }
            GifEdit::DeleteComments => GifAction::Delete,
        });
    }
    Ok(action)
}

fn copy_color_table<R: Read, W: Write>(
    reader: &mut R,
    writer: &mut W,
    packed: u8,
    path: &Path,
) -> Result<()> {
    if packed & 0x80 == 0 {
        return Ok(());
    }
    let entries =
        1_usize
            .checked_shl(u32::from((packed & 0x07) + 1))
            .ok_or(MetraError::InvalidOffset {
                context: "GIF global color table".to_owned(),
                offset: u64::from(packed),
            })?;
    let length = entries.checked_mul(3).ok_or(MetraError::InvalidOffset {
        context: "GIF global color table".to_owned(),
        offset: entries as u64,
    })?;
    copy_exact(reader, writer, length as u64, path)
}

fn copy_image<R: Read, W: Write>(reader: &mut R, writer: &mut W, path: &Path) -> Result<()> {
    let mut descriptor = [0_u8; 9];
    read_exact(reader, &mut descriptor, path)?;
    write_all(writer, &descriptor)?;
    copy_color_table(reader, writer, descriptor[8], path)?;

    let mut code_size = [0_u8; 1];
    read_exact(reader, &mut code_size, path)?;
    if code_size[0] == 0 {
        return Err(MetraError::InvalidTag {
            context: "GIF image data".to_owned(),
            message: "LZW minimum code size cannot be zero".to_owned(),
        });
    }
    write_all(writer, &code_size)?;
    copy_data_sub_blocks(reader, writer, path)
}

fn copy_sub_blocks<R: Read, W: Write>(
    reader: &mut R,
    writer: &mut W,
    path: &Path,
    metadata_bytes: &mut usize,
    limits: ParseLimits,
) -> Result<()> {
    loop {
        let mut size = [0_u8; 1];
        read_exact(reader, &mut size, path)?;
        write_all(writer, &size)?;
        let length = usize::from(size[0]);
        if length == 0 {
            return Ok(());
        }
        *metadata_bytes =
            metadata_bytes
                .checked_add(length)
                .ok_or(MetraError::ResourceLimitExceeded {
                    resource: "GIF extension metadata".to_owned(),
                    limit: limits.max_metadata_bytes,
                })?;
        if *metadata_bytes > limits.max_metadata_bytes {
            return Err(MetraError::ResourceLimitExceeded {
                resource: "GIF extension metadata".to_owned(),
                limit: limits.max_metadata_bytes,
            });
        }
        copy_exact(reader, writer, length as u64, path)?;
    }
}

fn consume_sub_blocks<R: Read>(
    reader: &mut R,
    path: &Path,
    metadata_bytes: &mut usize,
    limits: ParseLimits,
) -> Result<()> {
    loop {
        let mut size = [0_u8; 1];
        read_exact(reader, &mut size, path)?;
        let length = usize::from(size[0]);
        if length == 0 {
            return Ok(());
        }
        *metadata_bytes =
            metadata_bytes
                .checked_add(length)
                .ok_or(MetraError::ResourceLimitExceeded {
                    resource: "GIF extension metadata".to_owned(),
                    limit: limits.max_metadata_bytes,
                })?;
        if *metadata_bytes > limits.max_metadata_bytes {
            return Err(MetraError::ResourceLimitExceeded {
                resource: "GIF extension metadata".to_owned(),
                limit: limits.max_metadata_bytes,
            });
        }
        discard_exact(reader, length as u64, path)?;
    }
}

fn copy_data_sub_blocks<R: Read, W: Write>(
    reader: &mut R,
    writer: &mut W,
    path: &Path,
) -> Result<()> {
    loop {
        let mut size = [0_u8; 1];
        read_exact(reader, &mut size, path)?;
        write_all(writer, &size)?;
        let length = usize::from(size[0]);
        if length == 0 {
            return Ok(());
        }
        copy_exact(reader, writer, length as u64, path)?;
    }
}

fn write_comment<W: Write>(writer: &mut W, comment: &[u8]) -> Result<()> {
    write_all(writer, &[0x21, 0xFE])?;
    for chunk in comment.chunks(255) {
        write_all(writer, &[chunk.len() as u8])?;
        write_all(writer, chunk)?;
    }
    write_all(writer, &[0])
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
        read_exact(reader, &mut buffer[..requested], path)?;
        write_all(writer, &buffer[..requested])?;
        length -= requested as u64;
    }
    Ok(())
}

fn discard_exact<R: Read>(reader: &mut R, mut length: u64, path: &Path) -> Result<()> {
    let mut buffer = [0_u8; 64 * 1024];
    while length > 0 {
        let requested = usize::try_from(length)
            .unwrap_or(buffer.len())
            .min(buffer.len());
        read_exact(reader, &mut buffer[..requested], path)?;
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

fn read_exact<R: Read>(reader: &mut R, bytes: &mut [u8], path: &Path) -> Result<()> {
    reader
        .read_exact(bytes)
        .map_err(|source| io_error(path, source))
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
        .unwrap_or("metadata.gif");
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
        FileInfo::new("editable.gif".into(), size as u64, FileFormat::Gif)
    }

    fn gif_with_comment(comment: &[u8]) -> Vec<u8> {
        let mut bytes = b"GIF89a".to_vec();
        bytes.extend_from_slice(&2_u16.to_le_bytes());
        bytes.extend_from_slice(&2_u16.to_le_bytes());
        bytes.extend_from_slice(&[0, 0, 0]);
        bytes.extend_from_slice(&[0x21, 0xFE, comment.len() as u8]);
        bytes.extend_from_slice(comment);
        bytes.extend_from_slice(&[0]);
        bytes.extend_from_slice(&[0x2C]);
        bytes.extend_from_slice(&[0, 0, 0, 0, 2, 0, 2, 0, 0]);
        bytes.extend_from_slice(&[2, 1, 0x44, 0]);
        bytes.extend_from_slice(&[0x3B]);
        bytes
    }

    #[test]
    fn replaces_comment_and_preserves_image_data() {
        let bytes = gif_with_comment(b"before");
        let output = rewrite_gif_to_vec(
            &bytes,
            info(bytes.len()),
            ParseLimits::default(),
            &[GifEdit::SetComment("after".to_owned())],
        )
        .unwrap();
        let metadata = read_gif(
            &mut Cursor::new(output.clone()),
            info(output.len()),
            ParseLimits::default(),
        )
        .unwrap();
        assert_eq!(
            metadata.find("GIF:Comment").unwrap().display_value(),
            "after"
        );
        assert!(
            output
                .windows(14)
                .any(|window| { window == [0x2C, 0, 0, 0, 0, 2, 0, 2, 0, 0, 2, 1, 0x44, 0] })
        );
        assert!(output.ends_with(&[0x3B]));
    }

    #[test]
    fn deletes_and_inserts_comments() {
        let bytes = gif_with_comment(b"before");
        let deleted = rewrite_gif_to_vec(
            &bytes,
            info(bytes.len()),
            ParseLimits::default(),
            &[GifEdit::DeleteComments],
        )
        .unwrap();
        let deleted_metadata = read_gif(
            &mut Cursor::new(deleted.clone()),
            info(deleted.len()),
            ParseLimits::default(),
        )
        .unwrap();
        assert!(deleted_metadata.find("GIF:Comment").is_none());

        let mut without_comment = b"GIF89a".to_vec();
        without_comment.extend_from_slice(&[2, 0, 2, 0, 0, 0, 0]);
        without_comment.push(0x3B);
        let inserted = rewrite_gif_to_vec(
            &without_comment,
            info(without_comment.len()),
            ParseLimits::default(),
            &[GifEdit::SetComment("inserted".to_owned())],
        )
        .unwrap();
        let inserted_metadata = read_gif(
            &mut Cursor::new(inserted),
            info(without_comment.len() + 12),
            ParseLimits::default(),
        )
        .unwrap();
        assert_eq!(
            inserted_metadata
                .find("GIF:Comment")
                .unwrap()
                .display_value(),
            "inserted"
        );
    }

    #[test]
    fn path_rewrite_validates_before_atomic_replace() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("metra-gif-rewrite-{unique}"));
        fs::create_dir(&directory).unwrap();
        let path = directory.join("atomic.gif");
        fs::write(&path, gif_with_comment(b"before")).unwrap();

        rewrite_gif_path(
            &path,
            ParseLimits::default(),
            &[GifEdit::SetComment("atomic".to_owned())],
        )
        .unwrap();
        let rewritten = fs::read(&path).unwrap();
        let metadata = read_gif(
            &mut Cursor::new(rewritten.clone()),
            info(rewritten.len()),
            ParseLimits::default(),
        )
        .unwrap();
        assert_eq!(
            metadata.find("GIF:Comment").unwrap().display_value(),
            "atomic"
        );
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn rejects_comments_over_the_value_budget() {
        let bytes = gif_with_comment(b"before");
        let limits = ParseLimits {
            max_value_bytes: 3,
            ..ParseLimits::default()
        };
        let error = rewrite_gif_to_vec(
            &bytes,
            info(bytes.len()),
            limits,
            &[GifEdit::SetComment("longer".to_owned())],
        )
        .unwrap_err();
        assert!(error.to_string().contains("GIF comment"));
    }
}
