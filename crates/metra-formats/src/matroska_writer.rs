use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use metra_core::{FileFormat, FileInfo, Metadata, MetraError, ParseLimits, Result, TagValue};

use crate::atomic::atomic_replace;
use crate::matroska::read_matroska;

/// Lossless edits for existing Matroska/WebM `SimpleTag` string values.
///
/// The EBML element widths, tag names, container layout, and media payloads
/// remain unchanged. Replacements are zero-padded inside the existing
/// `TagString` payload and cannot create or delete tags.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MatroskaEdit {
    SetTag { name: String, value: String },
    SetString { key: String, value: String },
    DeleteTag { name: String },
    DeleteString { key: String },
}

pub fn rewrite_matroska<R: Read + Seek, W: Write + Seek>(
    reader: &mut R,
    writer: &mut W,
    file_info: FileInfo,
    limits: ParseLimits,
    edits: &[MatroskaEdit],
) -> Result<()> {
    if !is_matroska_format(file_info.format) {
        return Err(MetraError::WriteFailure {
            message: format!("Matroska writer cannot edit {}", file_info.format),
        });
    }
    let metadata = read_matroska(reader, file_info.clone(), limits)?;
    let patches = collect_patches(&metadata, limits, edits)?;
    reader
        .seek(SeekFrom::Start(0))
        .map_err(|source| io_error(&file_info.path, source))?;
    rewrite_stream(reader, writer, file_info.size, &file_info.path, &patches)
}

pub fn rewrite_matroska_to_vec(
    bytes: &[u8],
    file_info: FileInfo,
    limits: ParseLimits,
    edits: &[MatroskaEdit],
) -> Result<Vec<u8>> {
    let mut reader = std::io::Cursor::new(bytes);
    let mut output = std::io::Cursor::new(Vec::new());
    rewrite_matroska(&mut reader, &mut output, file_info.clone(), limits, edits)?;
    let output = output.into_inner();
    read_matroska(
        &mut std::io::Cursor::new(output.as_slice()),
        FileInfo::new(file_info.path, output.len() as u64, file_info.format),
        limits,
    )?;
    Ok(output)
}

pub fn rewrite_matroska_path(
    path: impl AsRef<Path>,
    limits: ParseLimits,
    edits: &[MatroskaEdit],
) -> Result<()> {
    let path = path.as_ref().to_path_buf();
    let source_metadata = crate::read_path_with_limits(&path, limits)?;
    let format = source_metadata.file_info.format;
    if !is_matroska_format(format) {
        return Err(MetraError::WriteFailure {
            message: format!("Matroska writer cannot edit {format}"),
        });
    }
    let source_file_metadata = fs::metadata(&path).map_err(|source| MetraError::Io {
        path: path.clone(),
        source,
    })?;
    let file_info = FileInfo::new(path.clone(), source_file_metadata.len(), format);
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
        rewrite_matroska(&mut input, &mut output, file_info.clone(), limits, edits)?;
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
        read_matroska(
            &mut validation,
            FileInfo::new(temp_path.clone(), written_size, format),
            limits,
        )?;
        fs::set_permissions(&temp_path, source_file_metadata.permissions()).map_err(|source| {
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
    edits: &[MatroskaEdit],
) -> Result<Vec<Patch>> {
    let mut patches = Vec::with_capacity(edits.len());
    for edit in edits {
        let (key, value, simple_tag) = match edit {
            MatroskaEdit::SetTag { name, value } => {
                if name.is_empty() || name.contains('\0') {
                    return Err(MetraError::WriteFailure {
                        message: "Matroska SimpleTag name cannot be empty or contain NUL"
                            .to_owned(),
                    });
                }
                (format!("Matroska:Tag:{name}"), value.as_str(), true)
            }
            MatroskaEdit::DeleteTag { name } => {
                if name.is_empty() || name.contains('\0') {
                    return Err(MetraError::WriteFailure {
                        message: "Matroska SimpleTag name cannot be empty or contain NUL"
                            .to_owned(),
                    });
                }
                (format!("Matroska:Tag:{name}"), "", true)
            }
            MatroskaEdit::SetString { key, value } => {
                if !is_writable_string_key(key) {
                    return Err(MetraError::WriteFailure {
                        message: format!("Matroska string field {key} is not writable"),
                    });
                }
                (key.clone(), value.as_str(), false)
            }
            MatroskaEdit::DeleteString { key } => {
                if !is_writable_string_key(key) {
                    return Err(MetraError::WriteFailure {
                        message: format!("Matroska string field {key} is not writable"),
                    });
                }
                (key.clone(), "", false)
            }
        };
        if value.contains('\0') {
            return Err(MetraError::WriteFailure {
                message: format!("Matroska string value for {key} cannot contain NUL"),
            });
        }
        let tag = match metadata.find_all(&key).as_slice() {
            [tag] => *tag,
            [] => {
                return Err(MetraError::WriteFailure {
                    message: format!("Matroska string field {key} does not exist"),
                });
            }
            _ => {
                return Err(MetraError::WriteFailure {
                    message: format!(
                        "Matroska string field {key} is repeated; an unambiguous target is required"
                    ),
                });
            }
        };
        let expected_container = if simple_tag {
            "Matroska/Tags"
        } else {
            "Matroska/Info"
        };
        if tag.source.container != expected_container {
            return Err(MetraError::WriteFailure {
                message: format!("Matroska string field {key} has no {expected_container} source"),
            });
        }
        if !matches!(tag.value, TagValue::String(_)) {
            return Err(MetraError::WriteFailure {
                message: format!("Matroska string field {key} is not a string"),
            });
        }
        let offset = tag.source.offset.ok_or_else(|| MetraError::WriteFailure {
            message: format!("Matroska string field {key} has no source offset"),
        })?;
        let span = tag.source.length.ok_or_else(|| MetraError::WriteFailure {
            message: format!("Matroska string field {key} has no source length"),
        })?;
        let span_usize = usize::try_from(span).map_err(|_| MetraError::ResourceLimitExceeded {
            resource: "Matroska SimpleTag value".to_owned(),
            limit: limits.max_value_bytes,
        })?;
        if span_usize > limits.max_value_bytes {
            return Err(MetraError::ResourceLimitExceeded {
                resource: "Matroska SimpleTag value".to_owned(),
                limit: limits.max_value_bytes,
            });
        }
        if value.len() > span_usize {
            return Err(MetraError::WriteFailure {
                message: format!(
                    "Matroska string field {key} has {} bytes available, {} needed",
                    span_usize,
                    value.len()
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
                    context: "Matroska rewrite patch".to_owned(),
                    offset: pair[0].offset,
                })?;
        if previous_end > pair[1].offset {
            return Err(MetraError::WriteFailure {
                message: "Matroska rewrite patches overlap".to_owned(),
            });
        }
    }
    Ok(patches)
}

fn is_writable_string_key(key: &str) -> bool {
    matches!(
        key,
        "Matroska:Title" | "Matroska:MuxingApp" | "Matroska:WritingApp"
    )
}

fn is_matroska_format(format: FileFormat) -> bool {
    matches!(format, FileFormat::Mkv | FileFormat::Webm)
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
                message: "Matroska rewrite patches are not ordered".to_owned(),
            });
        }
        copy_exact(reader, writer, patch.offset - cursor, path)?;
        let patch_end = patch
            .offset
            .checked_add(patch.span)
            .ok_or(MetraError::InvalidOffset {
                context: "Matroska rewrite cursor".to_owned(),
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
        .unwrap_or("document.mkv");
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

    fn element(id: &[u8], data: &[u8]) -> Vec<u8> {
        assert!(data.len() < 127);
        let mut output = id.to_vec();
        output.push(0x80 | data.len() as u8);
        output.extend_from_slice(data);
        output
    }

    fn minimal_webm(tag_value: &str) -> Vec<u8> {
        let ebml = element(&[0x42, 0x82], b"webm");
        let ebml_header = element(b"\x1A\x45\xDF\xA3", &ebml);
        let simple_tag = [
            element(&[0x45, 0xA3], b"TITLE"),
            element(&[0x44, 0x87], tag_value.as_bytes()),
        ]
        .concat();
        let tags = element(
            &[0x12, 0x54, 0xC3, 0x67],
            &element(&[0x73, 0x73], &element(&[0x67, 0xC8], &simple_tag)),
        );
        [ebml_header, tags].concat()
    }

    fn minimal_webm_with_info_title(title: &str) -> Vec<u8> {
        let ebml = element(&[0x42, 0x82], b"webm");
        let ebml_header = element(b"\x1A\x45\xDF\xA3", &ebml);
        let info = element(
            &[0x15, 0x49, 0xA9, 0x66],
            &element(&[0x7B, 0xA9], title.as_bytes()),
        );
        [ebml_header, info].concat()
    }

    #[test]
    fn replaces_existing_simple_tag_without_changing_ebml_layout() {
        let bytes = minimal_webm("old");
        let output = rewrite_matroska_to_vec(
            &bytes,
            FileInfo::new("editable.webm".into(), bytes.len() as u64, FileFormat::Webm),
            ParseLimits::default(),
            &[MatroskaEdit::SetTag {
                name: "TITLE".to_owned(),
                value: "new".to_owned(),
            }],
        )
        .unwrap();
        assert_eq!(output.len(), bytes.len());
        let metadata = read_matroska(
            &mut Cursor::new(output),
            FileInfo::new("editable.webm".into(), bytes.len() as u64, FileFormat::Webm),
            ParseLimits::default(),
        )
        .unwrap();
        assert_eq!(
            metadata.find("Matroska:Tag:TITLE").unwrap().display_value(),
            "new"
        );
    }

    #[test]
    fn refuses_simple_tag_growth_beyond_existing_payload() {
        let bytes = minimal_webm("old");
        let error = rewrite_matroska_to_vec(
            &bytes,
            FileInfo::new("editable.mkv".into(), bytes.len() as u64, FileFormat::Mkv),
            ParseLimits::default(),
            &[MatroskaEdit::SetTag {
                name: "TITLE".to_owned(),
                value: "longer".to_owned(),
            }],
        )
        .unwrap_err();
        assert!(error.to_string().contains("available"));
    }

    #[test]
    fn replaces_existing_info_title_without_changing_ebml_layout() {
        let bytes = minimal_webm_with_info_title("old");
        let output = rewrite_matroska_to_vec(
            &bytes,
            FileInfo::new("editable.webm".into(), bytes.len() as u64, FileFormat::Webm),
            ParseLimits::default(),
            &[MatroskaEdit::SetString {
                key: "Matroska:Title".to_owned(),
                value: "new".to_owned(),
            }],
        )
        .unwrap();
        assert_eq!(output.len(), bytes.len());
        let metadata = read_matroska(
            &mut Cursor::new(output),
            FileInfo::new("editable.webm".into(), bytes.len() as u64, FileFormat::Webm),
            ParseLimits::default(),
        )
        .unwrap();
        assert_eq!(
            metadata.find("Matroska:Title").unwrap().display_value(),
            "new"
        );
    }

    #[test]
    fn deletes_existing_simple_tag_without_changing_ebml_layout() {
        let bytes = minimal_webm("old");
        let output = rewrite_matroska_to_vec(
            &bytes,
            FileInfo::new("editable.webm".into(), bytes.len() as u64, FileFormat::Webm),
            ParseLimits::default(),
            &[MatroskaEdit::DeleteTag {
                name: "TITLE".to_owned(),
            }],
        )
        .expect("SimpleTag deletion should succeed");
        assert_eq!(output.len(), bytes.len());
        let metadata = read_matroska(
            &mut Cursor::new(output),
            FileInfo::new("editable.webm".into(), bytes.len() as u64, FileFormat::Webm),
            ParseLimits::default(),
        )
        .expect("deleted SimpleTag output should remain readable");
        assert!(metadata.find("Matroska:Tag:TITLE").is_none());
    }

    #[test]
    fn deletes_existing_info_title_without_changing_ebml_layout() {
        let bytes = minimal_webm_with_info_title("old");
        let output = rewrite_matroska_to_vec(
            &bytes,
            FileInfo::new("editable.webm".into(), bytes.len() as u64, FileFormat::Webm),
            ParseLimits::default(),
            &[MatroskaEdit::DeleteString {
                key: "Matroska:Title".to_owned(),
            }],
        )
        .expect("Info title deletion should succeed");
        assert_eq!(output.len(), bytes.len());
        let metadata = read_matroska(
            &mut Cursor::new(output),
            FileInfo::new("editable.webm".into(), bytes.len() as u64, FileFormat::Webm),
            ParseLimits::default(),
        )
        .expect("deleted Info output should remain readable");
        assert!(metadata.find("Matroska:Title").is_none());
    }
}
