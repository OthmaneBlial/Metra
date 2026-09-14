use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use metra_core::{FileFormat, FileInfo, Metadata, MetraError, ParseLimits, Result, TagValue};

use crate::atomic::atomic_replace;
use crate::psd::read_psd;
use crate::xmp::parse_xmp;

/// Lossless edits for existing Photoshop XMP image resources.
///
/// The resource directory is intentionally not rebuilt by this first PSD
/// writer. A replacement succeeds only when its byte length matches the
/// existing resource payload, preserving every section boundary and offset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PsdEdit {
    SetXmp(String),
    /// Remove the existing XMP resource while preserving its allocated span.
    DeleteXmp,
}

pub fn rewrite_psd<R: Read + Seek, W: Write + Seek>(
    reader: &mut R,
    writer: &mut W,
    file_info: FileInfo,
    limits: ParseLimits,
    edits: &[PsdEdit],
) -> Result<()> {
    if file_info.format != FileFormat::Psd {
        return Err(MetraError::WriteFailure {
            message: format!("PSD writer cannot edit {}", file_info.format),
        });
    }
    let metadata = read_psd(reader, file_info.clone(), limits)?;
    let patches = collect_patches(reader, &metadata, &file_info, limits, edits)?;
    reader
        .seek(SeekFrom::Start(0))
        .map_err(|source| io_error(&file_info.path, source))?;
    rewrite_stream(reader, writer, file_info.size, &file_info.path, &patches)
}

pub fn rewrite_psd_to_vec(
    bytes: &[u8],
    file_info: FileInfo,
    limits: ParseLimits,
    edits: &[PsdEdit],
) -> Result<Vec<u8>> {
    let mut reader = std::io::Cursor::new(bytes);
    let mut output = std::io::Cursor::new(Vec::new());
    rewrite_psd(&mut reader, &mut output, file_info.clone(), limits, edits)?;
    let output = output.into_inner();
    read_psd(
        &mut std::io::Cursor::new(output.as_slice()),
        FileInfo::new(file_info.path, output.len() as u64, FileFormat::Psd),
        limits,
    )?;
    Ok(output)
}

pub fn rewrite_psd_path(
    path: impl AsRef<Path>,
    limits: ParseLimits,
    edits: &[PsdEdit],
) -> Result<()> {
    let path = path.as_ref().to_path_buf();
    let source_metadata = fs::metadata(&path).map_err(|source| MetraError::Io {
        path: path.clone(),
        source,
    })?;
    let file_info = FileInfo::new(path.clone(), source_metadata.len(), FileFormat::Psd);
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
        rewrite_psd(&mut input, &mut output, file_info.clone(), limits, edits)?;
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
        read_psd(
            &mut validation,
            FileInfo::new(temp_path.clone(), written_size, FileFormat::Psd),
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

#[derive(Debug, Clone)]
struct Patch {
    offset: u64,
    span: u64,
    bytes: Vec<u8>,
}

fn collect_patches<R: Read + Seek>(
    reader: &mut R,
    metadata: &Metadata,
    file_info: &FileInfo,
    limits: ParseLimits,
    edits: &[PsdEdit],
) -> Result<Vec<Patch>> {
    let mut patches = Vec::with_capacity(edits.len());
    for edit in edits {
        let replacement = match edit {
            PsdEdit::SetXmp(value) => {
                let replacement = value.as_bytes().to_vec();
                if replacement.len() > limits.max_value_bytes {
                    return Err(MetraError::ResourceLimitExceeded {
                        resource: "PSD XMP packet".to_owned(),
                        limit: limits.max_value_bytes,
                    });
                }
                validate_xmp(&replacement, limits)?;
                Some(replacement)
            }
            PsdEdit::DeleteXmp => None,
        };
        let tag = match metadata.find_all("XMP:Packet").as_slice() {
            [tag] => *tag,
            [] => {
                return Err(MetraError::WriteFailure {
                    message: "PSD XMP resource does not exist".to_owned(),
                });
            }
            _ => {
                return Err(MetraError::WriteFailure {
                    message:
                        "PSD contains repeated XMP resources; an unambiguous target is required"
                            .to_owned(),
                });
            }
        };
        if tag.source.container != "PSD/XMP" {
            return Err(MetraError::WriteFailure {
                message: "PSD XMP tag does not reference an image resource".to_owned(),
            });
        }
        if !matches!(tag.value, TagValue::Bytes(_)) {
            return Err(MetraError::WriteFailure {
                message: "PSD XMP resource is not a byte packet".to_owned(),
            });
        }
        let offset = tag.source.offset.ok_or_else(|| MetraError::WriteFailure {
            message: "PSD XMP resource has no source offset".to_owned(),
        })?;
        let span = tag.source.length.ok_or_else(|| MetraError::WriteFailure {
            message: "PSD XMP resource has no source length".to_owned(),
        })?;
        let span_usize = usize::try_from(span).map_err(|_| MetraError::ResourceLimitExceeded {
            resource: "PSD XMP packet".to_owned(),
            limit: limits.max_value_bytes,
        })?;
        if span_usize > limits.max_value_bytes {
            return Err(MetraError::ResourceLimitExceeded {
                resource: "PSD XMP packet".to_owned(),
                limit: limits.max_value_bytes,
            });
        }
        let original = read_at(
            reader,
            offset,
            span_usize,
            file_info.size,
            &file_info.path,
            "PSD XMP packet",
        )?;
        let replacement = replacement.unwrap_or_else(|| vec![0_u8; original.len()]);
        if replacement.len() != original.len() {
            return Err(MetraError::WriteFailure {
                message: format!(
                    "PSD XMP replacement requires a fixed length ({} bytes available, {} needed)",
                    original.len(),
                    replacement.len()
                ),
            });
        }
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
                    context: "PSD rewrite patch".to_owned(),
                    offset: pair[0].offset,
                })?;
        if previous_end > pair[1].offset {
            return Err(MetraError::WriteFailure {
                message: "PSD rewrite patches overlap".to_owned(),
            });
        }
    }
    Ok(patches)
}

fn validate_xmp(bytes: &[u8], limits: ParseLimits) -> Result<()> {
    let mut metadata = Metadata::new(FileInfo::new(
        "<memory>".into(),
        bytes.len() as u64,
        FileFormat::Psd,
    ));
    parse_xmp(bytes, 0, "PSD/XMP", &mut metadata, limits)
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
                message: "PSD rewrite patches are not ordered".to_owned(),
            });
        }
        copy_exact(reader, writer, patch.offset - cursor, path)?;
        let patch_end = patch
            .offset
            .checked_add(patch.span)
            .ok_or(MetraError::InvalidOffset {
                context: "PSD rewrite cursor".to_owned(),
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
        .unwrap_or("document.psd");
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

    fn resource(id: u16, bytes: &[u8]) -> Vec<u8> {
        let mut output = b"8BIM".to_vec();
        output.extend_from_slice(&id.to_be_bytes());
        output.extend_from_slice(&[0, 0]);
        output.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        output.extend_from_slice(bytes);
        if bytes.len() % 2 == 1 {
            output.push(0);
        }
        output
    }

    fn psd(resources: &[u8]) -> Vec<u8> {
        let mut output = vec![0_u8; 26];
        output[..4].copy_from_slice(b"8BPS");
        output[4..6].copy_from_slice(&1_u16.to_be_bytes());
        output[12..14].copy_from_slice(&3_u16.to_be_bytes());
        output[14..18].copy_from_slice(&100_u32.to_be_bytes());
        output[18..22].copy_from_slice(&200_u32.to_be_bytes());
        output[22..24].copy_from_slice(&8_u16.to_be_bytes());
        output[24..26].copy_from_slice(&3_u16.to_be_bytes());
        output.extend_from_slice(&0_u32.to_be_bytes());
        output.extend_from_slice(&(resources.len() as u32).to_be_bytes());
        output.extend_from_slice(resources);
        output.extend_from_slice(&0_u32.to_be_bytes());
        output.extend_from_slice(&0_u16.to_be_bytes());
        output
    }

    fn packet(format: &str) -> Vec<u8> {
        format!(
            "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF><rdf:Description xmlns:dc=\"urn:dc\" dc:format=\"{format}\"/></rdf:RDF></x:xmpmeta>"
        )
        .into_bytes()
    }

    fn info(bytes: &[u8]) -> FileInfo {
        FileInfo::new("editable.psd".into(), bytes.len() as u64, FileFormat::Psd)
    }

    #[test]
    fn rewrites_existing_xmp_resource_without_changing_psd_layout() {
        let bytes = psd(&resource(0x0424, &packet("old")));
        let output = rewrite_psd_to_vec(
            &bytes,
            info(&bytes),
            ParseLimits::default(),
            &[PsdEdit::SetXmp(
                String::from_utf8(packet("new")).expect("XMP fixture should be UTF-8"),
            )],
        )
        .unwrap();
        assert_eq!(output.len(), bytes.len());
        let metadata = read_psd(
            &mut Cursor::new(output),
            info(&bytes),
            ParseLimits::default(),
        )
        .unwrap();
        assert_eq!(
            metadata.find("XMP:dc:format").unwrap().display_value(),
            "new"
        );
    }

    #[test]
    fn rejects_psd_xmp_growth_before_writing() {
        let bytes = psd(&resource(0x0424, &packet("old")));
        let error = rewrite_psd_to_vec(
            &bytes,
            info(&bytes),
            ParseLimits::default(),
            &[PsdEdit::SetXmp(
                String::from_utf8(packet("a-longer-value")).expect("XMP fixture should be UTF-8"),
            )],
        )
        .unwrap_err();
        assert!(error.to_string().contains("fixed length"));
    }

    #[test]
    fn deletes_existing_xmp_resource_without_changing_psd_layout() {
        let bytes = psd(&resource(0x0424, &packet("old")));
        let output = rewrite_psd_to_vec(
            &bytes,
            info(&bytes),
            ParseLimits::default(),
            &[PsdEdit::DeleteXmp],
        )
        .unwrap();
        assert_eq!(output.len(), bytes.len());
        assert!(
            output
                .windows(packet("old").len())
                .any(|window| { window.iter().all(|byte| *byte == 0) })
        );
        let metadata = read_psd(
            &mut Cursor::new(output),
            info(&bytes),
            ParseLimits::default(),
        )
        .unwrap();
        assert!(metadata.find("XMP:Packet").is_none());
        assert!(metadata.warnings.is_empty());
    }
}
