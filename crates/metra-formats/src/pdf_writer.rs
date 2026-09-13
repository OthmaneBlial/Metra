use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use metra_core::{FileFormat, FileInfo, Metadata, MetraError, ParseLimits, Result, TagValue};

use crate::atomic::atomic_replace;
use crate::pdf::read_pdf;

/// Lossless edits for existing PDF Info string tokens.
///
/// PDF object and xref creation is deliberately outside this first writer
/// surface. A replacement succeeds only when its encoded string token has the
/// exact same byte span as the existing token, so every xref offset remains
/// valid and unrelated PDF bytes are copied unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PdfEdit {
    SetInfo { name: String, value: String },
}

pub fn rewrite_pdf<R: Read + Seek, W: Write + Seek>(
    reader: &mut R,
    writer: &mut W,
    file_info: FileInfo,
    limits: ParseLimits,
    edits: &[PdfEdit],
) -> Result<()> {
    if file_info.format != FileFormat::Pdf {
        return Err(MetraError::WriteFailure {
            message: format!("PDF writer cannot edit {}", file_info.format),
        });
    }
    let metadata = read_pdf(reader, file_info.clone(), limits)?;
    let patches = collect_patches(reader, &metadata, &file_info, limits, edits)?;
    reader
        .seek(SeekFrom::Start(0))
        .map_err(|source| io_error(&file_info.path, source))?;
    rewrite_stream(reader, writer, file_info.size, &file_info.path, &patches)
}

pub fn rewrite_pdf_to_vec(
    bytes: &[u8],
    file_info: FileInfo,
    limits: ParseLimits,
    edits: &[PdfEdit],
) -> Result<Vec<u8>> {
    let mut reader = std::io::Cursor::new(bytes);
    let mut output = std::io::Cursor::new(Vec::new());
    rewrite_pdf(&mut reader, &mut output, file_info.clone(), limits, edits)?;
    let output = output.into_inner();
    read_pdf(
        &mut std::io::Cursor::new(output.as_slice()),
        FileInfo::new(file_info.path, output.len() as u64, FileFormat::Pdf),
        limits,
    )?;
    Ok(output)
}

pub fn rewrite_pdf_path(
    path: impl AsRef<Path>,
    limits: ParseLimits,
    edits: &[PdfEdit],
) -> Result<()> {
    let path = path.as_ref().to_path_buf();
    let source_metadata = fs::metadata(&path).map_err(|source| MetraError::Io {
        path: path.clone(),
        source,
    })?;
    let file_info = FileInfo::new(path.clone(), source_metadata.len(), FileFormat::Pdf);
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
        rewrite_pdf(&mut input, &mut output, file_info.clone(), limits, edits)?;
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
        read_pdf(
            &mut validation,
            FileInfo::new(temp_path.clone(), written_size, FileFormat::Pdf),
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
    edits: &[PdfEdit],
) -> Result<Vec<Patch>> {
    let mut patches = Vec::with_capacity(edits.len());
    for edit in edits {
        let PdfEdit::SetInfo { name, value } = edit;
        if !is_writable_info_name(name) {
            return Err(MetraError::WriteFailure {
                message: format!("PDF Info field {name} is not writable"),
            });
        }
        if value.contains('\0') {
            return Err(MetraError::WriteFailure {
                message: format!("PDF Info value for {name} cannot contain NUL"),
            });
        }
        let key = format!("PDF:{name}");
        let tag = match metadata.find_all(&key).as_slice() {
            [tag] => *tag,
            [] => {
                return Err(MetraError::WriteFailure {
                    message: format!("PDF Info field {name} does not exist"),
                });
            }
            _ => {
                return Err(MetraError::WriteFailure {
                    message: format!(
                        "PDF Info field {name} is repeated; an unambiguous target is required"
                    ),
                });
            }
        };
        if !matches!(tag.value, TagValue::String(_)) {
            return Err(MetraError::WriteFailure {
                message: format!("PDF Info field {name} is not a string"),
            });
        }
        let offset = tag.source.offset.ok_or_else(|| MetraError::WriteFailure {
            message: format!("PDF Info field {name} has no source offset"),
        })?;
        let span = tag.source.length.ok_or_else(|| MetraError::WriteFailure {
            message: format!("PDF Info field {name} has no source length"),
        })?;
        let span_usize = usize::try_from(span).map_err(|_| MetraError::ResourceLimitExceeded {
            resource: "PDF Info string token".to_owned(),
            limit: limits.max_value_bytes,
        })?;
        if span_usize > limits.max_value_bytes {
            return Err(MetraError::ResourceLimitExceeded {
                resource: "PDF Info string token".to_owned(),
                limit: limits.max_value_bytes,
            });
        }
        let token = read_at(
            reader,
            offset,
            span_usize,
            file_info.size,
            &file_info.path,
            "PDF Info string token",
        )?;
        let replacement = encode_pdf_string(&token, value)?;
        if replacement.len() != token.len() {
            return Err(MetraError::WriteFailure {
                message: format!(
                    "PDF Info field {name} requires a fixed-length replacement ({} bytes available, {} needed)",
                    token.len(),
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
                    context: "PDF rewrite patch".to_owned(),
                    offset: pair[0].offset,
                })?;
        if previous_end > pair[1].offset {
            return Err(MetraError::WriteFailure {
                message: "PDF rewrite patches overlap".to_owned(),
            });
        }
    }
    Ok(patches)
}

fn is_writable_info_name(name: &str) -> bool {
    matches!(
        name,
        "Title"
            | "Author"
            | "Subject"
            | "Keywords"
            | "Creator"
            | "Producer"
            | "CreationDate"
            | "ModifyDate"
    )
}

fn encode_pdf_string(token: &[u8], value: &str) -> Result<Vec<u8>> {
    if token.starts_with(b"(") {
        let old_bytes = decode_literal_string(token).ok_or_else(|| MetraError::WriteFailure {
            message: "PDF Info field uses an invalid literal string token".to_owned(),
        })?;
        let replacement_bytes = if old_bytes.starts_with(&[0xFE, 0xFF]) {
            let mut bytes = vec![0xFE, 0xFF];
            for unit in value.encode_utf16() {
                bytes.extend_from_slice(&unit.to_be_bytes());
            }
            bytes
        } else if value.is_ascii() {
            value.as_bytes().to_vec()
        } else {
            return Err(MetraError::WriteFailure {
                message: "literal PDF Info strings only accept ASCII replacements".to_owned(),
            });
        };
        return Ok(if old_bytes.starts_with(&[0xFE, 0xFF]) {
            encode_binary_literal_string(&replacement_bytes)
        } else {
            encode_literal_string(&replacement_bytes)
        });
    }
    if token.starts_with(b"<") && token.get(1) != Some(&b'<') {
        let old_bytes = decode_hex_string(token).ok_or_else(|| MetraError::WriteFailure {
            message: "PDF Info field uses an invalid hexadecimal string token".to_owned(),
        })?;
        let replacement_bytes = if old_bytes.starts_with(&[0xFE, 0xFF]) {
            let mut bytes = vec![0xFE, 0xFF];
            for unit in value.encode_utf16() {
                bytes.extend_from_slice(&unit.to_be_bytes());
            }
            bytes
        } else if value.is_ascii() {
            value.as_bytes().to_vec()
        } else {
            return Err(MetraError::WriteFailure {
                message: "non-Unicode PDF hex strings only accept ASCII replacements".to_owned(),
            });
        };
        return Ok(encode_hex_string(&replacement_bytes));
    }
    Err(MetraError::WriteFailure {
        message: "PDF Info field is not an editable literal or hexadecimal string".to_owned(),
    })
}

fn decode_literal_string(token: &[u8]) -> Option<Vec<u8>> {
    if token.first() != Some(&b'(') || token.last() != Some(&b')') {
        return None;
    }
    let mut result = Vec::new();
    let mut cursor = 1_usize;
    let mut depth = 1_usize;
    while cursor < token.len() {
        let byte = token[cursor];
        cursor += 1;
        if byte == b'\\' {
            let escaped = *token.get(cursor)?;
            cursor += 1;
            match escaped {
                b'n' => result.push(b'\n'),
                b'r' => result.push(b'\r'),
                b't' => result.push(b'\t'),
                b'b' => result.push(8),
                b'f' => result.push(12),
                b'(' | b')' | b'\\' => result.push(escaped),
                b'\r' => {
                    if token.get(cursor) == Some(&b'\n') {
                        cursor += 1;
                    }
                }
                b'\n' => {}
                b'0'..=b'7' => {
                    let mut value = u16::from(escaped - b'0');
                    for _ in 0..2 {
                        let Some(next @ b'0'..=b'7') = token.get(cursor).copied() else {
                            break;
                        };
                        value = value * 8 + u16::from(next - b'0');
                        cursor += 1;
                    }
                    result.push(value as u8);
                }
                other => result.push(other),
            }
        } else if byte == b'(' {
            depth += 1;
            result.push(byte);
        } else if byte == b')' {
            depth -= 1;
            if depth == 0 {
                return (cursor == token.len()).then_some(result);
            }
            result.push(byte);
        } else {
            result.push(byte);
        }
    }
    None
}

fn encode_literal_string(value: &[u8]) -> Vec<u8> {
    let mut encoded = Vec::with_capacity(value.len() + 2);
    encoded.push(b'(');
    for byte in value {
        match byte {
            b'(' | b')' | b'\\' => {
                encoded.push(b'\\');
                encoded.push(*byte);
            }
            b'\n' => encoded.extend_from_slice(b"\\n"),
            b'\r' => encoded.extend_from_slice(b"\\r"),
            b'\t' => encoded.extend_from_slice(b"\\t"),
            8 => encoded.extend_from_slice(b"\\b"),
            12 => encoded.extend_from_slice(b"\\f"),
            0..=31 | 127 => {
                encoded.push(b'\\');
                encoded.push(b'0' + (*byte >> 6));
                encoded.push(b'0' + ((*byte >> 3) & 7));
                encoded.push(b'0' + (*byte & 7));
            }
            _ => encoded.push(*byte),
        }
    }
    encoded.push(b')');
    encoded
}

fn encode_binary_literal_string(value: &[u8]) -> Vec<u8> {
    let mut encoded = Vec::with_capacity(value.len() + 2);
    encoded.push(b'(');
    for byte in value {
        match byte {
            b'(' | b')' | b'\\' => {
                encoded.push(b'\\');
                encoded.push(*byte);
            }
            _ => encoded.push(*byte),
        }
    }
    encoded.push(b')');
    encoded
}

fn decode_hex_string(token: &[u8]) -> Option<Vec<u8>> {
    if token.first() != Some(&b'<') || token.last() != Some(&b'>') {
        return None;
    }
    let mut bytes = Vec::new();
    let mut high = None;
    for byte in &token[1..token.len() - 1] {
        if byte.is_ascii_whitespace() {
            continue;
        }
        let nibble = hex_nibble(*byte)?;
        if let Some(high_nibble) = high.take() {
            bytes.push((high_nibble << 4) | nibble);
        } else {
            high = Some(nibble);
        }
    }
    if let Some(high_nibble) = high {
        bytes.push(high_nibble << 4);
    }
    Some(bytes)
}

fn encode_hex_string(bytes: &[u8]) -> Vec<u8> {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut token = Vec::with_capacity(bytes.len() * 2 + 2);
    token.push(b'<');
    for byte in bytes {
        token.push(HEX[usize::from(*byte >> 4)]);
        token.push(HEX[usize::from(*byte & 0x0F)]);
    }
    token.push(b'>');
    token
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
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
                message: "PDF rewrite patches are not ordered".to_owned(),
            });
        }
        copy_exact(reader, writer, patch.offset - cursor, path)?;
        let patch_end = patch
            .offset
            .checked_add(patch.span)
            .ok_or(MetraError::InvalidOffset {
                context: "PDF rewrite cursor".to_owned(),
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
        .unwrap_or("metadata.pdf");
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

    fn pdf_with_info(title: &str) -> Vec<u8> {
        format!(
            "%PDF-1.7\n5 0 obj\n<< /Title ({title}) /Author <FEFF004F0074> >>\nendobj\ntrailer\n<< /Info 5 0 R >>\nstartxref\n9\n%%EOF\n"
        )
        .into_bytes()
    }

    fn info(bytes: &[u8]) -> FileInfo {
        FileInfo::new("editable.pdf".into(), bytes.len() as u64, FileFormat::Pdf)
    }

    #[test]
    fn rewrites_fixed_length_pdf_info_strings_without_changing_layout() {
        let bytes = pdf_with_info("Before");
        let output = rewrite_pdf_to_vec(
            &bytes,
            info(&bytes),
            ParseLimits::default(),
            &[PdfEdit::SetInfo {
                name: "Title".to_owned(),
                value: "After!".to_owned(),
            }],
        )
        .unwrap();
        assert_eq!(output.len(), bytes.len());
        let metadata = read_pdf(
            &mut Cursor::new(output),
            info(&bytes),
            ParseLimits::default(),
        )
        .unwrap();
        assert_eq!(
            metadata.find("PDF:Title").unwrap().display_value(),
            "After!"
        );
    }

    #[test]
    fn rewrites_utf16_pdf_hex_strings() {
        let bytes = pdf_with_info("Before");
        let output = rewrite_pdf_to_vec(
            &bytes,
            info(&bytes),
            ParseLimits::default(),
            &[PdfEdit::SetInfo {
                name: "Author".to_owned(),
                value: "Li".to_owned(),
            }],
        )
        .unwrap();
        let metadata = read_pdf(
            &mut Cursor::new(output),
            info(&bytes),
            ParseLimits::default(),
        )
        .unwrap();
        assert_eq!(metadata.find("PDF:Author").unwrap().display_value(), "Li");
    }

    #[test]
    fn rewrites_utf16_pdf_literal_strings() {
        let mut bytes = b"%PDF-1.7\n5 0 obj\n<< /Title (".to_vec();
        bytes.extend_from_slice(&[
            0xFE, 0xFF, 0x00, b'B', 0x00, b'e', 0x00, b'f', 0x00, b'o', 0x00, b'r', 0x00, b'e',
        ]);
        bytes.extend_from_slice(b") >>\nendobj\ntrailer\n<< /Info 5 0 R >>\nstartxref\n9\n%%EOF\n");
        let output = rewrite_pdf_to_vec(
            &bytes,
            info(&bytes),
            ParseLimits::default(),
            &[PdfEdit::SetInfo {
                name: "Title".to_owned(),
                value: "After!".to_owned(),
            }],
        )
        .unwrap();
        let metadata = read_pdf(
            &mut Cursor::new(output),
            info(&bytes),
            ParseLimits::default(),
        )
        .unwrap();
        assert_eq!(
            metadata.find("PDF:Title").unwrap().display_value(),
            "After!"
        );
    }

    #[test]
    fn rejects_pdf_info_replacements_that_change_token_size() {
        let bytes = pdf_with_info("Before");
        let error = rewrite_pdf_to_vec(
            &bytes,
            info(&bytes),
            ParseLimits::default(),
            &[PdfEdit::SetInfo {
                name: "Title".to_owned(),
                value: "Longer!".to_owned(),
            }],
        )
        .unwrap_err();
        assert!(error.to_string().contains("fixed-length"));
    }
}
