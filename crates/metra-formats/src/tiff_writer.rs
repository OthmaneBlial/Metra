use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use metra_core::{FileFormat, FileInfo, Metadata, MetraError, ParseLimits, Result, TagValue};

use crate::atomic::atomic_replace;
use crate::tiff::read_tiff;

/// Safe in-place edits for existing TIFF/BigTIFF ASCII values.
///
/// The writer never changes an IFD layout or allocates a new value area. An
/// edit succeeds only when the replacement, including its terminating NUL,
/// fits in the original ASCII field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TiffEdit {
    SetAscii { key: String, value: String },
}

pub fn rewrite_tiff<R: Read + Seek, W: Write + Seek>(
    reader: &mut R,
    writer: &mut W,
    file_info: FileInfo,
    limits: ParseLimits,
    edits: &[TiffEdit],
) -> Result<()> {
    let metadata = read_tiff(reader, file_info.clone(), limits)?;
    let patches = collect_patches(reader, &metadata, &file_info, limits, edits)?;
    reader
        .seek(SeekFrom::Start(0))
        .map_err(|source| io_error(&file_info.path, source))?;
    rewrite_stream(reader, writer, file_info.size, &file_info.path, &patches)
}

pub fn rewrite_tiff_to_vec(
    bytes: &[u8],
    file_info: FileInfo,
    limits: ParseLimits,
    edits: &[TiffEdit],
) -> Result<Vec<u8>> {
    let mut reader = std::io::Cursor::new(bytes);
    let mut output = std::io::Cursor::new(Vec::new());
    rewrite_tiff(&mut reader, &mut output, file_info.clone(), limits, edits)?;
    let output = output.into_inner();
    let validation_info = FileInfo::new(file_info.path, output.len() as u64, file_info.format);
    read_tiff(
        &mut std::io::Cursor::new(output.as_slice()),
        validation_info,
        limits,
    )?;
    Ok(output)
}

pub fn rewrite_tiff_path(
    path: impl AsRef<Path>,
    limits: ParseLimits,
    edits: &[TiffEdit],
) -> Result<()> {
    let path = path.as_ref().to_path_buf();
    let source_metadata = fs::metadata(&path).map_err(|source| MetraError::Io {
        path: path.clone(),
        source,
    })?;
    let file_info = FileInfo::new(path.clone(), source_metadata.len(), FileFormat::Tiff);
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
        rewrite_tiff(&mut input, &mut output, file_info.clone(), limits, edits)?;
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
        let written_info = FileInfo::new(temp_path.clone(), written_size, FileFormat::Tiff);
        read_tiff(&mut validation, written_info, limits)?;
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

#[derive(Debug, Clone, Copy)]
enum Endian {
    Little,
    Big,
}

#[derive(Debug, Clone, Copy)]
enum Variant {
    Classic { endian: Endian },
    Big { endian: Endian },
}

impl Variant {
    const fn endian(self) -> Endian {
        match self {
            Self::Classic { endian } | Self::Big { endian } => endian,
        }
    }

    const fn inline_size(self) -> usize {
        match self {
            Self::Classic { .. } => 4,
            Self::Big { .. } => 8,
        }
    }

    const fn entry_size(self) -> usize {
        match self {
            Self::Classic { .. } => 12,
            Self::Big { .. } => 20,
        }
    }
}

fn collect_patches<R: Read + Seek>(
    reader: &mut R,
    metadata: &Metadata,
    file_info: &FileInfo,
    limits: ParseLimits,
    edits: &[TiffEdit],
) -> Result<Vec<Patch>> {
    let variant = read_variant(reader, file_info.size, &file_info.path)?;
    let mut patches = Vec::with_capacity(edits.len());
    for edit in edits {
        let TiffEdit::SetAscii { key, value } = edit;
        if value.contains('\0') {
            return Err(MetraError::WriteFailure {
                message: format!("TIFF ASCII value for {key} cannot contain NUL"),
            });
        }
        if value.len() >= limits.max_value_bytes {
            return Err(MetraError::ResourceLimitExceeded {
                resource: "TIFF ASCII value".to_owned(),
                limit: limits.max_value_bytes,
            });
        }
        let matches = metadata.find_all(key);
        let tag = match matches.as_slice() {
            [tag] => *tag,
            [] => {
                return Err(MetraError::WriteFailure {
                    message: format!("TIFF tag {key} does not exist; insertion is not supported"),
                });
            }
            _ => {
                return Err(MetraError::WriteFailure {
                    message: format!(
                        "TIFF tag {key} is repeated; an unambiguous target is required"
                    ),
                });
            }
        };
        let entry_offset = tag.source.offset.ok_or_else(|| MetraError::WriteFailure {
            message: format!("TIFF tag {key} has no source entry offset"),
        })?;
        let entry_size = variant.entry_size();
        let entry = read_at(
            reader,
            entry_offset,
            entry_size,
            file_info.size,
            &file_info.path,
            "TIFF IFD entry",
        )?;
        let endian = variant.endian();
        let type_id = read_u16(endian, &entry[2..4]);
        if type_id != 2 || !matches!(tag.value, TagValue::String(_)) {
            return Err(MetraError::WriteFailure {
                message: format!("TIFF tag {key} is not an existing ASCII value"),
            });
        }
        let count = match variant {
            Variant::Classic { .. } => u64::from(read_u32(endian, &entry[4..8])),
            Variant::Big { .. } => read_u64(endian, &entry[4..12]),
        };
        let count_usize =
            usize::try_from(count).map_err(|_| MetraError::ResourceLimitExceeded {
                resource: "TIFF ASCII value".to_owned(),
                limit: limits.max_value_bytes,
            })?;
        if count == 0 || count_usize > limits.max_value_bytes {
            return Err(MetraError::ResourceLimitExceeded {
                resource: "TIFF ASCII value".to_owned(),
                limit: limits.max_value_bytes,
            });
        }
        let mut replacement = value.as_bytes().to_vec();
        replacement.push(0);
        if replacement.len() > count_usize {
            return Err(MetraError::WriteFailure {
                message: format!(
                    "TIFF ASCII value for {key} needs {} bytes but the field stores {count} bytes",
                    replacement.len()
                ),
            });
        }
        replacement.resize(count_usize, 0);
        let value_field_start = match variant {
            Variant::Classic { .. } => 8,
            Variant::Big { .. } => 12,
        };
        let value_offset = if count_usize <= variant.inline_size() {
            entry_offset
                .checked_add(value_field_start as u64)
                .ok_or(MetraError::InvalidOffset {
                    context: "TIFF inline ASCII value".to_owned(),
                    offset: entry_offset,
                })?
        } else {
            match variant {
                Variant::Classic { .. } => read_u32(
                    endian,
                    &entry[value_field_start..value_field_start + variant.inline_size()],
                ) as u64,
                Variant::Big { .. } => read_u64(
                    endian,
                    &entry[value_field_start..value_field_start + variant.inline_size()],
                ),
            }
        };
        let value_end = value_offset
            .checked_add(count)
            .ok_or(MetraError::InvalidOffset {
                context: format!("TIFF {key} ASCII value"),
                offset: value_offset,
            })?;
        if value_end > file_info.size {
            return Err(MetraError::UnexpectedEof {
                context: format!("TIFF {key} ASCII value"),
            });
        }
        patches.push(Patch {
            offset: value_offset,
            span: count,
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
                    context: "TIFF rewrite patch".to_owned(),
                    offset: pair[0].offset,
                })?;
        if previous_end > pair[1].offset {
            return Err(MetraError::WriteFailure {
                message: "TIFF rewrite patches overlap".to_owned(),
            });
        }
    }
    Ok(patches)
}

fn read_variant<R: Read + Seek>(reader: &mut R, file_length: u64, path: &Path) -> Result<Variant> {
    let header = read_at(reader, 0, 8, file_length, path, "TIFF header")?;
    let endian = match &header[..2] {
        b"II" => Endian::Little,
        b"MM" => Endian::Big,
        _ => {
            return Err(MetraError::InvalidHeader {
                context: "TIFF".to_owned(),
                message: "byte order must be II or MM".to_owned(),
            });
        }
    };
    match read_u16(endian, &header[2..4]) {
        42 => Ok(Variant::Classic { endian }),
        43 => {
            let extended = read_at(reader, 4, 12, file_length, path, "BigTIFF header")?;
            if read_u16(endian, &extended[..2]) != 8 || read_u16(endian, &extended[2..4]) != 0 {
                return Err(MetraError::InvalidHeader {
                    context: "BigTIFF".to_owned(),
                    message: "invalid offset-size or reserved field".to_owned(),
                });
            }
            Ok(Variant::Big { endian })
        }
        magic => Err(MetraError::InvalidHeader {
            context: "TIFF".to_owned(),
            message: format!("expected magic 42 or 43, got {magic}"),
        }),
    }
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
                message: "TIFF rewrite patches are not ordered".to_owned(),
            });
        }
        copy_exact(reader, writer, patch.offset - cursor, path)?;
        reader
            .seek(SeekFrom::Current(i64::try_from(patch.span).map_err(
                |_| MetraError::InvalidOffset {
                    context: "TIFF rewrite patch span".to_owned(),
                    offset: patch.span,
                },
            )?))
            .map_err(|source| io_error(path, source))?;
        writer
            .write_all(&patch.bytes)
            .map_err(|source| write_io_error(path, source))?;
        cursor = patch
            .offset
            .checked_add(patch.span)
            .ok_or(MetraError::InvalidOffset {
                context: "TIFF rewrite cursor".to_owned(),
                offset: patch.offset,
            })?;
    }
    copy_exact(reader, writer, file_length.saturating_sub(cursor), path)
}

fn read_u16(endian: Endian, bytes: &[u8]) -> u16 {
    let bytes: [u8; 2] = bytes.try_into().expect("caller validates u16 length");
    match endian {
        Endian::Little => u16::from_le_bytes(bytes),
        Endian::Big => u16::from_be_bytes(bytes),
    }
}

fn read_u32(endian: Endian, bytes: &[u8]) -> u32 {
    let bytes: [u8; 4] = bytes.try_into().expect("caller validates u32 length");
    match endian {
        Endian::Little => u32::from_le_bytes(bytes),
        Endian::Big => u32::from_be_bytes(bytes),
    }
}

fn read_u64(endian: Endian, bytes: &[u8]) -> u64 {
    let bytes: [u8; 8] = bytes.try_into().expect("caller validates u64 length");
    match endian {
        Endian::Little => u64::from_le_bytes(bytes),
        Endian::Big => u64::from_be_bytes(bytes),
    }
}

fn read_at<R: Read + Seek>(
    reader: &mut R,
    offset: u64,
    length: usize,
    file_length: u64,
    path: &Path,
    context: &str,
) -> Result<Vec<u8>> {
    let length_u64 = u64::try_from(length).map_err(|_| MetraError::InvalidOffset {
        context: context.to_owned(),
        offset,
    })?;
    let end = offset
        .checked_add(length_u64)
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
        .map_err(|source| io_error(path, source))?;
    let mut bytes = vec![0_u8; length];
    reader
        .read_exact(&mut bytes)
        .map_err(|source| io_error(path, source))?;
    Ok(bytes)
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
        .unwrap_or("metadata.tif");
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
    use metra_core::FileFormat;

    fn tiff_with_make(value: &[u8]) -> Vec<u8> {
        let mut bytes = vec![
            b'I',
            b'I',
            42,
            0,
            8,
            0,
            0,
            0,
            1,
            0, // one IFD0 entry
            0x0F,
            0x01,
            2,
            0,
            value.len() as u8,
            0,
            0,
            0,
            26,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
        ];
        assert_eq!(bytes.len(), 26);
        bytes.extend_from_slice(value);
        bytes
    }

    fn info(bytes: &[u8]) -> FileInfo {
        FileInfo::new("editable.tif".into(), bytes.len() as u64, FileFormat::Tiff)
    }

    #[test]
    fn replaces_existing_ascii_without_changing_file_size() {
        let bytes = tiff_with_make(b"Canon\0");
        let output = rewrite_tiff_to_vec(
            &bytes,
            info(&bytes),
            ParseLimits::default(),
            &[TiffEdit::SetAscii {
                key: "EXIF:Make".to_owned(),
                value: "Sony".to_owned(),
            }],
        )
        .expect("in-place TIFF edit should succeed");
        assert_eq!(output.len(), bytes.len());
        let metadata = read_tiff(
            &mut Cursor::new(output),
            FileInfo::new("edited.tif".into(), bytes.len() as u64, FileFormat::Tiff),
            ParseLimits::default(),
        )
        .expect("edited TIFF should remain readable");
        assert_eq!(metadata.find("EXIF:Make").unwrap().display_value(), "Sony");
    }

    #[test]
    fn refuses_values_that_need_new_storage() {
        let bytes = tiff_with_make(b"A\0");
        let result = rewrite_tiff_to_vec(
            &bytes,
            info(&bytes),
            ParseLimits::default(),
            &[TiffEdit::SetAscii {
                key: "EXIF:Make".to_owned(),
                value: "Canon".to_owned(),
            }],
        );
        assert!(matches!(result, Err(MetraError::WriteFailure { .. })));
    }

    #[test]
    fn edits_bigtiff_inline_ascii_values() {
        let mut bytes = vec![b'I', b'I', 43, 0, 8, 0, 0, 0, 16, 0, 0, 0, 0, 0, 0, 0];
        bytes.extend_from_slice(&1_u64.to_le_bytes());
        bytes.extend_from_slice(&0x010F_u16.to_le_bytes());
        bytes.extend_from_slice(&2_u16.to_le_bytes());
        bytes.extend_from_slice(&5_u64.to_le_bytes());
        bytes.extend_from_slice(b"Canon\0\0\0");
        bytes.extend_from_slice(&0_u64.to_le_bytes());
        let output = rewrite_tiff_to_vec(
            &bytes,
            info(&bytes),
            ParseLimits::default(),
            &[TiffEdit::SetAscii {
                key: "EXIF:Make".to_owned(),
                value: "Sony".to_owned(),
            }],
        )
        .expect("BigTIFF in-place edit should succeed");
        let metadata = read_tiff(
            &mut Cursor::new(output),
            info(&bytes),
            ParseLimits::default(),
        )
        .expect("edited BigTIFF should remain readable");
        assert_eq!(metadata.find("EXIF:Make").unwrap().display_value(), "Sony");
    }
}
