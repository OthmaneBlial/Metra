use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use metra_core::{FileFormat, FileInfo, Metadata, MetraError, ParseLimits, Result};

use crate::atomic::atomic_replace;
use crate::png::{PNG_SIGNATURE, crc32, read_png};
use crate::xmp::parse_xmp;

/// Lossless PNG metadata edits for fixed textual, timestamp, and resolution chunks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PngEdit {
    SetText {
        keyword: String,
        value: String,
    },
    DeleteText {
        keyword: String,
    },
    SetXmp(String),
    DeleteXmp,
    /// Replace or insert the PNG `tIME` chunk using `YYYY-MM-DD HH:MM:SS`.
    SetTime(String),
    /// Remove every PNG `tIME` chunk.
    DeleteTime,
    /// Set the horizontal pixels-per-unit value in `pHYs`.
    SetPhysX(String),
    /// Set the vertical pixels-per-unit value in `pHYs`.
    SetPhysY(String),
    /// Set the `pHYs` unit to `meter`, `unknown`, `0`, or `1`.
    SetPhysUnit(String),
    /// Remove every PNG `pHYs` chunk.
    DeletePhys,
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
enum TextAction {
    Set { keyword: Vec<u8>, value: Vec<u8> },
    Delete { keyword: Vec<u8> },
    SetXmp(Vec<u8>),
    DeleteXmp,
}

#[derive(Debug)]
enum TimeAction {
    Set(Vec<u8>),
    Delete,
}

#[derive(Debug)]
enum PhysAction {
    Set {
        x: Option<u32>,
        y: Option<u32>,
        unit: Option<u8>,
    },
    Delete,
}

fn rewrite_png_stream<R: Read, W: Write>(
    reader: &mut R,
    writer: &mut W,
    path: &Path,
    limits: ParseLimits,
    edits: &[PngEdit],
) -> Result<()> {
    let action = text_action(edits, limits)?;
    let time_action = time_action(edits)?;
    let phys_action = phys_action(edits)?;
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
    let mut time_inserted = false;
    let mut phys_inserted = false;

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
                write_replacement_chunk(writer, action, &mut inserted)?;
            } else {
                write_all(writer, &header)?;
                write_all(writer, &data)?;
                write_all(writer, &crc)?;
            }
        } else if &chunk_type == b"iTXt" {
            let length =
                usize::try_from(data_length).map_err(|_| MetraError::ResourceLimitExceeded {
                    resource: "PNG iTXt chunk during rewrite".to_owned(),
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
            if let Some(action) = action.as_ref()
                && itxt_keyword(&data).is_some_and(|keyword| action_matches(action, keyword))
            {
                write_replacement_chunk(writer, action, &mut inserted)?;
            } else {
                write_all(writer, &header)?;
                write_all(writer, &data)?;
                write_all(writer, &crc)?;
            }
        } else if &chunk_type == b"tIME" {
            if let Some(action) = time_action.as_ref() {
                copy_exact(&mut *reader, &mut std::io::sink(), data_length, path)?;
                copy_exact(&mut *reader, &mut std::io::sink(), 4, path)?;
                if !time_inserted {
                    write_time_action(writer, action)?;
                    time_inserted = true;
                }
            } else {
                write_all(writer, &header)?;
                copy_exact(reader, writer, data_length, path)?;
                copy_exact(reader, writer, 4, path)?;
            }
        } else if &chunk_type == b"pHYs" {
            if let Some(action) = phys_action.as_ref() {
                let mut data = [0_u8; 9];
                if data_length == 9 {
                    read_exact(reader, &mut data, path)?;
                } else {
                    copy_exact(&mut *reader, &mut std::io::sink(), data_length, path)?;
                }
                copy_exact(&mut *reader, &mut std::io::sink(), 4, path)?;
                if !phys_inserted {
                    write_phys_action(writer, action, data_length == 9, &mut data)?;
                    phys_inserted = true;
                }
            } else {
                write_all(writer, &header)?;
                copy_exact(reader, writer, data_length, path)?;
                copy_exact(reader, writer, 4, path)?;
            }
        } else if &chunk_type == b"IEND" {
            if let Some(action) = action.as_ref()
                && !inserted
                && action_is_set(action)
            {
                write_action_chunk(writer, action)?;
            }
            if let Some(action) = time_action.as_ref()
                && !time_inserted
                && matches!(action, TimeAction::Set(_))
            {
                write_time_action(writer, action)?;
            }
            if let Some(action) = phys_action.as_ref()
                && !phys_inserted
                && matches!(action, PhysAction::Set { .. })
            {
                write_phys_action(writer, action, false, &mut [0; 9])?;
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

fn time_action(edits: &[PngEdit]) -> Result<Option<TimeAction>> {
    let mut action = None;
    for edit in edits {
        match edit {
            PngEdit::SetTime(value) => action = Some(TimeAction::Set(parse_png_time(value)?)),
            PngEdit::DeleteTime => action = Some(TimeAction::Delete),
            _ => {}
        }
    }
    Ok(action)
}

fn phys_action(edits: &[PngEdit]) -> Result<Option<PhysAction>> {
    let mut action = None;
    for edit in edits {
        match edit {
            PngEdit::SetPhysX(value) => {
                let x = parse_phys_value(value, "PixelsPerUnitX")?;
                action = Some(match action {
                    Some(PhysAction::Set { y, unit, .. }) => PhysAction::Set {
                        x: Some(x),
                        y,
                        unit,
                    },
                    Some(PhysAction::Delete) | None => PhysAction::Set {
                        x: Some(x),
                        y: None,
                        unit: None,
                    },
                });
            }
            PngEdit::SetPhysY(value) => {
                let y = parse_phys_value(value, "PixelsPerUnitY")?;
                action = Some(match action {
                    Some(PhysAction::Set { x, unit, .. }) => PhysAction::Set {
                        x,
                        y: Some(y),
                        unit,
                    },
                    Some(PhysAction::Delete) | None => PhysAction::Set {
                        x: None,
                        y: Some(y),
                        unit: None,
                    },
                });
            }
            PngEdit::SetPhysUnit(value) => {
                let unit = parse_phys_unit(value)?;
                action = Some(match action {
                    Some(PhysAction::Set { x, y, .. }) => PhysAction::Set {
                        x,
                        y,
                        unit: Some(unit),
                    },
                    Some(PhysAction::Delete) | None => PhysAction::Set {
                        x: None,
                        y: None,
                        unit: Some(unit),
                    },
                });
            }
            PngEdit::DeletePhys => action = Some(PhysAction::Delete),
            _ => {}
        }
    }
    Ok(action)
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
            PngEdit::SetXmp(value) => {
                action = Some(TextAction::SetXmp(validate_xmp(value, limits)?));
            }
            PngEdit::DeleteXmp => action = Some(TextAction::DeleteXmp),
            PngEdit::SetTime(_)
            | PngEdit::DeleteTime
            | PngEdit::SetPhysX(_)
            | PngEdit::SetPhysY(_)
            | PngEdit::SetPhysUnit(_)
            | PngEdit::DeletePhys => {}
        }
    }
    Ok(action)
}

fn parse_png_time(value: &str) -> Result<Vec<u8>> {
    let bytes = value.as_bytes();
    if bytes.len() != 19
        || !matches!(bytes[4], b'-' | b':')
        || bytes[7] != bytes[4]
        || bytes[10] != b' '
        || bytes[13] != b':'
        || bytes[16] != b':'
    {
        return Err(MetraError::InvalidTag {
            context: "PNG tIME".to_owned(),
            message: "expected YYYY-MM-DD HH:MM:SS".to_owned(),
        });
    }
    let year = parse_decimal(&bytes[..4])?;
    let month = parse_decimal(&bytes[5..7])?;
    let day = parse_decimal(&bytes[8..10])?;
    let hour = parse_decimal(&bytes[11..13])?;
    let minute = parse_decimal(&bytes[14..16])?;
    let second = parse_decimal(&bytes[17..19])?;
    if !(1..=12).contains(&month)
        || !(1..=days_in_month(year, month)).contains(&day)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return Err(MetraError::InvalidTag {
            context: "PNG tIME".to_owned(),
            message: "date or time components are out of range".to_owned(),
        });
    }
    Ok(vec![
        (year >> 8) as u8,
        year as u8,
        month as u8,
        day as u8,
        hour as u8,
        minute as u8,
        second as u8,
    ])
}

fn parse_decimal(bytes: &[u8]) -> Result<u16> {
    if bytes.is_empty() || bytes.len() > 4 || !bytes.iter().all(|byte| byte.is_ascii_digit()) {
        return Err(MetraError::InvalidTag {
            context: "PNG tIME".to_owned(),
            message: "date and time components must be decimal digits".to_owned(),
        });
    }
    bytes.iter().try_fold(0_u16, |value, byte| {
        value
            .checked_mul(10)
            .and_then(|value| value.checked_add(u16::from(byte - b'0')))
            .ok_or_else(|| MetraError::InvalidTag {
                context: "PNG tIME".to_owned(),
                message: "year is outside the PNG 16-bit range".to_owned(),
            })
    })
}

fn days_in_month(year: u16, month: u16) -> u16 {
    match month {
        2 if year.is_multiple_of(400) || (year.is_multiple_of(4) && !year.is_multiple_of(100)) => {
            29
        }
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

fn parse_phys_value(value: &str, field: &str) -> Result<u32> {
    value.parse::<u32>().map_err(|_| MetraError::InvalidTag {
        context: format!("PNG pHYs {field}"),
        message: "pixels-per-unit values must be unsigned 32-bit integers".to_owned(),
    })
}

fn parse_phys_unit(value: &str) -> Result<u8> {
    match value.to_ascii_lowercase().as_str() {
        "meter" | "metre" | "1" => Ok(1),
        "unknown" | "0" => Ok(0),
        _ => Err(MetraError::InvalidTag {
            context: "PNG pHYs Unit".to_owned(),
            message: "unit must be meter, unknown, 0, or 1".to_owned(),
        }),
    }
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

fn validate_xmp(value: &str, limits: ParseLimits) -> Result<Vec<u8>> {
    let bytes = value.as_bytes().to_vec();
    if bytes.len() > limits.max_value_bytes {
        return Err(MetraError::ResourceLimitExceeded {
            resource: "PNG XMP packet".to_owned(),
            limit: limits.max_value_bytes,
        });
    }
    let mut validation = Metadata::new(FileInfo::new(
        "<memory>".into(),
        bytes.len() as u64,
        FileFormat::Png,
    ));
    parse_xmp(&bytes, 0, "PNG/XMP", &mut validation, limits)?;
    Ok(bytes)
}

fn text_keyword(data: &[u8]) -> Option<&[u8]> {
    let separator = data.iter().position(|byte| *byte == 0)?;
    Some(&data[..separator])
}

fn itxt_keyword(data: &[u8]) -> Option<&[u8]> {
    data.get(..data.iter().position(|byte| *byte == 0)?)
}

fn action_matches(action: &TextAction, keyword: &[u8]) -> bool {
    match action {
        TextAction::Set {
            keyword: target, ..
        }
        | TextAction::Delete { keyword: target } => target == keyword,
        TextAction::SetXmp(_) | TextAction::DeleteXmp => keyword == b"XML:com.adobe.xmp",
    }
}

fn action_is_set(action: &TextAction) -> bool {
    matches!(action, TextAction::Set { .. } | TextAction::SetXmp(_))
}

fn write_replacement_chunk<W: Write>(
    writer: &mut W,
    action: &TextAction,
    inserted: &mut bool,
) -> Result<()> {
    if *inserted {
        return Ok(());
    }
    match action {
        TextAction::Set { keyword, value } => write_text_chunk(writer, keyword, value)?,
        TextAction::SetXmp(value) => write_itxt_xmp_chunk(writer, value)?,
        TextAction::Delete { .. } | TextAction::DeleteXmp => {}
    }
    *inserted = true;
    Ok(())
}

fn write_action_chunk<W: Write>(writer: &mut W, action: &TextAction) -> Result<()> {
    match action {
        TextAction::Set { keyword, value } => write_text_chunk(writer, keyword, value),
        TextAction::SetXmp(value) => write_itxt_xmp_chunk(writer, value),
        TextAction::Delete { .. } | TextAction::DeleteXmp => Ok(()),
    }
}

fn write_time_action<W: Write>(writer: &mut W, action: &TimeAction) -> Result<()> {
    if let TimeAction::Set(value) = action {
        write_png_chunk(writer, b"tIME", value)?;
    }
    Ok(())
}

fn write_phys_action<W: Write>(
    writer: &mut W,
    action: &PhysAction,
    had_valid_chunk: bool,
    data: &mut [u8; 9],
) -> Result<()> {
    match action {
        PhysAction::Set { x, y, unit } => {
            if !had_valid_chunk {
                *data = [0; 9];
            }
            if let Some(x) = x {
                data[..4].copy_from_slice(&x.to_be_bytes());
            }
            if let Some(y) = y {
                data[4..8].copy_from_slice(&y.to_be_bytes());
            }
            if let Some(unit) = unit {
                data[8] = *unit;
            }
            write_png_chunk(writer, b"pHYs", data)
        }
        PhysAction::Delete => Ok(()),
    }
}

fn write_png_chunk<W: Write>(writer: &mut W, chunk_type: &[u8; 4], data: &[u8]) -> Result<()> {
    let length = u32::try_from(data.len()).map_err(|_| MetraError::WriteFailure {
        message: "PNG chunk exceeds the 32-bit length limit".to_owned(),
    })?;
    write_all(writer, &length.to_be_bytes())?;
    write_all(writer, chunk_type)?;
    write_all(writer, data)?;
    write_all(writer, &crc32(chunk_type, data).to_be_bytes())
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

fn write_itxt_xmp_chunk<W: Write>(writer: &mut W, value: &[u8]) -> Result<()> {
    let keyword = b"XML:com.adobe.xmp";
    let data_length = keyword.len() + 5 + value.len();
    let length = u32::try_from(data_length).map_err(|_| MetraError::WriteFailure {
        message: "PNG iTXt XMP chunk exceeds the 32-bit length limit".to_owned(),
    })?;
    let chunk_type = *b"iTXt";
    let mut data = Vec::with_capacity(data_length);
    data.extend_from_slice(keyword);
    data.extend_from_slice(&[0, 0, 0, 0, 0]);
    data.extend_from_slice(value);
    write_all(writer, &length.to_be_bytes())?;
    write_all(writer, &chunk_type)?;
    write_all(writer, &data)?;
    write_all(writer, &crc32(&chunk_type, &data).to_be_bytes())
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

    const XMP_BEFORE: &[u8] =
        br#"<x:xmpmeta><rdf:RDF><rdf:Description xmlns:dc="urn:dc" dc:format="before"/></rdf:RDF></x:xmpmeta>"#;
    const XMP_AFTER: &[u8] =
        br#"<x:xmpmeta><rdf:RDF><rdf:Description xmlns:dc="urn:dc" dc:format="after"/></rdf:RDF></x:xmpmeta>"#;

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

    fn png_with_time() -> Vec<u8> {
        let mut bytes = PNG_SIGNATURE.to_vec();
        bytes.extend_from_slice(&chunk(b"IHDR", &[0; 13]));
        bytes.extend_from_slice(&chunk(b"tIME", &[0x07, 0xEA, 9, 13, 12, 34, 56]));
        bytes.extend_from_slice(&chunk(b"IDAT", &[1, 2, 3, 4]));
        bytes.extend_from_slice(&chunk(b"IEND", &[]));
        bytes
    }

    fn png_with_phys() -> Vec<u8> {
        let mut bytes = PNG_SIGNATURE.to_vec();
        bytes.extend_from_slice(&chunk(b"IHDR", &[0; 13]));
        bytes.extend_from_slice(&chunk(b"pHYs", &[0, 0, 0, 96, 0, 0, 0, 96, 1]));
        bytes.extend_from_slice(&chunk(b"IDAT", &[1, 2, 3, 4]));
        bytes.extend_from_slice(&chunk(b"IEND", &[]));
        bytes
    }

    fn png_with_xmp(xmp: &[u8]) -> Vec<u8> {
        let mut itxt = b"XML:com.adobe.xmp\0\0\0\0\0".to_vec();
        itxt.extend_from_slice(xmp);
        let mut bytes = PNG_SIGNATURE.to_vec();
        bytes.extend_from_slice(&chunk(b"IHDR", &[0; 13]));
        bytes.extend_from_slice(&chunk(b"iTXt", &itxt));
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

    #[test]
    fn replaces_deletes_and_inserts_modification_time() {
        let bytes = png_with_time();
        let output = rewrite_png_to_vec(
            &bytes,
            info(bytes.len()),
            ParseLimits::default(),
            &[PngEdit::SetTime("2026-09-14 01:02:03".to_owned())],
        )
        .unwrap();
        let metadata = read_png(
            &mut Cursor::new(output.clone()),
            info(output.len()),
            ParseLimits::default(),
        )
        .unwrap();
        assert_eq!(
            metadata
                .find("PNG:ModificationTime")
                .unwrap()
                .display_value(),
            "2026-09-14 01:02:03"
        );
        assert!(
            output
                .windows(11)
                .any(|window| window == [0, 0, 0, 7, b't', b'I', b'M', b'E', 7, 234, 9])
        );

        let deleted = rewrite_png_to_vec(
            &bytes,
            info(bytes.len()),
            ParseLimits::default(),
            &[PngEdit::DeleteTime],
        )
        .unwrap();
        assert!(!deleted.windows(4).any(|window| window == b"tIME"));

        let mut without_time = PNG_SIGNATURE.to_vec();
        without_time.extend_from_slice(&chunk(b"IHDR", &[0; 13]));
        without_time.extend_from_slice(&chunk(b"IEND", &[]));
        let inserted = rewrite_png_to_vec(
            &without_time,
            info(without_time.len()),
            ParseLimits::default(),
            &[PngEdit::SetTime("2026:09:14 01:02:03".to_owned())],
        )
        .unwrap();
        let inserted_metadata = read_png(
            &mut Cursor::new(inserted),
            info(without_time.len() + 19),
            ParseLimits::default(),
        )
        .unwrap();
        assert_eq!(
            inserted_metadata
                .find("PNG:ModificationTime")
                .unwrap()
                .display_value(),
            "2026-09-14 01:02:03"
        );
    }

    #[test]
    fn rejects_invalid_modification_time() {
        let bytes = png_with_time();
        let error = rewrite_png_to_vec(
            &bytes,
            info(bytes.len()),
            ParseLimits::default(),
            &[PngEdit::SetTime("2026-02-29 01:02:03".to_owned())],
        )
        .unwrap_err();
        assert!(error.to_string().contains("out of range"));
    }

    #[test]
    fn replaces_deletes_and_inserts_physical_resolution() {
        let bytes = png_with_phys();
        let output = rewrite_png_to_vec(
            &bytes,
            info(bytes.len()),
            ParseLimits::default(),
            &[
                PngEdit::SetPhysX("300".to_owned()),
                PngEdit::SetPhysUnit("unknown".to_owned()),
            ],
        )
        .unwrap();
        let metadata = read_png(
            &mut Cursor::new(output.clone()),
            info(output.len()),
            ParseLimits::default(),
        )
        .unwrap();
        assert_eq!(
            metadata.find("PNG:PixelsPerUnitX").unwrap().display_value(),
            "300"
        );
        assert_eq!(
            metadata.find("PNG:PixelsPerUnitY").unwrap().display_value(),
            "96"
        );
        assert_eq!(
            metadata.find("PNG:Unit").unwrap().display_value(),
            "unknown"
        );

        let deleted = rewrite_png_to_vec(
            &bytes,
            info(bytes.len()),
            ParseLimits::default(),
            &[PngEdit::DeletePhys],
        )
        .unwrap();
        assert!(!deleted.windows(4).any(|window| window == b"pHYs"));

        let mut without_phys = PNG_SIGNATURE.to_vec();
        without_phys.extend_from_slice(&chunk(b"IHDR", &[0; 13]));
        without_phys.extend_from_slice(&chunk(b"IEND", &[]));
        let inserted = rewrite_png_to_vec(
            &without_phys,
            info(without_phys.len()),
            ParseLimits::default(),
            &[PngEdit::SetPhysY("72".to_owned())],
        )
        .unwrap();
        let inserted_metadata = read_png(
            &mut Cursor::new(inserted.clone()),
            info(inserted.len()),
            ParseLimits::default(),
        )
        .unwrap();
        assert_eq!(
            inserted_metadata
                .find("PNG:PixelsPerUnitY")
                .unwrap()
                .display_value(),
            "72"
        );
        assert_eq!(
            inserted_metadata
                .find("PNG:PixelsPerUnitX")
                .unwrap()
                .display_value(),
            "0"
        );
    }

    #[test]
    fn rejects_invalid_physical_resolution_values() {
        let bytes = png_with_phys();
        let error = rewrite_png_to_vec(
            &bytes,
            info(bytes.len()),
            ParseLimits::default(),
            &[PngEdit::SetPhysX("-1".to_owned())],
        )
        .unwrap_err();
        assert!(error.to_string().contains("unsigned 32-bit"));
        let error = rewrite_png_to_vec(
            &bytes,
            info(bytes.len()),
            ParseLimits::default(),
            &[PngEdit::SetPhysUnit("inch".to_owned())],
        )
        .unwrap_err();
        assert!(error.to_string().contains("unit must be"));
    }

    #[test]
    fn replaces_deletes_and_inserts_xmp_chunks() {
        let bytes = png_with_xmp(XMP_BEFORE);
        let output = rewrite_png_to_vec(
            &bytes,
            info(bytes.len()),
            ParseLimits::default(),
            &[PngEdit::SetXmp(
                String::from_utf8(XMP_AFTER.to_vec()).unwrap(),
            )],
        )
        .unwrap();
        assert!(
            output
                .windows(12)
                .any(|window| window == [0, 0, 0, 4, b'I', b'D', b'A', b'T', 1, 2, 3, 4])
        );
        let metadata = read_png(
            &mut Cursor::new(output.clone()),
            info(output.len()),
            ParseLimits::default(),
        )
        .unwrap();
        assert_eq!(
            metadata.find("XMP:dc:format").unwrap().display_value(),
            "after"
        );

        let deleted = rewrite_png_to_vec(
            &bytes,
            info(bytes.len()),
            ParseLimits::default(),
            &[PngEdit::DeleteXmp],
        )
        .unwrap();
        let deleted_metadata = read_png(
            &mut Cursor::new(deleted.clone()),
            info(deleted.len()),
            ParseLimits::default(),
        )
        .unwrap();
        assert!(deleted_metadata.find("XMP:Packet").is_none());

        let mut without_xmp = PNG_SIGNATURE.to_vec();
        without_xmp.extend_from_slice(&chunk(b"IHDR", &[0; 13]));
        without_xmp.extend_from_slice(&chunk(b"IEND", &[]));
        let inserted = rewrite_png_to_vec(
            &without_xmp,
            info(without_xmp.len()),
            ParseLimits::default(),
            &[PngEdit::SetXmp(
                String::from_utf8(XMP_AFTER.to_vec()).unwrap(),
            )],
        )
        .unwrap();
        let inserted_metadata = read_png(
            &mut Cursor::new(inserted.clone()),
            info(inserted.len()),
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
        let bytes = png_with_xmp(XMP_BEFORE);
        let error = rewrite_png_to_vec(
            &bytes,
            info(bytes.len()),
            ParseLimits::default(),
            &[PngEdit::SetXmp("<broken".to_owned())],
        )
        .unwrap_err();
        assert!(error.to_string().contains("XML"));
    }
}
