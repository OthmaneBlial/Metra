use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use metra_core::{
    FileInfo, Metadata, MetraError, ParseLimits, Result, Source, Tag, TagValue, ValueType, Warning,
};

const EBML_SIGNATURE: &[u8; 4] = b"\x1A\x45\xDF\xA3";
const EBML: u64 = 0x1A45_DFA3;
const SEGMENT: u64 = 0x1853_8067;
const INFO: u64 = 0x1549_A966;
const TRACKS: u64 = 0x1654_AE6B;
const TRACK_ENTRY: u64 = 0xAE;
const TAGS: u64 = 0x1254_C367;
const TAG: u64 = 0x7373;
const SIMPLE_TAG: u64 = 0x67C8;
const DOC_TYPE: u64 = 0x4282;
const TIMECODE_SCALE: u64 = 0x002A_D7B1;
const DURATION: u64 = 0x4489;
const TITLE: u64 = 0x7BA9;
const MUXING_APP: u64 = 0x4D80;
const WRITING_APP: u64 = 0x5741;
const DATE_UTC: u64 = 0x4461;
const TRACK_NUMBER: u64 = 0xD7;
const TRACK_UID: u64 = 0x73C5;
const TRACK_TYPE: u64 = 0x83;
const TRACK_NAME: u64 = 0x536E;
const LANGUAGE: u64 = 0x22B59C;
const CODEC_ID: u64 = 0x86;
const TAG_NAME: u64 = 0x45A3;
const TAG_STRING: u64 = 0x4487;
const TARGET_TYPE_VALUE: u64 = 0x68CA;
const CLUSTER: u64 = 0x1F43_B675;
const CHAPTERS: u64 = 0x1043_A770;
const EDITION_ENTRY: u64 = 0x45B9;
const EDITION_UID: u64 = 0x45BC;
const EDITION_FLAG_DEFAULT: u64 = 0x45DB;
const CHAPTER_ATOM: u64 = 0xB6;
const CHAPTER_UID: u64 = 0x73C4;
const CHAPTER_TIME_START: u64 = 0x91;
const CHAPTER_TIME_END: u64 = 0x92;
const CHAPTER_DISPLAY: u64 = 0x80;
const CHAP_STRING: u64 = 0x85;
const CHAP_LANGUAGE: u64 = 0x437C;
const CHAP_COUNTRY: u64 = 0x437D;
const CUES: u64 = 0x1C53_BB6B;
const CUE_POINT: u64 = 0xBB;
const CUE_TIME: u64 = 0xB3;
const CUE_TRACK_POSITIONS: u64 = 0xB7;
const CUE_TRACK: u64 = 0xF7;
const CUE_CLUSTER_POSITION: u64 = 0xF1;
const ATTACHMENTS: u64 = 0x1941_A469;
const ATTACHED_FILE: u64 = 0x61A7;
const FILE_DESCRIPTION: u64 = 0x467E;
const FILE_NAME: u64 = 0x466E;
const FILE_MIME_TYPE: u64 = 0x4660;
const FILE_DATA: u64 = 0x465C;
const FILE_UID: u64 = 0x46AE;

#[derive(Debug, Default)]
struct MatroskaState {
    timecode_scale: u64,
    duration: Option<f64>,
}

#[derive(Debug, Clone, Copy)]
struct ElementHeader {
    id: u64,
    size: Option<u64>,
    header_length: u64,
}

pub(crate) fn document_type(bytes: &[u8]) -> Option<String> {
    if !bytes.starts_with(EBML_SIGNATURE) {
        return None;
    }
    let (header_size, header_size_length, unknown) = parse_vint(bytes.get(4..)?, true)?;
    if unknown {
        return None;
    }
    let start = 4usize.checked_add(header_size_length)?;
    let end = start.checked_add(usize::try_from(header_size).ok()?)?;
    let mut cursor = start;
    while cursor < end {
        let (id, id_length, _) = parse_vint(bytes.get(cursor..)?, false)?;
        cursor = cursor.checked_add(id_length)?;
        let (size, size_length, unknown) = parse_vint(bytes.get(cursor..)?, true)?;
        cursor = cursor.checked_add(size_length)?;
        let size = usize::try_from(size).ok()?;
        let value_end = cursor.checked_add(size)?;
        if value_end > end {
            return None;
        }
        if id == DOC_TYPE {
            return Some(String::from_utf8_lossy(&bytes[cursor..value_end]).to_ascii_lowercase());
        }
        if unknown {
            return None;
        }
        cursor = value_end;
    }
    None
}

pub fn read_matroska<R: Read + Seek>(
    reader: &mut R,
    file_info: FileInfo,
    limits: ParseLimits,
) -> Result<Metadata> {
    let path = file_info.path.clone();
    let file_length = file_info.size;
    let mut metadata = Metadata::new(file_info);
    if file_length < EBML_SIGNATURE.len() as u64 {
        return Err(MetraError::InvalidHeader {
            context: "Matroska".to_owned(),
            message: "file is shorter than the EBML signature".to_owned(),
        });
    }
    let signature = read_at(reader, 0, 4, file_length, &path, "EBML signature")?;
    if signature.as_slice() != EBML_SIGNATURE {
        return Err(MetraError::InvalidHeader {
            context: "Matroska".to_owned(),
            message: "expected EBML signature".to_owned(),
        });
    }
    let mut state = MatroskaState {
        timecode_scale: 1_000_000,
        duration: None,
    };
    let mut materialized = 0_usize;
    let mut element_count = 0_usize;
    scan_region(
        reader,
        0,
        file_length,
        0,
        "Root",
        &mut metadata,
        limits,
        &mut materialized,
        &mut element_count,
        &path,
        file_length,
        &mut state,
    )?;
    if let Some(duration) = state.duration {
        add_tag(
            &mut metadata,
            None,
            "DurationSeconds",
            TagValue::Float(duration * state.timecode_scale as f64 / 1_000_000_000.0),
            ValueType::Float,
            "Matroska/derived",
            None,
            None,
            None,
        );
    }
    metadata.sort_tags();
    Ok(metadata)
}

#[allow(clippy::too_many_arguments)]
fn scan_region<R: Read + Seek>(
    reader: &mut R,
    mut cursor: u64,
    end: u64,
    depth: usize,
    group: &str,
    metadata: &mut Metadata,
    limits: ParseLimits,
    materialized: &mut usize,
    element_count: &mut usize,
    path: &Path,
    file_length: u64,
    state: &mut MatroskaState,
) -> Result<()> {
    if depth > limits.max_recursion_depth {
        metadata.add_warning(
            Warning::new("matroska-recursion-limit", "EBML nesting limit reached").at(cursor),
        );
        return Ok(());
    }
    while cursor < end {
        if *element_count >= limits.max_jpeg_segments {
            metadata.add_warning(
                Warning::new("matroska-element-limit", "EBML element limit reached").at(cursor),
            );
            break;
        }
        let Some(header) = read_element_header(reader, cursor, end, file_length, path)? else {
            metadata.add_warning(
                Warning::new(
                    "truncated-matroska-element",
                    "EBML element header is truncated",
                )
                .at(cursor),
            );
            break;
        };
        let payload_start =
            cursor
                .checked_add(header.header_length)
                .ok_or(MetraError::InvalidOffset {
                    context: "EBML payload".to_owned(),
                    offset: cursor,
                })?;
        let Some(payload_length) = header.size else {
            metadata.add_warning(
                Warning::new(
                    "unknown-matroska-size",
                    format!("element 0x{:X} has an unknown size", header.id),
                )
                .at(payload_start),
            );
            break;
        };
        let payload_end =
            payload_start
                .checked_add(payload_length)
                .ok_or(MetraError::InvalidOffset {
                    context: format!("EBML element 0x{:X}", header.id),
                    offset: payload_length,
                })?;
        if payload_end > end {
            metadata.add_warning(
                Warning::new(
                    "truncated-matroska-element",
                    format!("element 0x{:X} extends beyond its container", header.id),
                )
                .at(payload_start),
            );
            break;
        }
        match header.id {
            EBML | SEGMENT | INFO | TRACKS | CHAPTERS | EDITION_ENTRY | CHAPTER_ATOM
            | CHAPTER_DISPLAY | CUES | CUE_POINT | CUE_TRACK_POSITIONS | ATTACHMENTS
            | ATTACHED_FILE => {
                let child_group = match header.id {
                    INFO => "Info",
                    TRACKS => "Tracks",
                    CHAPTERS => "Chapters",
                    EDITION_ENTRY => "Edition",
                    CHAPTER_ATOM => "Chapter",
                    CHAPTER_DISPLAY => "ChapterDisplay",
                    CUES => "Cues",
                    CUE_POINT => "CuePoint",
                    CUE_TRACK_POSITIONS => "CueTrackPositions",
                    ATTACHMENTS => "Attachments",
                    ATTACHED_FILE => "Attachment",
                    _ => group,
                };
                scan_region(
                    reader,
                    payload_start,
                    payload_end,
                    depth + 1,
                    child_group,
                    metadata,
                    limits,
                    materialized,
                    element_count,
                    path,
                    file_length,
                    state,
                )?;
            }
            TRACK_ENTRY => scan_region(
                reader,
                payload_start,
                payload_end,
                depth + 1,
                "Track",
                metadata,
                limits,
                materialized,
                element_count,
                path,
                file_length,
                state,
            )?,
            TAGS | TAG => scan_region(
                reader,
                payload_start,
                payload_end,
                depth + 1,
                "Tags",
                metadata,
                limits,
                materialized,
                element_count,
                path,
                file_length,
                state,
            )?,
            SIMPLE_TAG => parse_simple_tag(
                reader,
                payload_start,
                payload_end,
                metadata,
                limits,
                materialized,
                element_count,
                path,
                file_length,
            )?,
            CLUSTER => {}
            _ => {
                if let Some((name, value, value_type, raw)) = read_known_value(
                    reader,
                    header.id,
                    payload_start,
                    payload_length,
                    metadata,
                    limits,
                    materialized,
                    path,
                    file_length,
                    state,
                )? {
                    let element_group = match header.id {
                        DOC_TYPE => "EBML",
                        TRACK_NUMBER | TRACK_UID | TRACK_TYPE | TRACK_NAME | LANGUAGE
                        | CODEC_ID
                            if group == "Track" =>
                        {
                            "Track"
                        }
                        _ => group,
                    };
                    add_tag(
                        metadata,
                        Some(header.id as u32),
                        name,
                        value,
                        value_type,
                        &format!("Matroska/{element_group}"),
                        Some(payload_start),
                        Some(payload_length),
                        Some(raw),
                    );
                    if header.id == DURATION
                        && let TagValue::Float(value) = metadata
                            .tags
                            .last()
                            .map(|tag| tag.value.clone())
                            .unwrap_or(TagValue::Float(0.0))
                    {
                        state.duration = Some(value);
                    }
                }
            }
        }
        cursor = payload_end;
        *element_count += 1;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn parse_simple_tag<R: Read + Seek>(
    reader: &mut R,
    mut cursor: u64,
    end: u64,
    metadata: &mut Metadata,
    limits: ParseLimits,
    materialized: &mut usize,
    element_count: &mut usize,
    path: &Path,
    file_length: u64,
) -> Result<()> {
    let mut name = None;
    let mut value = None;
    let mut value_offset = None;
    let mut value_length = None;
    while cursor < end {
        let Some(header) = read_element_header(reader, cursor, end, file_length, path)? else {
            metadata.add_warning(
                Warning::new("truncated-matroska-tag", "SimpleTag element is truncated").at(cursor),
            );
            break;
        };
        let payload_start = cursor + header.header_length;
        let Some(payload_length) = header.size else {
            metadata.add_warning(
                Warning::new("unknown-matroska-size", "SimpleTag child has unknown size")
                    .at(payload_start),
            );
            break;
        };
        let payload_end =
            payload_start
                .checked_add(payload_length)
                .ok_or(MetraError::InvalidOffset {
                    context: "Matroska SimpleTag".to_owned(),
                    offset: payload_length,
                })?;
        if payload_end > end {
            metadata.add_warning(
                Warning::new(
                    "truncated-matroska-tag",
                    "SimpleTag child exceeds its container",
                )
                .at(payload_start),
            );
            break;
        }
        if (header.id == TAG_NAME || header.id == TAG_STRING)
            && let Some(bytes) = read_value(
                reader,
                payload_start,
                payload_length,
                metadata,
                limits,
                materialized,
                path,
                file_length,
                "Matroska tag string",
            )?
        {
            let string = String::from_utf8_lossy(&bytes)
                .trim_end_matches('\0')
                .to_owned();
            if header.id == TAG_NAME {
                name = Some(string);
            } else {
                value = Some(string);
                value_offset = Some(payload_start);
                value_length = Some(payload_length);
            }
        }
        cursor = payload_end;
        *element_count += 1;
    }
    if let (Some(name), Some(value)) = (name, value) {
        add_tag(
            metadata,
            None,
            &format!("Tag:{name}"),
            TagValue::String(value),
            ValueType::String,
            "Matroska/Tags",
            value_offset,
            value_length,
            None,
        );
    }
    Ok(())
}

type MatroskaKnownValue = (&'static str, TagValue, ValueType, Vec<u8>);

#[allow(clippy::too_many_arguments)]
fn read_known_value<R: Read + Seek>(
    reader: &mut R,
    id: u64,
    offset: u64,
    length: u64,
    metadata: &mut Metadata,
    limits: ParseLimits,
    materialized: &mut usize,
    path: &Path,
    file_length: u64,
    state: &mut MatroskaState,
) -> Result<Option<MatroskaKnownValue>> {
    let (name, value_type) = match id {
        DOC_TYPE => ("DocType", ValueType::String),
        TIMECODE_SCALE => ("TimecodeScale", ValueType::UnsignedInteger),
        DURATION => ("Duration", ValueType::Float),
        TITLE => ("Title", ValueType::String),
        MUXING_APP => ("MuxingApp", ValueType::String),
        WRITING_APP => ("WritingApp", ValueType::String),
        DATE_UTC => ("DateUTC", ValueType::SignedInteger),
        TRACK_NUMBER => ("TrackNumber", ValueType::UnsignedInteger),
        TRACK_UID => ("TrackUID", ValueType::UnsignedInteger),
        TRACK_TYPE => ("TrackType", ValueType::UnsignedInteger),
        TRACK_NAME => ("TrackName", ValueType::String),
        LANGUAGE => ("Language", ValueType::String),
        CODEC_ID => ("CodecID", ValueType::String),
        TARGET_TYPE_VALUE => ("TargetTypeValue", ValueType::UnsignedInteger),
        EDITION_UID => ("EditionUID", ValueType::UnsignedInteger),
        EDITION_FLAG_DEFAULT => ("EditionFlagDefault", ValueType::UnsignedInteger),
        CHAPTER_UID => ("ChapterUID", ValueType::UnsignedInteger),
        CHAPTER_TIME_START => ("ChapterTimeStart", ValueType::UnsignedInteger),
        CHAPTER_TIME_END => ("ChapterTimeEnd", ValueType::UnsignedInteger),
        CHAP_STRING => ("ChapterString", ValueType::String),
        CHAP_LANGUAGE => ("ChapterLanguage", ValueType::String),
        CHAP_COUNTRY => ("ChapterCountry", ValueType::String),
        CUE_TIME => ("CueTime", ValueType::UnsignedInteger),
        CUE_TRACK => ("CueTrack", ValueType::UnsignedInteger),
        CUE_CLUSTER_POSITION => ("CueClusterPosition", ValueType::UnsignedInteger),
        FILE_DESCRIPTION => ("FileDescription", ValueType::String),
        FILE_NAME => ("FileName", ValueType::String),
        FILE_MIME_TYPE => ("FileMimeType", ValueType::String),
        FILE_UID => ("FileUID", ValueType::UnsignedInteger),
        FILE_DATA => ("FileDataSize", ValueType::UnsignedInteger),
        _ => return Ok(None),
    };
    if id == FILE_DATA {
        return Ok(Some((
            "FileDataSize",
            TagValue::Unsigned(length),
            ValueType::UnsignedInteger,
            Vec::new(),
        )));
    }
    let Some(bytes) = read_value(
        reader,
        offset,
        length,
        metadata,
        limits,
        materialized,
        path,
        file_length,
        "Matroska metadata value",
    )?
    else {
        return Ok(None);
    };
    let (value, value_type) = match id {
        DOC_TYPE | TITLE | MUXING_APP | WRITING_APP | TRACK_NAME | LANGUAGE | CODEC_ID
        | CHAP_STRING | CHAP_LANGUAGE | CHAP_COUNTRY | FILE_DESCRIPTION | FILE_NAME
        | FILE_MIME_TYPE => (
            TagValue::String(
                String::from_utf8_lossy(&bytes)
                    .trim_end_matches('\0')
                    .to_owned(),
            ),
            value_type,
        ),
        DURATION => {
            let Some(value) = parse_float(&bytes) else {
                return Ok(None);
            };
            (TagValue::Float(value), ValueType::Float)
        }
        DATE_UTC => {
            let Some(value) = parse_signed_integer(&bytes) else {
                return Ok(None);
            };
            (TagValue::Signed(value), ValueType::SignedInteger)
        }
        TIMECODE_SCALE | TRACK_NUMBER | TRACK_UID | TRACK_TYPE | TARGET_TYPE_VALUE
        | EDITION_UID | EDITION_FLAG_DEFAULT | CHAPTER_UID | CHAPTER_TIME_START
        | CHAPTER_TIME_END | CUE_TIME | CUE_TRACK | CUE_CLUSTER_POSITION | FILE_UID => {
            let Some(value) = parse_unsigned_integer(&bytes) else {
                return Ok(None);
            };
            if id == TIMECODE_SCALE {
                state.timecode_scale = value;
            }
            (TagValue::Unsigned(value), ValueType::UnsignedInteger)
        }
        FILE_DATA => (TagValue::Unsigned(length), ValueType::UnsignedInteger),
        _ => return Ok(None),
    };
    Ok(Some((name, value, value_type, bytes)))
}

#[allow(clippy::too_many_arguments)]
fn read_value<R: Read + Seek>(
    reader: &mut R,
    offset: u64,
    length: u64,
    metadata: &mut Metadata,
    limits: ParseLimits,
    materialized: &mut usize,
    path: &Path,
    file_length: u64,
    context: &str,
) -> Result<Option<Vec<u8>>> {
    if length > u64::try_from(limits.max_value_bytes).unwrap_or(u64::MAX) {
        metadata.add_warning(
            Warning::new(
                "matroska-value-limit",
                format!("{context} contains {length} bytes; value omitted"),
            )
            .at(offset),
        );
        return Ok(None);
    }
    let length = usize::try_from(length).map_err(|_| MetraError::ResourceLimitExceeded {
        resource: context.to_owned(),
        limit: limits.max_value_bytes,
    })?;
    let total = materialized
        .checked_add(length)
        .ok_or(MetraError::ResourceLimitExceeded {
            resource: "Matroska metadata values".to_owned(),
            limit: limits.max_metadata_bytes,
        })?;
    if total > limits.max_metadata_bytes {
        metadata.add_warning(
            Warning::new(
                "matroska-metadata-limit",
                format!("{context} would exceed the metadata budget"),
            )
            .at(offset),
        );
        return Ok(None);
    }
    let bytes = read_at(reader, offset, length, file_length, path, context)?;
    *materialized = total;
    Ok(Some(bytes))
}

#[allow(clippy::too_many_arguments)]
fn add_tag(
    metadata: &mut Metadata,
    id: Option<u32>,
    name: &str,
    value: TagValue,
    value_type: ValueType,
    container: &str,
    offset: Option<u64>,
    length: Option<u64>,
    raw_value: Option<Vec<u8>>,
) {
    metadata.add_tag(Tag {
        namespace: "Matroska".to_owned(),
        group: if container.ends_with("Info") {
            "Info".to_owned()
        } else if container.ends_with("Track") {
            "Track".to_owned()
        } else if container.ends_with("Tags") {
            "Tags".to_owned()
        } else if container.contains("Chapter") || container.ends_with("Edition") {
            "Chapters".to_owned()
        } else if container.contains("Cue") {
            "Cues".to_owned()
        } else if container.contains("Attachment") {
            "Attachments".to_owned()
        } else {
            "Derived".to_owned()
        },
        id,
        name: name.to_owned(),
        description: Some("Matroska/WebM metadata property".to_owned()),
        raw_value,
        value,
        value_type,
        source: Source::new(container, offset, length),
        writable: false,
    });
}

fn parse_float(bytes: &[u8]) -> Option<f64> {
    match bytes.len() {
        4 => Some(f32::from_bits(u32::from_be_bytes(bytes.try_into().ok()?)) as f64),
        8 => Some(f64::from_bits(u64::from_be_bytes(bytes.try_into().ok()?))),
        _ => None,
    }
}

fn parse_unsigned_integer(bytes: &[u8]) -> Option<u64> {
    if bytes.is_empty() || bytes.len() > 8 {
        return None;
    }
    let mut value = 0_u64;
    for byte in bytes {
        value = value.checked_mul(256)?.checked_add(u64::from(*byte))?;
    }
    Some(value)
}

fn parse_signed_integer(bytes: &[u8]) -> Option<i64> {
    if bytes.is_empty() || bytes.len() > 8 {
        return None;
    }
    let unsigned = parse_unsigned_integer(bytes)?;
    let shift = (8 - bytes.len()) * 8;
    Some(i64::from_be_bytes((unsigned << shift).to_be_bytes()) >> shift)
}

fn read_element_header<R: Read + Seek>(
    reader: &mut R,
    offset: u64,
    end: u64,
    file_length: u64,
    path: &Path,
) -> Result<Option<ElementHeader>> {
    if offset >= end {
        return Ok(None);
    }
    let Some((id, id_length, _)) = read_vint(reader, offset, false, end, file_length, path)? else {
        return Ok(None);
    };
    let size_offset = offset
        .checked_add(id_length as u64)
        .ok_or(MetraError::InvalidOffset {
            context: "EBML size offset".to_owned(),
            offset,
        })?;
    let Some((size, size_length, unknown)) =
        read_vint(reader, size_offset, true, end, file_length, path)?
    else {
        return Ok(None);
    };
    Ok(Some(ElementHeader {
        id,
        size: (!unknown).then_some(size),
        header_length: id_length as u64 + size_length as u64,
    }))
}

fn read_vint<R: Read + Seek>(
    reader: &mut R,
    offset: u64,
    is_size: bool,
    end: u64,
    file_length: u64,
    path: &Path,
) -> Result<Option<(u64, usize, bool)>> {
    let first = read_at(
        reader,
        offset,
        1,
        file_length,
        path,
        "EBML variable integer",
    )?[0];
    let Some(width) = vint_width(first) else {
        return Ok(None);
    };
    let width_end = offset.saturating_add(width as u64);
    if width_end > end {
        return Ok(None);
    }
    let bytes = read_at(
        reader,
        offset,
        width,
        file_length,
        path,
        "EBML variable integer",
    )?;
    Ok(parse_vint(&bytes, is_size))
}

fn parse_vint(bytes: &[u8], is_size: bool) -> Option<(u64, usize, bool)> {
    let first = *bytes.first()?;
    let width = vint_width(first)?;
    if bytes.len() < width {
        return None;
    }
    let marker = 0x80_u8 >> (width - 1);
    let mut value = u64::from(if is_size { first & !marker } else { first });
    for byte in &bytes[1..width] {
        value = value.checked_shl(8)?.checked_add(u64::from(*byte))?;
    }
    let unknown = is_size && value == (1_u64 << (7 * width)) - 1;
    Some((value, width, unknown))
}

fn vint_width(first: u8) -> Option<usize> {
    (1..=8).find(|width| first & (0x80_u8 >> (width - 1)) != 0)
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
        .map_err(|source| MetraError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    let mut bytes = vec![0_u8; length];
    reader
        .read_exact(&mut bytes)
        .map_err(|source| MetraError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    Ok(bytes)
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

    fn minimal_webm() -> Vec<u8> {
        let ebml = element(&[0x42, 0x82], b"webm");
        let ebml_header = element(EBML_SIGNATURE, &ebml);
        let mut info = element(&[0x2A, 0xD7, 0xB1], &[0x0F, 0x42, 0x40]);
        info.extend_from_slice(&element(&[0x44, 0x89], &10.0_f64.to_be_bytes()));
        info.extend_from_slice(&element(&[0x7B, 0xA9], b"Metra\0"));
        let mut track = element(&[0xD7], &[1]);
        track.extend_from_slice(&element(&[0x83], &[1]));
        track.extend_from_slice(&element(&[0x86], b"V_VP9"));
        let tracks = element(&[0xAE], &track);
        let tracks = element(&[0x16, 0x54, 0xAE, 0x6B], &tracks);
        let simple_tag = element(&[0x45, 0xA3], b"TITLE");
        let mut simple_tag = simple_tag;
        simple_tag.extend_from_slice(&element(&[0x44, 0x87], b"Sample"));
        let tags = element(&[0x73, 0x73], &element(&[0x67, 0xC8], &simple_tag));
        let mut segment_data = element(&[0x15, 0x49, 0xA9, 0x66], &info);
        segment_data.extend_from_slice(&tracks);
        segment_data.extend_from_slice(&element(&[0x12, 0x54, 0xC3, 0x67], &tags));
        let segment = element(&[0x18, 0x53, 0x80, 0x67], &segment_data);
        [ebml_header, segment].concat()
    }

    #[test]
    fn reads_webm_info_tracks_and_tags_without_clusters() {
        let bytes = minimal_webm();
        let info = FileInfo::new(
            "sample.webm".into(),
            bytes.len() as u64,
            metra_core::FileFormat::Webm,
        );
        let metadata = read_matroska(&mut Cursor::new(bytes), info, ParseLimits::default())
            .expect("WebM fixture should parse");

        assert_eq!(
            metadata.find("Matroska:DocType").unwrap().display_value(),
            "webm"
        );
        assert_eq!(
            metadata.find("Matroska:Title").unwrap().display_value(),
            "Metra"
        );
        assert_eq!(
            metadata.find("Matroska:TrackNumber").unwrap().value,
            TagValue::Unsigned(1)
        );
        assert_eq!(
            metadata.find("Matroska:CodecID").unwrap().display_value(),
            "V_VP9"
        );
        assert_eq!(
            metadata.find("Matroska:Tag:TITLE").unwrap().display_value(),
            "Sample"
        );
        assert_eq!(
            metadata
                .find("Matroska:DurationSeconds")
                .unwrap()
                .display_value(),
            "0.01"
        );
    }

    #[test]
    fn reads_chapters_cues_and_attachment_descriptors_without_loading_file_data() {
        let display = [element(&[0x85], b"Intro"), element(&[0x43, 0x7C], b"eng")].concat();
        let chapter_atom = [
            element(&[0x73, 0xC4], &[1]),
            element(&[0x91], &1_u64.to_be_bytes()),
            element(&[0x92], &2_u64.to_be_bytes()),
            element(&[0x80], &display),
        ]
        .concat();
        let edition = element(&[0x45, 0xB9], &element(&[0xB6], &chapter_atom));
        let chapters = element(&[0x10, 0x43, 0xA7, 0x70], &edition);

        let attached_file = [
            element(&[0x46, 0x6E], b"cover.jpg"),
            element(&[0x46, 0x60], b"image/jpeg"),
            element(&[0x46, 0x7E], b"cover art"),
            element(&[0x46, 0x5C], b"not loaded"),
        ]
        .concat();
        let attachments = element(
            &[0x19, 0x41, 0xA4, 0x69],
            &element(&[0x61, 0xA7], &attached_file),
        );

        let cue_positions = [element(&[0xF7], &[1]), element(&[0xF1], &[0x20])].concat();
        let cue_point = [element(&[0xB3], &[0, 1]), element(&[0xB7], &cue_positions)].concat();
        let cues = element(&[0x1C, 0x53, 0xBB, 0x6B], &element(&[0xBB], &cue_point));

        let ebml = element(&[0x42, 0x82], b"matroska");
        let ebml_header = element(EBML_SIGNATURE, &ebml);
        let segment_data = [chapters, attachments, cues].concat();
        let bytes = [
            ebml_header,
            element(&[0x18, 0x53, 0x80, 0x67], &segment_data),
        ]
        .concat();
        let info = FileInfo::new(
            "chapters.mkv".into(),
            bytes.len() as u64,
            metra_core::FileFormat::Mkv,
        );
        let metadata = read_matroska(&mut Cursor::new(bytes), info, ParseLimits::default())
            .expect("Matroska chapter fixture should parse");

        assert_eq!(
            metadata.find("Matroska:ChapterTimeStart").unwrap().value,
            TagValue::Unsigned(1)
        );
        assert_eq!(
            metadata
                .find("Matroska:ChapterString")
                .unwrap()
                .display_value(),
            "Intro"
        );
        assert_eq!(
            metadata.find("Matroska:FileName").unwrap().display_value(),
            "cover.jpg"
        );
        assert_eq!(
            metadata
                .find("Matroska:FileMimeType")
                .unwrap()
                .display_value(),
            "image/jpeg"
        );
        assert_eq!(
            metadata.find("Matroska:FileDataSize").unwrap().value,
            TagValue::Unsigned(10)
        );
        assert_eq!(
            metadata.find("Matroska:CueClusterPosition").unwrap().value,
            TagValue::Unsigned(32)
        );
    }

    #[test]
    fn rejects_non_ebml_input() {
        let info = FileInfo::new("invalid.mkv".into(), 4, metra_core::FileFormat::Mkv);
        let error = read_matroska(
            &mut Cursor::new(b"nope".to_vec()),
            info,
            ParseLimits::default(),
        )
        .expect_err("non-EBML input should fail");
        assert!(matches!(error, MetraError::InvalidHeader { .. }));
    }
}
