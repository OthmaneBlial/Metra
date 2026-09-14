use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use metra_core::{FileFormat, FileInfo, Metadata, MetraError, ParseLimits, Result, TagValue};

use crate::atomic::atomic_replace;
use crate::avi::read_avi;

/// Lossless edits for existing AVI `LIST/INFO` string chunks.
///
/// AVI media chunks and RIFF layout are copied unchanged. A replacement is
/// written into the existing INFO payload with a NUL terminator when capacity
/// allows and zero padding, so no chunk size or media offset changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AviEdit {
    SetInfo { name: String, value: String },
    DeleteInfo { name: String },
}

pub fn rewrite_avi<R: Read + Seek, W: Write + Seek>(
    reader: &mut R,
    writer: &mut W,
    file_info: FileInfo,
    limits: ParseLimits,
    edits: &[AviEdit],
) -> Result<()> {
    if file_info.format != FileFormat::Avi {
        return Err(MetraError::WriteFailure {
            message: format!("AVI writer cannot edit {}", file_info.format),
        });
    }
    let metadata = read_avi(reader, file_info.clone(), limits)?;
    let patches = collect_patches(&metadata, limits, edits)?;
    reader
        .seek(SeekFrom::Start(0))
        .map_err(|source| io_error(&file_info.path, source))?;
    rewrite_stream(reader, writer, file_info.size, &file_info.path, &patches)
}

pub fn rewrite_avi_to_vec(
    bytes: &[u8],
    file_info: FileInfo,
    limits: ParseLimits,
    edits: &[AviEdit],
) -> Result<Vec<u8>> {
    let mut reader = std::io::Cursor::new(bytes);
    let mut output = std::io::Cursor::new(Vec::new());
    rewrite_avi(&mut reader, &mut output, file_info.clone(), limits, edits)?;
    let output = output.into_inner();
    read_avi(
        &mut std::io::Cursor::new(output.as_slice()),
        FileInfo::new(file_info.path, output.len() as u64, FileFormat::Avi),
        limits,
    )?;
    Ok(output)
}

pub fn rewrite_avi_path(
    path: impl AsRef<Path>,
    limits: ParseLimits,
    edits: &[AviEdit],
) -> Result<()> {
    let path = path.as_ref().to_path_buf();
    let source_metadata = fs::metadata(&path).map_err(|source| MetraError::Io {
        path: path.clone(),
        source,
    })?;
    let file_info = FileInfo::new(path.clone(), source_metadata.len(), FileFormat::Avi);
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
        rewrite_avi(&mut input, &mut output, file_info.clone(), limits, edits)?;
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
        read_avi(
            &mut validation,
            FileInfo::new(temp_path.clone(), written_size, FileFormat::Avi),
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

#[derive(Debug, Clone)]
struct Patch {
    offset: u64,
    span: u64,
    bytes: Vec<u8>,
}

fn collect_patches(
    metadata: &Metadata,
    limits: ParseLimits,
    edits: &[AviEdit],
) -> Result<Vec<Patch>> {
    let mut patches = Vec::with_capacity(edits.len());
    for edit in edits {
        let (name, value) = match edit {
            AviEdit::SetInfo { name, value } => (name, value.as_str()),
            AviEdit::DeleteInfo { name } => (name, ""),
        };
        if !is_writable_info_name(name) {
            return Err(MetraError::WriteFailure {
                message: format!("AVI INFO field {name} is not writable"),
            });
        }
        if value.contains('\0') {
            return Err(MetraError::WriteFailure {
                message: format!("AVI INFO value for {name} cannot contain NUL"),
            });
        }
        let key = format!("AVI:{name}");
        let tag = match metadata.find_all(&key).as_slice() {
            [tag] => *tag,
            [] => {
                return Err(MetraError::WriteFailure {
                    message: format!("AVI INFO field {name} does not exist"),
                });
            }
            _ => {
                return Err(MetraError::WriteFailure {
                    message: format!(
                        "AVI INFO field {name} is repeated; an unambiguous target is required"
                    ),
                });
            }
        };
        if tag.source.container != "AVI/INFO" {
            return Err(MetraError::WriteFailure {
                message: format!("AVI INFO field {name} has no INFO chunk source"),
            });
        }
        if !matches!(tag.value, TagValue::String(_)) {
            return Err(MetraError::WriteFailure {
                message: format!("AVI INFO field {name} is not a string"),
            });
        }
        let offset = tag.source.offset.ok_or_else(|| MetraError::WriteFailure {
            message: format!("AVI INFO field {name} has no source offset"),
        })?;
        let span = tag.source.length.ok_or_else(|| MetraError::WriteFailure {
            message: format!("AVI INFO field {name} has no source length"),
        })?;
        let span_usize = usize::try_from(span).map_err(|_| MetraError::ResourceLimitExceeded {
            resource: "AVI INFO value".to_owned(),
            limit: limits.max_value_bytes,
        })?;
        if span_usize > limits.max_value_bytes {
            return Err(MetraError::ResourceLimitExceeded {
                resource: "AVI INFO value".to_owned(),
                limit: limits.max_value_bytes,
            });
        }
        let value_length = value.len();
        if value_length > span_usize {
            return Err(MetraError::WriteFailure {
                message: format!(
                    "AVI INFO field {name} has {} bytes available, {} needed",
                    span_usize, value_length
                ),
            });
        }
        let mut replacement = vec![0_u8; span_usize];
        replacement[..value.len()].copy_from_slice(value.as_bytes());
        patches.push(Patch {
            offset,
            span,
            bytes: replacement,
        });
    }
    patches.sort_by_key(|patch| patch.offset);
    for pair in patches.windows(2) {
        let previous_end =
            pair[0]
                .offset
                .checked_add(pair[0].span)
                .ok_or(MetraError::InvalidOffset {
                    context: "AVI rewrite patch".to_owned(),
                    offset: pair[0].offset,
                })?;
        if previous_end > pair[1].offset {
            return Err(MetraError::WriteFailure {
                message: "AVI rewrite patches overlap".to_owned(),
            });
        }
    }
    Ok(patches)
}

fn is_writable_info_name(name: &str) -> bool {
    matches!(
        name,
        "Title"
            | "Artist"
            | "Comment"
            | "Copyright"
            | "Software"
            | "Genre"
            | "Product"
            | "Keywords"
            | "DateTime"
    )
}

fn rewrite_stream<R: Read + Seek, W: Write + Seek>(
    reader: &mut R,
    writer: &mut W,
    file_length: u64,
    path: &Path,
    patches: &[Patch],
) -> Result<()> {
    writer
        .seek(SeekFrom::Start(0))
        .map_err(|source| write_io_error(path, source))?;
    let mut cursor = 0_u64;
    for patch in patches {
        if patch.offset < cursor {
            return Err(MetraError::WriteFailure {
                message: "AVI rewrite patches are not ordered".to_owned(),
            });
        }
        copy_exact(reader, writer, patch.offset - cursor, path)?;
        let patch_end = patch
            .offset
            .checked_add(patch.span)
            .ok_or(MetraError::InvalidOffset {
                context: "AVI rewrite cursor".to_owned(),
                offset: patch.offset,
            })?;
        reader
            .seek(SeekFrom::Start(patch_end))
            .map_err(|source| io_error(path, source))?;
        writer
            .write_all(&patch.bytes)
            .map_err(|source| write_io_error(path, source))?;
        cursor = patch_end;
    }
    copy_exact(reader, writer, file_length.saturating_sub(cursor), path)
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
        writer
            .write_all(&buffer[..requested])
            .map_err(|source| write_io_error(path, source))?;
        length -= requested as u64;
    }
    Ok(())
}

fn temporary_path(path: &Path) -> Result<PathBuf> {
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("document.avi");
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

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut output = kind.to_vec();
        output.extend_from_slice(&(data.len() as u32).to_le_bytes());
        output.extend_from_slice(data);
        if data.len() % 2 == 1 {
            output.push(0);
        }
        output
    }

    fn minimal_avi(title: &str) -> Vec<u8> {
        let mut info_payload = b"INFO".to_vec();
        info_payload.extend_from_slice(&chunk(b"INAM", title.as_bytes()));
        let info = chunk(b"LIST", &info_payload);
        let mut output = b"RIFF".to_vec();
        output.extend_from_slice(&((4 + info.len()) as u32).to_le_bytes());
        output.extend_from_slice(b"AVI ");
        output.extend_from_slice(&info);
        output
    }

    #[test]
    fn replaces_existing_info_without_changing_avi_layout() {
        let bytes = minimal_avi("old");
        let output = rewrite_avi_to_vec(
            &bytes,
            FileInfo::new("editable.avi".into(), bytes.len() as u64, FileFormat::Avi),
            ParseLimits::default(),
            &[AviEdit::SetInfo {
                name: "Title".to_owned(),
                value: "new".to_owned(),
            }],
        )
        .unwrap();
        assert_eq!(output.len(), bytes.len());
        let metadata = read_avi(
            &mut Cursor::new(output),
            FileInfo::new("editable.avi".into(), bytes.len() as u64, FileFormat::Avi),
            ParseLimits::default(),
        )
        .unwrap();
        assert_eq!(metadata.find("AVI:Title").unwrap().display_value(), "new");
    }

    #[test]
    fn refuses_info_growth_beyond_the_existing_payload() {
        let bytes = minimal_avi("old");
        let error = rewrite_avi_to_vec(
            &bytes,
            FileInfo::new("editable.avi".into(), bytes.len() as u64, FileFormat::Avi),
            ParseLimits::default(),
            &[AviEdit::SetInfo {
                name: "Title".to_owned(),
                value: "longer".to_owned(),
            }],
        )
        .unwrap_err();
        assert!(error.to_string().contains("available"));
    }

    #[test]
    fn deletes_existing_info_without_changing_avi_layout() {
        let bytes = minimal_avi("old");
        let output = rewrite_avi_to_vec(
            &bytes,
            FileInfo::new("editable.avi".into(), bytes.len() as u64, FileFormat::Avi),
            ParseLimits::default(),
            &[AviEdit::DeleteInfo {
                name: "Title".to_owned(),
            }],
        )
        .expect("AVI INFO deletion should succeed");
        assert_eq!(output.len(), bytes.len());
        let metadata = read_avi(
            &mut Cursor::new(output),
            FileInfo::new("editable.avi".into(), bytes.len() as u64, FileFormat::Avi),
            ParseLimits::default(),
        )
        .expect("deleted AVI should remain readable");
        assert!(metadata.find("AVI:Title").is_none());
    }
}
