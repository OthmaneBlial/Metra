use std::fs::{self, File, OpenOptions};
use std::io::{Cursor, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use metra_core::{FileFormat, FileInfo, Metadata, MetraError, ParseLimits, Result, TagValue};

use crate::atomic::atomic_replace;
use crate::icc::read_icc;

/// Lossless edits for existing text-bearing ICC profile tags.
///
/// The writer never changes the ICC tag table or a tag payload's allocated
/// span. `desc` and `text` payloads can be replaced when the existing storage
/// is large enough; all supported text payloads can be cleared by zero-filling
/// the existing span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IccEdit {
    SetText { name: String, value: String },
    DeleteText { name: String },
}

pub fn rewrite_icc<R: Read + Seek, W: Write + Seek>(
    reader: &mut R,
    writer: &mut W,
    file_info: FileInfo,
    limits: ParseLimits,
    edits: &[IccEdit],
) -> Result<()> {
    if file_info.format != FileFormat::Icc {
        return Err(MetraError::WriteFailure {
            message: format!("ICC writer cannot edit {}", file_info.format),
        });
    }
    let bytes = crate::read_bounded_document(reader, &file_info, limits, "ICC profile")?;
    let metadata = read_icc(
        &mut Cursor::new(bytes.as_slice()),
        file_info.clone(),
        limits,
    )?;
    let output = rewrite_bytes(&bytes, &metadata, &file_info, limits, edits)?;
    writer
        .seek(SeekFrom::Start(0))
        .map_err(|source| write_error(&file_info.path, source))?;
    writer
        .write_all(&output)
        .map_err(|source| write_error(&file_info.path, source))
}

pub fn rewrite_icc_to_vec(
    bytes: &[u8],
    file_info: FileInfo,
    limits: ParseLimits,
    edits: &[IccEdit],
) -> Result<Vec<u8>> {
    let metadata = read_icc(
        &mut Cursor::new(bytes),
        FileInfo::new(file_info.path.clone(), bytes.len() as u64, FileFormat::Icc),
        limits,
    )?;
    let file_info = FileInfo::new(file_info.path, bytes.len() as u64, FileFormat::Icc);
    let output = rewrite_bytes(bytes, &metadata, &file_info, limits, edits)?;
    read_icc(
        &mut Cursor::new(output.as_slice()),
        FileInfo::new(file_info.path, output.len() as u64, FileFormat::Icc),
        limits,
    )?;
    Ok(output)
}

pub fn rewrite_icc_path(
    path: impl AsRef<Path>,
    limits: ParseLimits,
    edits: &[IccEdit],
) -> Result<()> {
    let path = path.as_ref().to_path_buf();
    let source_metadata = fs::metadata(&path).map_err(|source| MetraError::Io {
        path: path.clone(),
        source,
    })?;
    let file_info = FileInfo::new(path.clone(), source_metadata.len(), FileFormat::Icc);
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
        rewrite_icc(&mut input, &mut output, file_info.clone(), limits, edits)?;
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
        read_icc(
            &mut validation,
            FileInfo::new(temp_path.clone(), written_size, FileFormat::Icc),
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
struct Patch {
    offset: usize,
    bytes: Vec<u8>,
}

fn rewrite_bytes(
    bytes: &[u8],
    metadata: &Metadata,
    file_info: &FileInfo,
    limits: ParseLimits,
    edits: &[IccEdit],
) -> Result<Vec<u8>> {
    if edits.is_empty() {
        return Err(MetraError::WriteFailure {
            message: "ICC rewrite requires at least one edit".to_owned(),
        });
    }
    let mut patches = Vec::with_capacity(edits.len());
    for edit in edits {
        let (name, operation) = match edit {
            IccEdit::SetText { name, value } => (name.as_str(), Operation::Set(value)),
            IccEdit::DeleteText { name } => (name.as_str(), Operation::Delete),
        };
        if !is_writable_text_name(name) {
            return Err(MetraError::WriteFailure {
                message: format!("ICC profile tag {name} is not writable"),
            });
        }
        let key = format!("ICC:{name}");
        let tag = match metadata.find_all(&key).as_slice() {
            [tag] => *tag,
            [] => {
                return Err(MetraError::WriteFailure {
                    message: format!("ICC profile tag {name} does not exist"),
                });
            }
            _ => {
                return Err(MetraError::WriteFailure {
                    message: format!(
                        "ICC profile tag {name} is repeated; an unambiguous target is required"
                    ),
                });
            }
        };
        if !matches!(tag.value, TagValue::String(_)) {
            return Err(MetraError::WriteFailure {
                message: format!("ICC profile tag {name} is not a text value"),
            });
        }
        let offset =
            usize::try_from(tag.source.offset.ok_or_else(|| MetraError::WriteFailure {
                message: format!("ICC profile tag {name} has no source offset"),
            })?)
            .map_err(|_| MetraError::ResourceLimitExceeded {
                resource: "ICC tag offset".to_owned(),
                limit: limits.max_value_bytes,
            })?;
        let span = usize::try_from(tag.source.length.ok_or_else(|| MetraError::WriteFailure {
            message: format!("ICC profile tag {name} has no source length"),
        })?)
        .map_err(|_| MetraError::ResourceLimitExceeded {
            resource: "ICC tag payload".to_owned(),
            limit: limits.max_value_bytes,
        })?;
        if span > limits.max_value_bytes {
            return Err(MetraError::ResourceLimitExceeded {
                resource: "ICC tag payload".to_owned(),
                limit: limits.max_value_bytes,
            });
        }
        let end = offset.checked_add(span).ok_or(MetraError::InvalidOffset {
            context: "ICC rewrite tag payload".to_owned(),
            offset: offset as u64,
        })?;
        let current = bytes.get(offset..end).ok_or(MetraError::UnexpectedEof {
            context: "ICC rewrite tag payload".to_owned(),
        })?;
        let replacement = match operation {
            Operation::Set(value) => encode_text_payload(current, value, name, limits)?,
            Operation::Delete => delete_text_payload(current, name)?,
        };
        if replacement.len() != current.len() {
            return Err(MetraError::WriteFailure {
                message: format!("ICC profile tag {name} changed its allocated payload size"),
            });
        }
        patches.push(Patch {
            offset,
            bytes: replacement,
        });
    }
    patches.sort_by_key(|patch| patch.offset);
    for pair in patches.windows(2) {
        let previous_end =
            pair[0]
                .offset
                .checked_add(pair[0].bytes.len())
                .ok_or(MetraError::InvalidOffset {
                    context: "ICC rewrite patch".to_owned(),
                    offset: pair[0].offset as u64,
                })?;
        if previous_end > pair[1].offset {
            return Err(MetraError::WriteFailure {
                message: "ICC rewrite patches overlap".to_owned(),
            });
        }
    }
    let mut output = bytes.to_vec();
    for patch in patches {
        let end = patch.offset + patch.bytes.len();
        output[patch.offset..end].copy_from_slice(&patch.bytes);
    }
    let _ = file_info;
    Ok(output)
}

enum Operation<'a> {
    Set(&'a str),
    Delete,
}

fn is_writable_text_name(name: &str) -> bool {
    matches!(
        name,
        "Description" | "Copyright" | "ManufacturerDescription" | "ModelDescription"
    )
}

fn encode_text_payload(
    current: &[u8],
    value: &str,
    name: &str,
    limits: ParseLimits,
) -> Result<Vec<u8>> {
    let value = value.as_bytes();
    if value.is_empty()
        || value.contains(&0)
        || !value
            .iter()
            .all(|byte| *byte == b' ' || byte.is_ascii_graphic())
    {
        return Err(MetraError::WriteFailure {
            message: format!("ICC profile tag {name} requires non-empty printable ASCII"),
        });
    }
    if value.len() >= limits.max_value_bytes {
        return Err(MetraError::ResourceLimitExceeded {
            resource: format!("ICC profile tag {name}"),
            limit: limits.max_value_bytes,
        });
    }
    let mut replacement = current.to_vec();
    match current.get(..4) {
        Some(b"desc") => {
            if current.len() < 12 {
                return Err(MetraError::WriteFailure {
                    message: format!("ICC profile tag {name} has an invalid desc payload"),
                });
            }
            let declared = usize::try_from(u32::from_be_bytes(
                current[8..12].try_into().expect("ICC desc length"),
            ))
            .map_err(|_| MetraError::ResourceLimitExceeded {
                resource: format!("ICC profile tag {name}"),
                limit: limits.max_value_bytes,
            })?;
            let body_end = 12_usize
                .checked_add(declared)
                .ok_or(MetraError::InvalidOffset {
                    context: "ICC desc payload".to_owned(),
                    offset: declared as u64,
                })?;
            if declared == 0 || body_end > current.len() || value.len() + 1 > declared {
                return Err(MetraError::WriteFailure {
                    message: format!(
                        "ICC profile tag {name} replacement does not fit its desc payload"
                    ),
                });
            }
            replacement[12..body_end].fill(0);
            replacement[12..12 + value.len()].copy_from_slice(value);
        }
        Some(b"text") => {
            if current.len() < 8 || value.len() > current.len() - 8 {
                return Err(MetraError::WriteFailure {
                    message: format!(
                        "ICC profile tag {name} replacement does not fit its text payload"
                    ),
                });
            }
            replacement[8..].fill(0);
            replacement[8..8 + value.len()].copy_from_slice(value);
        }
        Some(b"mluc") => return encode_mluc_payload(current, value, name, limits),
        _ => {
            return Err(MetraError::WriteFailure {
                message: format!("ICC profile tag {name} does not use writable text storage"),
            });
        }
    }
    Ok(replacement)
}

fn encode_mluc_payload(
    current: &[u8],
    value: &[u8],
    name: &str,
    limits: ParseLimits,
) -> Result<Vec<u8>> {
    let value = std::str::from_utf8(value).map_err(|_| MetraError::WriteFailure {
        message: format!("ICC profile tag {name} requires valid UTF-8 text"),
    })?;
    if value.contains('\0') {
        return Err(MetraError::WriteFailure {
            message: format!("ICC profile tag {name} cannot contain NUL"),
        });
    }
    let encoded_length =
        value
            .encode_utf16()
            .count()
            .checked_mul(2)
            .ok_or(MetraError::ResourceLimitExceeded {
                resource: format!("ICC profile tag {name}"),
                limit: limits.max_value_bytes,
            })?;
    if encoded_length == 0 || encoded_length > limits.max_value_bytes {
        return Err(MetraError::ResourceLimitExceeded {
            resource: format!("ICC profile tag {name}"),
            limit: limits.max_value_bytes,
        });
    }
    if current.len() < 28 {
        return Err(MetraError::WriteFailure {
            message: format!("ICC profile tag {name} has an invalid mluc payload"),
        });
    }
    let count = usize::try_from(u32::from_be_bytes(
        current[8..12].try_into().expect("ICC mluc count"),
    ))
    .map_err(|_| MetraError::ResourceLimitExceeded {
        resource: format!("ICC profile tag {name} records"),
        limit: limits.max_ifd_entries,
    })?;
    let record_size = usize::try_from(u32::from_be_bytes(
        current[12..16].try_into().expect("ICC mluc record size"),
    ))
    .map_err(|_| MetraError::ResourceLimitExceeded {
        resource: format!("ICC profile tag {name} records"),
        limit: limits.max_value_bytes,
    })?;
    if count == 0 || record_size < 12 || count > limits.max_ifd_entries {
        return Err(MetraError::WriteFailure {
            message: format!("ICC profile tag {name} has invalid mluc records"),
        });
    }
    let table_end = 16_usize
        .checked_add(
            count
                .checked_mul(record_size)
                .ok_or(MetraError::InvalidOffset {
                    context: "ICC mluc record table".to_owned(),
                    offset: count as u64,
                })?,
        )
        .ok_or(MetraError::InvalidOffset {
            context: "ICC mluc record table".to_owned(),
            offset: count as u64,
        })?;
    if table_end > current.len() {
        return Err(MetraError::WriteFailure {
            message: format!("ICC profile tag {name} has a truncated mluc table"),
        });
    }
    let old_length = usize::try_from(u32::from_be_bytes(
        current[20..24].try_into().expect("ICC mluc length"),
    ))
    .map_err(|_| MetraError::ResourceLimitExceeded {
        resource: format!("ICC profile tag {name}"),
        limit: limits.max_value_bytes,
    })?;
    let offset = usize::try_from(u32::from_be_bytes(
        current[24..28].try_into().expect("ICC mluc offset"),
    ))
    .map_err(|_| MetraError::ResourceLimitExceeded {
        resource: format!("ICC profile tag {name}"),
        limit: limits.max_value_bytes,
    })?;
    let end = offset
        .checked_add(old_length)
        .ok_or(MetraError::InvalidOffset {
            context: "ICC mluc string".to_owned(),
            offset: offset as u64,
        })?;
    if old_length % 2 != 0 || offset < table_end || end > current.len() {
        return Err(MetraError::WriteFailure {
            message: format!("ICC profile tag {name} has an invalid mluc string range"),
        });
    }
    if encoded_length > old_length {
        return Err(MetraError::WriteFailure {
            message: format!("ICC profile tag {name} replacement does not fit its mluc payload"),
        });
    }
    let mut replacement = current.to_vec();
    replacement[20..24].copy_from_slice(&(encoded_length as u32).to_be_bytes());
    replacement[offset..end].fill(0);
    for (index, unit) in value.encode_utf16().enumerate() {
        let start = offset + index * 2;
        replacement[start..start + 2].copy_from_slice(&unit.to_be_bytes());
    }
    Ok(replacement)
}

fn delete_text_payload(current: &[u8], name: &str) -> Result<Vec<u8>> {
    match current.get(..4) {
        Some(b"desc" | b"text" | b"mluc") if !current.is_empty() => Ok(vec![0; current.len()]),
        _ => Err(MetraError::WriteFailure {
            message: format!("ICC profile tag {name} does not use supported text storage"),
        }),
    }
}

fn temporary_path(path: &Path) -> Result<PathBuf> {
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("metadata.icc");
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

fn write_error(path: &Path, source: std::io::Error) -> MetraError {
    MetraError::WriteFailure {
        message: format!("{}: {source}", path.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::icc_create::{IccCreateOptions, create_icc_to_vec};

    fn info(bytes: &[u8]) -> FileInfo {
        FileInfo::new("editable.icc".into(), bytes.len() as u64, FileFormat::Icc)
    }

    #[test]
    fn rewrites_existing_desc_text_without_changing_profile_size() {
        let bytes = create_icc_to_vec(
            &IccCreateOptions::new().with_text("Description", "old"),
            ParseLimits::default(),
        )
        .unwrap();
        let output = rewrite_icc_to_vec(
            &bytes,
            info(&bytes),
            ParseLimits::default(),
            &[IccEdit::SetText {
                name: "Description".to_owned(),
                value: "new".to_owned(),
            }],
        )
        .unwrap();
        assert_eq!(output.len(), bytes.len());
        let metadata = read_icc(
            &mut Cursor::new(output),
            info(&bytes),
            ParseLimits::default(),
        )
        .unwrap();
        assert_eq!(
            metadata.find("ICC:Description").unwrap().display_value(),
            "new"
        );
    }

    #[test]
    fn deletes_existing_desc_text_by_zero_filling_payload() {
        let bytes = create_icc_to_vec(
            &IccCreateOptions::new().with_text("Description", "old"),
            ParseLimits::default(),
        )
        .unwrap();
        let output = rewrite_icc_to_vec(
            &bytes,
            info(&bytes),
            ParseLimits::default(),
            &[IccEdit::DeleteText {
                name: "Description".to_owned(),
            }],
        )
        .unwrap();
        let metadata = read_icc(
            &mut Cursor::new(output),
            info(&bytes),
            ParseLimits::default(),
        )
        .unwrap();
        assert!(metadata.find("ICC:Description").is_none());
    }

    #[test]
    fn rejects_text_growth_before_writing() {
        let bytes = create_icc_to_vec(
            &IccCreateOptions::new().with_text("Description", "old"),
            ParseLimits::default(),
        )
        .unwrap();
        let error = rewrite_icc_to_vec(
            &bytes,
            info(&bytes),
            ParseLimits::default(),
            &[IccEdit::SetText {
                name: "Description".to_owned(),
                value: "too long".to_owned(),
            }],
        )
        .unwrap_err();
        assert!(error.to_string().contains("does not fit"));
    }

    #[test]
    fn rewrites_first_mluc_locale_without_changing_profile_size() {
        let value = "old";
        let encoded = value
            .encode_utf16()
            .flat_map(u16::to_be_bytes)
            .collect::<Vec<_>>();
        let offset = 28_u32;
        let mut payload = b"mluc".to_vec();
        payload.extend_from_slice(&[0; 4]);
        payload.extend_from_slice(&1_u32.to_be_bytes());
        payload.extend_from_slice(&12_u32.to_be_bytes());
        payload.extend_from_slice(b"enUS");
        payload.extend_from_slice(&(encoded.len() as u32).to_be_bytes());
        payload.extend_from_slice(&offset.to_be_bytes());
        payload.extend_from_slice(&encoded);

        let tag_offset = 144_u32;
        let mut bytes = vec![0_u8; tag_offset as usize];
        bytes[8] = 4;
        bytes[9] = 0x30;
        bytes[12..16].copy_from_slice(b"mntr");
        bytes[16..20].copy_from_slice(b"RGB ");
        bytes[20..24].copy_from_slice(b"XYZ ");
        bytes[24..36].copy_from_slice(&[0x07, 0xEA, 0, 1, 0, 1, 0, 0, 0, 0, 0, 0]);
        bytes[36..40].copy_from_slice(b"acsp");
        bytes[128..132].copy_from_slice(&1_u32.to_be_bytes());
        bytes[132..136].copy_from_slice(b"desc");
        bytes[136..140].copy_from_slice(&tag_offset.to_be_bytes());
        bytes[140..144].copy_from_slice(&(payload.len() as u32).to_be_bytes());
        bytes.extend_from_slice(&payload);
        let profile_size = bytes.len() as u32;
        bytes[0..4].copy_from_slice(&profile_size.to_be_bytes());

        let output = rewrite_icc_to_vec(
            &bytes,
            info(&bytes),
            ParseLimits::default(),
            &[IccEdit::SetText {
                name: "Description".to_owned(),
                value: "new".to_owned(),
            }],
        )
        .unwrap();
        assert_eq!(output.len(), bytes.len());
        let metadata = read_icc(
            &mut Cursor::new(output),
            info(&bytes),
            ParseLimits::default(),
        )
        .unwrap();
        assert_eq!(
            metadata.find("ICC:Description").unwrap().display_value(),
            "new"
        );
    }
}
