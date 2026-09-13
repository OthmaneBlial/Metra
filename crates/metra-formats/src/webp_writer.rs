use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use metra_core::{FileFormat, FileInfo, Metadata, MetraError, ParseLimits, Result};

use crate::atomic::atomic_replace;
use crate::webp::read_webp;
use crate::xmp::parse_xmp;

/// Lossless WebP XMP chunk edits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebpEdit {
    SetXmp(String),
    DeleteXmp,
}

pub fn rewrite_webp<R: Read + Seek, W: Write + Seek>(
    reader: &mut R,
    writer: &mut W,
    file_info: FileInfo,
    limits: ParseLimits,
    edits: &[WebpEdit],
) -> Result<()> {
    read_webp(reader, file_info.clone(), limits)?;
    reader
        .seek(SeekFrom::Start(0))
        .map_err(|source| io_error(&file_info.path, source))?;
    rewrite_webp_stream(reader, writer, &file_info.path, limits, edits)
}

pub fn rewrite_webp_to_vec(
    bytes: &[u8],
    file_info: FileInfo,
    limits: ParseLimits,
    edits: &[WebpEdit],
) -> Result<Vec<u8>> {
    let mut reader = std::io::Cursor::new(bytes);
    let mut output = std::io::Cursor::new(Vec::new());
    rewrite_webp(&mut reader, &mut output, file_info.clone(), limits, edits)?;
    let output = output.into_inner();
    let output_size = output.len() as u64;
    read_webp(
        &mut std::io::Cursor::new(output.as_slice()),
        FileInfo::new(file_info.path, output_size, FileFormat::Webp),
        limits,
    )?;
    Ok(output)
}

pub fn rewrite_webp_path(
    path: impl AsRef<Path>,
    limits: ParseLimits,
    edits: &[WebpEdit],
) -> Result<()> {
    let path = path.as_ref().to_path_buf();
    let source_metadata = fs::metadata(&path).map_err(|source| MetraError::Io {
        path: path.clone(),
        source,
    })?;
    let file_info = FileInfo::new(path.clone(), source_metadata.len(), FileFormat::Webp);
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
        rewrite_webp(&mut input, &mut output, file_info.clone(), limits, edits)?;
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
        read_webp(
            &mut validation,
            FileInfo::new(temp_path.clone(), written_size, FileFormat::Webp),
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

#[derive(Debug)]
enum XmpAction {
    Set(Vec<u8>),
    Delete,
}

fn rewrite_webp_stream<R: Read, W: Write + Seek>(
    reader: &mut R,
    writer: &mut W,
    path: &Path,
    limits: ParseLimits,
    edits: &[WebpEdit],
) -> Result<()> {
    let action = xmp_action(edits, limits)?;
    let mut header = [0_u8; 12];
    read_exact(reader, &mut header, path)?;
    if &header[..4] != b"RIFF" || &header[8..12] != b"WEBP" {
        return Err(MetraError::InvalidHeader {
            context: "WebP".to_owned(),
            message: "missing RIFF/WEBP header".to_owned(),
        });
    }
    let riff_size = u64::from(u32::from_le_bytes(
        header[4..8].try_into().expect("RIFF size"),
    ));
    let declared_end = riff_size.checked_add(8).ok_or(MetraError::InvalidOffset {
        context: "WebP RIFF boundary".to_owned(),
        offset: riff_size,
    })?;
    if declared_end < 12 {
        return Err(MetraError::InvalidHeader {
            context: "WebP".to_owned(),
            message: "RIFF body is shorter than the WEBP form type".to_owned(),
        });
    }
    write_all(writer, &header)?;

    let mut offset = 12_u64;
    let mut chunk_count = 0_usize;
    let mut inserted = false;
    while offset < declared_end {
        if chunk_count >= limits.max_jpeg_segments {
            return Err(MetraError::ResourceLimitExceeded {
                resource: "WebP chunks during rewrite".to_owned(),
                limit: limits.max_jpeg_segments,
            });
        }
        if declared_end.saturating_sub(offset) < 8 {
            return Err(MetraError::UnexpectedEof {
                context: "WebP chunk header".to_owned(),
            });
        }
        let mut chunk_header = [0_u8; 8];
        read_exact(reader, &mut chunk_header, path)?;
        let kind: [u8; 4] = chunk_header[..4].try_into().expect("WebP chunk type");
        let length = u64::from(u32::from_le_bytes(
            chunk_header[4..8].try_into().expect("WebP chunk length"),
        ));
        let padded_length = length + (length & 1);
        let chunk_end = offset
            .checked_add(8)
            .and_then(|value| value.checked_add(padded_length))
            .ok_or(MetraError::InvalidOffset {
                context: "WebP chunk end".to_owned(),
                offset,
            })?;
        if chunk_end > declared_end {
            return Err(MetraError::UnexpectedEof {
                context: "WebP chunk".to_owned(),
            });
        }
        if &kind == b"XMP "
            && let Some(action) = action.as_ref()
        {
            let length =
                usize::try_from(length).map_err(|_| MetraError::ResourceLimitExceeded {
                    resource: "WebP XMP chunk".to_owned(),
                    limit: limits.max_metadata_bytes,
                })?;
            if length > limits.max_metadata_bytes {
                return Err(MetraError::ResourceLimitExceeded {
                    resource: "WebP XMP chunk".to_owned(),
                    limit: limits.max_metadata_bytes,
                });
            }
            discard_exact(reader, length as u64, path)?;
            if length & 1 == 1 {
                discard_exact(reader, 1, path)?;
            }
            match action {
                XmpAction::Set(value) if !inserted => {
                    write_xmp_chunk(writer, value)?;
                    inserted = true;
                }
                XmpAction::Set(_) | XmpAction::Delete => {}
            }
        } else {
            write_all(writer, &chunk_header)?;
            copy_exact(reader, writer, padded_length, path)?;
        }
        offset = chunk_end;
        chunk_count += 1;
    }
    if let Some(XmpAction::Set(value)) = action.as_ref()
        && !inserted
    {
        write_xmp_chunk(writer, value)?;
    }
    let riff_end = writer
        .stream_position()
        .map_err(|source| io_error(path, source))?;
    let riff_body_size = riff_end.checked_sub(8).ok_or(MetraError::InvalidOffset {
        context: "WebP output RIFF boundary".to_owned(),
        offset: riff_end,
    })?;
    let riff_body_size = u32::try_from(riff_body_size).map_err(|_| MetraError::WriteFailure {
        message: "rewritten WebP RIFF body exceeds the 32-bit size limit".to_owned(),
    })?;
    writer
        .seek(SeekFrom::Start(4))
        .map_err(|source| io_error(path, source))?;
    write_all(writer, &riff_body_size.to_le_bytes())?;
    writer
        .seek(SeekFrom::Start(riff_end))
        .map_err(|source| io_error(path, source))?;
    copy_remainder(reader, writer, path)
}

fn xmp_action(edits: &[WebpEdit], limits: ParseLimits) -> Result<Option<XmpAction>> {
    let mut action = None;
    for edit in edits {
        action = Some(match edit {
            WebpEdit::SetXmp(value) => {
                let bytes = value.as_bytes().to_vec();
                if bytes.len() > limits.max_value_bytes {
                    return Err(MetraError::ResourceLimitExceeded {
                        resource: "WebP XMP packet".to_owned(),
                        limit: limits.max_value_bytes,
                    });
                }
                let mut validation = Metadata::new(FileInfo::new(
                    PathBuf::from("<memory>"),
                    bytes.len() as u64,
                    FileFormat::Webp,
                ));
                parse_xmp(&bytes, 0, "WebP/XMP", &mut validation, limits)?;
                XmpAction::Set(bytes)
            }
            WebpEdit::DeleteXmp => XmpAction::Delete,
        });
    }
    Ok(action)
}

fn write_xmp_chunk<W: Write>(writer: &mut W, value: &[u8]) -> Result<()> {
    let length = u32::try_from(value.len()).map_err(|_| MetraError::WriteFailure {
        message: "WebP XMP chunk exceeds the 32-bit size limit".to_owned(),
    })?;
    write_all(writer, b"XMP ")?;
    write_all(writer, &length.to_le_bytes())?;
    write_all(writer, value)?;
    if value.len() & 1 == 1 {
        write_all(writer, &[0])?;
    }
    Ok(())
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
        .unwrap_or("metadata.webp");
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

    const XMP_BEFORE: &[u8] = br#"<x:xmpmeta><rdf:RDF><rdf:Description xmlns:dc="urn:dc" dc:format="before"/></rdf:RDF></x:xmpmeta>"#;
    const XMP_AFTER: &[u8] = br#"<x:xmpmeta><rdf:RDF><rdf:Description xmlns:dc="urn:dc" dc:format="after"/></rdf:RDF></x:xmpmeta>"#;

    fn info(size: usize) -> FileInfo {
        FileInfo::new("editable.webp".into(), size as u64, FileFormat::Webp)
    }

    fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut result = kind.to_vec();
        result.extend_from_slice(&(data.len() as u32).to_le_bytes());
        result.extend_from_slice(data);
        if data.len() & 1 == 1 {
            result.push(0);
        }
        result
    }

    fn webp_with_xmp(xmp: &[u8]) -> Vec<u8> {
        let mut body = chunk(b"VP8X", &[0, 0, 0, 0, 1, 0, 0, 1, 0, 0]);
        body.extend(chunk(b"VP8 ", &[1, 2, 3]));
        body.extend(chunk(b"XMP ", xmp));
        let mut bytes = b"RIFF".to_vec();
        bytes.extend_from_slice(&((4 + body.len()) as u32).to_le_bytes());
        bytes.extend_from_slice(b"WEBP");
        bytes.extend(body);
        bytes
    }

    #[test]
    fn replaces_xmp_and_preserves_image_chunks_and_riff_size() {
        let bytes = webp_with_xmp(XMP_BEFORE);
        let output = rewrite_webp_to_vec(
            &bytes,
            info(bytes.len()),
            ParseLimits::default(),
            &[WebpEdit::SetXmp(
                String::from_utf8(XMP_AFTER.to_vec()).unwrap(),
            )],
        )
        .unwrap();
        let metadata = read_webp(
            &mut Cursor::new(output.clone()),
            info(output.len()),
            ParseLimits::default(),
        )
        .unwrap();
        assert_eq!(
            metadata.find("XMP:dc:format").unwrap().display_value(),
            "after"
        );
        assert!(
            output
                .windows(11)
                .any(|window| { window == [b'V', b'P', b'8', b' ', 3, 0, 0, 0, 1, 2, 3] })
        );
        let declared_size = u32::from_le_bytes(output[4..8].try_into().unwrap()) as usize;
        assert_eq!(declared_size + 8, output.len());
    }

    #[test]
    fn deletes_and_inserts_xmp_chunks() {
        let bytes = webp_with_xmp(XMP_BEFORE);
        let deleted = rewrite_webp_to_vec(
            &bytes,
            info(bytes.len()),
            ParseLimits::default(),
            &[WebpEdit::DeleteXmp],
        )
        .unwrap();
        let deleted_metadata = read_webp(
            &mut Cursor::new(deleted.clone()),
            info(deleted.len()),
            ParseLimits::default(),
        )
        .unwrap();
        assert!(deleted_metadata.find("XMP:Packet").is_none());

        let mut without_xmp = b"RIFF".to_vec();
        let body = chunk(b"VP8X", &[0, 0, 0, 0, 1, 0, 0, 1, 0, 0]);
        without_xmp.extend_from_slice(&((4 + body.len()) as u32).to_le_bytes());
        without_xmp.extend_from_slice(b"WEBP");
        without_xmp.extend_from_slice(&body);
        let inserted = rewrite_webp_to_vec(
            &without_xmp,
            info(without_xmp.len()),
            ParseLimits::default(),
            &[WebpEdit::SetXmp(
                String::from_utf8(XMP_AFTER.to_vec()).unwrap(),
            )],
        )
        .unwrap();
        let inserted_metadata = read_webp(
            &mut Cursor::new(inserted),
            info(without_xmp.len() + XMP_AFTER.len() + 12),
            ParseLimits::default(),
        )
        .unwrap();
        assert_eq!(
            inserted_metadata
                .find("XMP:dc:format")
                .unwrap()
                .display_value(),
            "after"
        );
    }

    #[test]
    fn rejects_invalid_xmp_before_writing() {
        let bytes = webp_with_xmp(XMP_BEFORE);
        let error = rewrite_webp_to_vec(
            &bytes,
            info(bytes.len()),
            ParseLimits::default(),
            &[WebpEdit::SetXmp("<broken".to_owned())],
        )
        .unwrap_err();
        assert!(error.to_string().contains("XML"));
    }

    #[test]
    fn path_rewrite_validates_before_atomic_replace() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("metra-webp-rewrite-{unique}"));
        fs::create_dir(&directory).unwrap();
        let path = directory.join("atomic.webp");
        fs::write(&path, webp_with_xmp(XMP_BEFORE)).unwrap();

        rewrite_webp_path(
            &path,
            ParseLimits::default(),
            &[WebpEdit::SetXmp(
                String::from_utf8(XMP_AFTER.to_vec()).unwrap(),
            )],
        )
        .unwrap();
        let rewritten = fs::read(&path).unwrap();
        let metadata = read_webp(
            &mut Cursor::new(rewritten.clone()),
            info(rewritten.len()),
            ParseLimits::default(),
        )
        .unwrap();
        assert_eq!(
            metadata.find("XMP:dc:format").unwrap().display_value(),
            "after"
        );
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        fs::remove_dir_all(directory).unwrap();
    }
}
