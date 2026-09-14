use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use metra_core::{
    FileInfo, Metadata, MetraError, ParseLimits, Result, Source, Tag, TagValue, ValueType, Warning,
};

#[derive(Debug)]
struct ScanSegment {
    file_offset: u64,
    bytes: Vec<u8>,
}

pub fn read_pdf<R: Read + Seek>(
    reader: &mut R,
    file_info: FileInfo,
    limits: ParseLimits,
) -> Result<Metadata> {
    let path = file_info.path.clone();
    let file_length = file_info.size;
    let mut metadata = Metadata::new(file_info);
    if file_length < 5 {
        return Err(MetraError::InvalidHeader {
            context: "PDF".to_owned(),
            message: "file is shorter than the PDF header".to_owned(),
        });
    }
    let header_length = usize::try_from(file_length).unwrap_or(usize::MAX).min(16);
    let header = read_at(reader, 0, header_length, &path, "PDF header")?;
    if !header.starts_with(b"%PDF-") {
        return Err(MetraError::InvalidHeader {
            context: "PDF".to_owned(),
            message: "expected %PDF- header".to_owned(),
        });
    }
    if let Some(version_end) = header[5..]
        .iter()
        .position(|byte| byte.is_ascii_whitespace())
    {
        let version = String::from_utf8_lossy(&header[5..5 + version_end]).into_owned();
        add_tag(
            &mut metadata,
            "Version",
            TagValue::String(version),
            ValueType::String,
            "Header",
            5,
            version_end as u64,
            None,
        );
    }

    let segments = scan_segments(reader, &path, file_length, limits, &mut metadata)?;
    let mut xmp_seen = false;
    for segment in &segments {
        if let Some((object_number, generation)) = find_info_reference(&segment.bytes)
            && let Some((dictionary, dictionary_offset)) =
                find_info_dictionary(&segment.bytes, object_number, generation)
        {
            parse_info_dictionary(
                dictionary,
                segment.file_offset + dictionary_offset as u64,
                limits,
                &mut metadata,
            );
        }
        if !xmp_seen && let Some((packet, packet_offset)) = find_xmp_packet(&segment.bytes) {
            match crate::xmp::parse_xmp(
                packet,
                segment.file_offset + packet_offset as u64,
                "PDF/XMP",
                &mut metadata,
                limits,
            ) {
                Ok(()) => xmp_seen = true,
                Err(error) => metadata.add_warning(
                    Warning::new("invalid-pdf-xmp", error.to_string())
                        .at(segment.file_offset + packet_offset as u64),
                ),
            }
        }
    }
    metadata.sort_tags();
    Ok(metadata)
}

fn scan_segments<R: Read + Seek>(
    reader: &mut R,
    path: &Path,
    file_length: u64,
    limits: ParseLimits,
    metadata: &mut Metadata,
) -> Result<Vec<ScanSegment>> {
    if file_length <= limits.max_metadata_bytes as u64 {
        return Ok(vec![ScanSegment {
            file_offset: 0,
            bytes: read_at(reader, 0, file_length as usize, path, "PDF metadata scan")?,
        }]);
    }
    let window = limits.max_metadata_bytes / 2;
    if window == 0 {
        metadata.add_warning(Warning::new(
            "pdf-scan-limit",
            "PDF metadata scan budget is zero",
        ));
        return Ok(Vec::new());
    }
    let tail_offset = file_length - window as u64;
    metadata.add_warning(Warning::new(
        "pdf-scan-limit",
        format!(
            "PDF exceeds the metadata scan budget; inspecting the first and last {window} bytes"
        ),
    ));
    let head = read_at(reader, 0, window, path, "PDF head scan")?;
    let tail = read_at(reader, tail_offset, window, path, "PDF tail scan")?;
    Ok(vec![
        ScanSegment {
            file_offset: 0,
            bytes: head,
        },
        ScanSegment {
            file_offset: tail_offset,
            bytes: tail,
        },
    ])
}

fn find_info_reference(bytes: &[u8]) -> Option<(u32, u32)> {
    let mut trailer_start = 0_usize;
    let mut result = None;
    while let Some(relative) = find_subslice(&bytes[trailer_start..], b"trailer") {
        let start = trailer_start + relative + b"trailer".len();
        let end = find_subslice(&bytes[start..], b"startxref")
            .map(|relative| start + relative)
            .unwrap_or(bytes.len());
        if let Some(info) = find_reference_after(&bytes[start..end], b"/Info") {
            result = Some(info);
        }
        trailer_start = start;
        if trailer_start >= bytes.len() {
            break;
        }
    }
    result
}

fn find_reference_after(bytes: &[u8], key: &[u8]) -> Option<(u32, u32)> {
    let key_start = find_subslice(bytes, key)? + key.len();
    let mut cursor = skip_whitespace(bytes, key_start);
    let (object_number, next) = read_decimal(bytes, cursor)?;
    cursor = skip_whitespace(bytes, next);
    let (generation, next) = read_decimal(bytes, cursor)?;
    cursor = skip_whitespace(bytes, next);
    if bytes.get(cursor..cursor + 1) == Some(b"R") {
        Some((object_number, generation))
    } else {
        None
    }
}

fn find_info_dictionary(
    bytes: &[u8],
    object_number: u32,
    generation: u32,
) -> Option<(&[u8], usize)> {
    let marker = format!("{object_number} {generation} obj");
    let object_start = find_subslice(bytes, marker.as_bytes())?;
    let dictionary_relative = find_subslice(&bytes[object_start..], b"<<")?;
    let dictionary_start = object_start + dictionary_relative + 2;
    let dictionary_end = find_subslice(&bytes[dictionary_start..], b">>")? + dictionary_start;
    Some((&bytes[dictionary_start..dictionary_end], dictionary_start))
}

fn parse_info_dictionary(
    dictionary: &[u8],
    dictionary_offset: u64,
    limits: ParseLimits,
    metadata: &mut Metadata,
) {
    let mut cursor = 0_usize;
    while cursor < dictionary.len() {
        let Some(relative) = find_subslice(&dictionary[cursor..], b"/") else {
            break;
        };
        let key_start = cursor + relative;
        let key_end = dictionary[key_start + 1..]
            .iter()
            .position(|byte| !is_pdf_name_byte(*byte))
            .map(|relative| key_start + 1 + relative)
            .unwrap_or(dictionary.len());
        let key = String::from_utf8_lossy(&dictionary[key_start + 1..key_end]);
        cursor = skip_whitespace(dictionary, key_end);
        let Some((value, value_end)) = parse_value(dictionary, cursor, limits.max_value_bytes)
        else {
            break;
        };
        if value != b"null"
            && let Some(name) = info_name(&key)
            && let Some(value) = decode_pdf_string(&value)
        {
            let raw = value.as_bytes().to_vec();
            add_tag(
                metadata,
                name,
                TagValue::String(value),
                ValueType::String,
                "Info",
                dictionary_offset + cursor as u64,
                (value_end - cursor) as u64,
                Some(&raw),
            );
        }
        cursor = value_end.max(key_end + 1);
    }
}

fn info_name(key: &str) -> Option<&'static str> {
    Some(match key {
        "Title" => "Title",
        "Author" => "Author",
        "Subject" => "Subject",
        "Keywords" => "Keywords",
        "Creator" => "Creator",
        "Producer" => "Producer",
        "CreationDate" => "CreationDate",
        "ModDate" => "ModifyDate",
        "Trapped" => "Trapped",
        _ => return None,
    })
}

fn find_xmp_packet(bytes: &[u8]) -> Option<(&[u8], usize)> {
    let start =
        find_subslice(bytes, b"<?xpacket").or_else(|| find_subslice(bytes, b"<x:xmpmeta"))?;
    let end = if let Some(relative) = find_subslice(&bytes[start..], b"<?xpacket end") {
        let end_start = start + relative;
        find_subslice(&bytes[end_start..], b"?>")
            .map(|end| end_start + end + 2)
            .unwrap_or(bytes.len())
    } else if let Some(relative) = find_subslice(&bytes[start..], b"</x:xmpmeta>") {
        start + relative + b"</x:xmpmeta>".len()
    } else {
        bytes.len()
    };
    Some((&bytes[start..end], start))
}

fn parse_value(bytes: &[u8], start: usize, max_value_bytes: usize) -> Option<(Vec<u8>, usize)> {
    match bytes.get(start)? {
        b'(' => parse_literal_string(bytes, start, max_value_bytes),
        b'<' if bytes.get(start + 1) != Some(&b'<') => {
            parse_hex_string(bytes, start, max_value_bytes)
        }
        b'/' => {
            let end = bytes[start + 1..]
                .iter()
                .position(|byte| !is_pdf_name_byte(*byte))
                .map(|relative| start + 1 + relative)
                .unwrap_or(bytes.len());
            Some((bytes[start + 1..end].to_vec(), end))
        }
        _ => {
            let end = bytes[start..]
                .iter()
                .position(|byte| is_pdf_delimiter(*byte))
                .map(|relative| start + relative)
                .unwrap_or(bytes.len());
            if end == start {
                None
            } else {
                Some((bytes[start..end].to_vec(), end))
            }
        }
    }
}

fn parse_literal_string(
    bytes: &[u8],
    start: usize,
    max_value_bytes: usize,
) -> Option<(Vec<u8>, usize)> {
    let mut result = Vec::new();
    let mut cursor = start + 1;
    let mut depth = 1_usize;
    while cursor < bytes.len() {
        let byte = bytes[cursor];
        cursor += 1;
        if byte == b'\\' {
            let escaped = *bytes.get(cursor)?;
            cursor += 1;
            match escaped {
                b'n' => result.push(b'\n'),
                b'r' => result.push(b'\r'),
                b't' => result.push(b'\t'),
                b'b' => result.push(8),
                b'f' => result.push(12),
                b'(' | b')' | b'\\' => result.push(escaped),
                b'\r' => {
                    if bytes.get(cursor) == Some(&b'\n') {
                        cursor += 1;
                    }
                }
                b'\n' => {}
                b'0'..=b'7' => {
                    let mut value = u16::from(escaped - b'0');
                    for _ in 0..2 {
                        let Some(next @ b'0'..=b'7') = bytes.get(cursor).copied() else {
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
                return Some((result, cursor));
            }
            result.push(byte);
        } else {
            result.push(byte);
        }
        if result.len() > max_value_bytes {
            return None;
        }
    }
    None
}

fn parse_hex_string(
    bytes: &[u8],
    start: usize,
    max_value_bytes: usize,
) -> Option<(Vec<u8>, usize)> {
    let mut result = Vec::new();
    let mut high_nibble = None;
    let mut cursor = start + 1;
    while cursor < bytes.len() {
        let byte = bytes[cursor];
        cursor += 1;
        if byte == b'>' {
            if let Some(high) = high_nibble {
                result.push(high << 4);
            }
            return Some((result, cursor));
        }
        if byte.is_ascii_whitespace() {
            continue;
        }
        let nibble = hex_nibble(byte)?;
        if let Some(high) = high_nibble.take() {
            result.push((high << 4) | nibble);
        } else {
            high_nibble = Some(nibble);
        }
        if result.len() > max_value_bytes {
            return None;
        }
    }
    None
}

fn decode_pdf_string(bytes: &[u8]) -> Option<String> {
    if bytes.len() >= 2 && bytes[..2] == [0xFE, 0xFF] {
        let units = bytes[2..]
            .chunks_exact(2)
            .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>();
        return Some(String::from_utf16_lossy(&units));
    }
    Some(bytes.iter().map(|byte| char::from(*byte)).collect())
}

fn read_decimal(bytes: &[u8], start: usize) -> Option<(u32, usize)> {
    let mut cursor = start;
    let mut value = 0_u32;
    let mut read_any = false;
    while let Some(byte) = bytes.get(cursor).copied() {
        if !byte.is_ascii_digit() {
            break;
        }
        read_any = true;
        value = value.checked_mul(10)?.checked_add(u32::from(byte - b'0'))?;
        cursor += 1;
    }
    read_any.then_some((value, cursor))
}

fn skip_whitespace(bytes: &[u8], mut cursor: usize) -> usize {
    while bytes
        .get(cursor)
        .is_some_and(|byte| byte.is_ascii_whitespace())
    {
        cursor += 1;
    }
    cursor
}

fn is_pdf_name_byte(byte: u8) -> bool {
    !is_pdf_delimiter(byte) && !byte.is_ascii_whitespace()
}

fn is_pdf_delimiter(byte: u8) -> bool {
    byte.is_ascii_whitespace() || matches!(byte, b'<' | b'>' | b'[' | b']' | b'(' | b')' | b'/')
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn find_subslice(bytes: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    bytes
        .windows(needle.len())
        .position(|window| window == needle)
}

#[allow(clippy::too_many_arguments)]
fn add_tag(
    metadata: &mut Metadata,
    name: &str,
    value: TagValue,
    value_type: ValueType,
    group: &str,
    offset: u64,
    length: u64,
    raw_value: Option<&[u8]>,
) {
    metadata.add_tag(Tag {
        namespace: "PDF".to_owned(),
        group: group.to_owned(),
        id: None,
        name: name.to_owned(),
        description: Some("PDF metadata".to_owned()),
        raw_value: raw_value.map(<[u8]>::to_vec),
        value,
        value_type,
        source: Source::new("PDF", Some(offset), Some(length)),
        writable: false,
    });
}

fn read_at<R: Read + Seek>(
    reader: &mut R,
    offset: u64,
    length: usize,
    path: &Path,
    context: &str,
) -> Result<Vec<u8>> {
    reader
        .seek(SeekFrom::Start(offset))
        .map_err(|source| io_error(path, source))?;
    let mut bytes = vec![0_u8; length];
    reader
        .read_exact(&mut bytes)
        .map_err(|source| match source.kind() {
            std::io::ErrorKind::UnexpectedEof => MetraError::UnexpectedEof {
                context: context.to_owned(),
            },
            _ => io_error(path, source),
        })?;
    Ok(bytes)
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

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;
    use metra_core::{FileFormat, FileInfo};

    #[test]
    fn reads_pdf_info_dictionary() {
        let bytes = b"%PDF-1.7\n5 0 obj\n<< /Title (Metra\\050PDF\\051) /Author <FEFF004F0074> /Trapped /False >>\nendobj\ntrailer\n<< /Info 5 0 R >>\nstartxref\n9\n%%EOF\n";
        let info = FileInfo::new("document.pdf".into(), bytes.len() as u64, FileFormat::Pdf);
        let metadata = read_pdf(&mut Cursor::new(bytes), info, ParseLimits::default()).unwrap();
        assert_eq!(metadata.find("PDF:Version").unwrap().display_value(), "1.7");
        assert_eq!(
            metadata.find("PDF:Title").unwrap().display_value(),
            "Metra(PDF)"
        );
        assert_eq!(metadata.find("PDF:Author").unwrap().display_value(), "Ot");
        assert_eq!(
            metadata.find("PDF:Trapped").unwrap().display_value(),
            "False"
        );
    }

    #[test]
    fn rejects_invalid_pdf_header() {
        let bytes = b"not a pdf";
        let info = FileInfo::new("bad.pdf".into(), bytes.len() as u64, FileFormat::Pdf);
        let result = read_pdf(&mut Cursor::new(bytes), info, ParseLimits::default());
        assert!(matches!(result, Err(MetraError::InvalidHeader { .. })));
    }
}
