use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use metra_core::{FileFormat, FileInfo, Metadata, MetraError, ParseLimits, Result, TagValue};

use crate::atomic::atomic_replace;
use crate::xmp::{parse_xmp, read_xmp};

/// Lossless replacement for an existing standalone XMP packet.
///
/// A packet is a complete XML document, so the first writer surface preserves
/// the source byte layout by requiring an exactly equal replacement length.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum XmpEdit {
    SetPacket(String),
    /// Clear XMP properties while retaining a valid empty packet envelope.
    DeletePacket,
}

pub fn rewrite_xmp<R: Read + Seek, W: Write + Seek>(
    reader: &mut R,
    writer: &mut W,
    file_info: FileInfo,
    limits: ParseLimits,
    edits: &[XmpEdit],
) -> Result<()> {
    if file_info.format != FileFormat::Xmp {
        return Err(MetraError::WriteFailure {
            message: format!("XMP writer cannot edit {}", file_info.format),
        });
    }
    let metadata = read_xmp(reader, file_info.clone(), limits)?;
    let replacement = collect_replacement(&metadata, &file_info, limits, edits)?;
    if replacement.len() != usize::try_from(file_info.size).unwrap_or(usize::MAX) {
        return Err(MetraError::WriteFailure {
            message: format!(
                "standalone XMP replacement requires a fixed length ({} bytes available, {} needed)",
                file_info.size,
                replacement.len()
            ),
        });
    }
    writer
        .seek(SeekFrom::Start(0))
        .map_err(|source| write_io_error(&file_info.path, source))?;
    writer
        .write_all(&replacement)
        .map_err(|source| write_io_error(&file_info.path, source))
}

pub fn rewrite_xmp_to_vec(
    bytes: &[u8],
    file_info: FileInfo,
    limits: ParseLimits,
    edits: &[XmpEdit],
) -> Result<Vec<u8>> {
    let mut reader = std::io::Cursor::new(bytes);
    let mut output = std::io::Cursor::new(Vec::with_capacity(bytes.len()));
    rewrite_xmp(&mut reader, &mut output, file_info.clone(), limits, edits)?;
    let output = output.into_inner();
    read_xmp(
        &mut std::io::Cursor::new(output.as_slice()),
        FileInfo::new(file_info.path, output.len() as u64, FileFormat::Xmp),
        limits,
    )?;
    Ok(output)
}

pub fn rewrite_xmp_path(
    path: impl AsRef<Path>,
    limits: ParseLimits,
    edits: &[XmpEdit],
) -> Result<()> {
    let path = path.as_ref().to_path_buf();
    let source_metadata = fs::metadata(&path).map_err(|source| MetraError::Io {
        path: path.clone(),
        source,
    })?;
    let file_info = FileInfo::new(path.clone(), source_metadata.len(), FileFormat::Xmp);
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
        rewrite_xmp(&mut input, &mut output, file_info.clone(), limits, edits)?;
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
        read_xmp(
            &mut validation,
            FileInfo::new(temp_path.clone(), written_size, FileFormat::Xmp),
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

fn collect_replacement(
    metadata: &Metadata,
    file_info: &FileInfo,
    limits: ParseLimits,
    edits: &[XmpEdit],
) -> Result<Vec<u8>> {
    if !matches!(edits, [XmpEdit::SetPacket(_) | XmpEdit::DeletePacket]) {
        return Err(MetraError::WriteFailure {
            message: "standalone XMP requires exactly one packet replacement".to_owned(),
        });
    }
    let replacement = match edits {
        [XmpEdit::SetPacket(packet)] => packet.as_bytes().to_vec(),
        [XmpEdit::DeletePacket] => empty_packet(file_info.size, limits)?,
        _ => unreachable!("validated standalone XMP edit shape"),
    };
    if replacement.len() > limits.max_value_bytes {
        return Err(MetraError::ResourceLimitExceeded {
            resource: "standalone XMP packet".to_owned(),
            limit: limits.max_value_bytes,
        });
    }
    let mut validation = Metadata::new(FileInfo::new(
        "<memory>".into(),
        replacement.len() as u64,
        FileFormat::Xmp,
    ));
    parse_xmp(&replacement, 0, "XMP/rewrite", &mut validation, limits)?;
    let tag = metadata
        .find("XMP:Packet")
        .ok_or_else(|| MetraError::WriteFailure {
            message: "standalone XMP packet does not exist".to_owned(),
        })?;
    if !matches!(tag.value, TagValue::Bytes(_)) {
        return Err(MetraError::WriteFailure {
            message: "standalone XMP packet is not a byte packet".to_owned(),
        });
    }
    let source_length = tag.source.length.ok_or_else(|| MetraError::WriteFailure {
        message: "standalone XMP packet has no source length".to_owned(),
    })?;
    if source_length != file_info.size {
        return Err(MetraError::WriteFailure {
            message: "standalone XMP source length is inconsistent".to_owned(),
        });
    }
    Ok(replacement)
}

fn empty_packet(source_length: u64, limits: ParseLimits) -> Result<Vec<u8>> {
    const EMPTY_PACKET: &[u8] = br#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"/></x:xmpmeta>"#;
    let source_length = usize::try_from(source_length).map_err(|_| MetraError::WriteFailure {
        message: "standalone XMP source is too large to rewrite".to_owned(),
    })?;
    if source_length < EMPTY_PACKET.len() {
        return Err(MetraError::WriteFailure {
            message: format!(
                "standalone XMP packet is too short for an empty RDF packet ({} bytes needed, {} available)",
                EMPTY_PACKET.len(),
                source_length
            ),
        });
    }
    if source_length > limits.max_value_bytes {
        return Err(MetraError::ResourceLimitExceeded {
            resource: "standalone XMP packet".to_owned(),
            limit: limits.max_value_bytes,
        });
    }
    let mut packet = Vec::with_capacity(source_length);
    packet.extend_from_slice(EMPTY_PACKET);
    packet.resize(source_length, b' ');
    Ok(packet)
}

fn temporary_path(path: &Path) -> Result<PathBuf> {
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("metadata.xmp");
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

fn write_io_error(path: &Path, source: std::io::Error) -> MetraError {
    MetraError::WriteFailure {
        message: format!("{}: {source}", path.display()),
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    fn packet(value: &str) -> Vec<u8> {
        format!(
            "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF><rdf:Description xmlns:dc=\"urn:dc\" dc:format=\"{value}\"/></rdf:RDF></x:xmpmeta>"
        )
        .into_bytes()
    }

    fn info(bytes: &[u8]) -> FileInfo {
        FileInfo::new("editable.xmp".into(), bytes.len() as u64, FileFormat::Xmp)
    }

    #[test]
    fn rewrites_standalone_packet_without_changing_length() {
        let bytes = packet("old");
        let output = rewrite_xmp_to_vec(
            &bytes,
            info(&bytes),
            ParseLimits::default(),
            &[XmpEdit::SetPacket(
                String::from_utf8(packet("new")).expect("packet should be UTF-8"),
            )],
        )
        .unwrap();
        assert_eq!(output.len(), bytes.len());
        let metadata = read_xmp(
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
    fn rejects_standalone_packet_growth_before_writing() {
        let bytes = packet("old");
        let error = rewrite_xmp_to_vec(
            &bytes,
            info(&bytes),
            ParseLimits::default(),
            &[XmpEdit::SetPacket(
                String::from_utf8(packet("a-longer-value")).expect("packet should be UTF-8"),
            )],
        )
        .unwrap_err();
        assert!(error.to_string().contains("fixed length"));
    }

    #[test]
    fn deletes_standalone_properties_with_an_empty_padded_packet() {
        let bytes = packet("old");
        let output = rewrite_xmp_to_vec(
            &bytes,
            info(&bytes),
            ParseLimits::default(),
            &[XmpEdit::DeletePacket],
        )
        .unwrap();
        assert_eq!(output.len(), bytes.len());
        let metadata = read_xmp(
            &mut Cursor::new(output),
            info(&bytes),
            ParseLimits::default(),
        )
        .unwrap();
        assert!(metadata.find("XMP:dc:format").is_none());
        assert!(metadata.find("XMP:Packet").is_some());
    }
}
