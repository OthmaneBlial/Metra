use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use metra_core::{
    FileFormat, FileInfo, Metadata, MetraError, ParseLimits, Result, Source, Tag, TagValue,
    ValueType, Warning,
};

use crate::atomic::atomic_replace;
use crate::icc::parse_icc_profile;
use crate::iptc::parse_photoshop_resources;
use crate::iptc_writer::{
    IptcAction, delete_action as delete_iptc_action, new_photoshop_app13, rewrite_photoshop_app13,
    set_action as set_iptc_action,
};
use crate::tiff::parse_tiff_from_reader;
use crate::xmp::parse_xmp;

const XMP_PREFIX: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";
const ICC_PREFIX: &[u8] = b"ICC_PROFILE\0";

/// The first rewrite surface is intentionally narrow: JPEG COM segments are
/// self-contained, bounded, and can be changed without re-encoding pixels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JpegEdit {
    SetComment(String),
    DeleteComments,
    SetXmp(String),
    DeleteXmp,
    SetIptc { name: String, value: String },
    DeleteIptc { name: String },
    SetExifAscii { key: String, value: String },
}

#[derive(Debug, Default)]
struct IccAssembler {
    total: Option<u8>,
    fragments: BTreeMap<u8, Vec<u8>>,
    bytes: usize,
    first_data_offset: Option<u64>,
    oversized: bool,
}

pub fn read_jpeg<R: Read + Seek>(
    reader: &mut R,
    file_info: FileInfo,
    limits: ParseLimits,
) -> Result<Metadata> {
    let path = file_info.path.clone();
    let mut metadata = Metadata::new(file_info);
    let mut icc = IccAssembler::default();
    let mut soi = [0_u8; 2];
    read_exact(reader, &mut soi, &path)?;
    if soi != [0xFF, 0xD8] {
        return Err(MetraError::InvalidHeader {
            context: "JPEG".to_owned(),
            message: "missing SOI marker".to_owned(),
        });
    }

    let mut offset = 2_u64;
    let mut segments = 0_usize;
    let mut consumed_metadata = 0_usize;
    loop {
        if segments >= limits.max_jpeg_segments {
            metadata.add_warning(
                Warning::new(
                    "jpeg-segment-limit",
                    format!("stopped after {} JPEG segments", limits.max_jpeg_segments),
                )
                .at(offset),
            );
            break;
        }
        let marker = match read_marker(reader, &mut offset, &path)? {
            Some(marker) => marker,
            None => {
                metadata.add_warning(Warning::new(
                    "truncated-jpeg",
                    "JPEG ended before an EOI or SOS marker",
                ));
                break;
            }
        };
        if marker == 0xD9 || marker == 0xDA {
            break;
        }
        if (0xD0..=0xD7).contains(&marker) || marker == 0x01 {
            segments += 1;
            continue;
        }

        let mut length_bytes = [0_u8; 2];
        read_exact(reader, &mut length_bytes, &path)?;
        offset = offset.checked_add(2).ok_or(MetraError::InvalidOffset {
            context: "JPEG segment length".to_owned(),
            offset,
        })?;
        let segment_length = usize::from(u16::from_be_bytes(length_bytes));
        if segment_length < 2 {
            return Err(MetraError::InvalidTag {
                context: format!("JPEG marker 0xFF{marker:02X}"),
                message: "segment length must include its two length bytes".to_owned(),
            });
        }
        let data_length = segment_length - 2;
        let data_offset = offset;
        let remaining_budget = limits.max_metadata_bytes.saturating_sub(consumed_metadata);
        if data_length > remaining_budget {
            metadata.add_warning(
                Warning::new(
                    "jpeg-metadata-limit",
                    format!("skipped {data_length}-byte segment after metadata budget was reached"),
                )
                .at(data_offset),
            );
            seek_forward(reader, data_length, &path)?;
            offset = offset
                .checked_add(u64::try_from(data_length).unwrap_or(u64::MAX))
                .ok_or(MetraError::InvalidOffset {
                    context: "JPEG segment end".to_owned(),
                    offset: data_offset,
                })?;
            segments += 1;
            continue;
        }

        let mut data = vec![0_u8; data_length];
        read_exact(reader, &mut data, &path)?;
        consumed_metadata = consumed_metadata.saturating_add(data_length);
        offset = offset
            .checked_add(
                u64::try_from(data_length).map_err(|_| MetraError::InvalidOffset {
                    context: "JPEG segment length".to_owned(),
                    offset: data_length as u64,
                })?,
            )
            .ok_or(MetraError::InvalidOffset {
                context: "JPEG segment end".to_owned(),
                offset: data_offset,
            })?;
        process_segment(marker, &data, data_offset, &mut metadata, limits, &mut icc)?;
        segments += 1;
    }
    icc.finish(&mut metadata, limits);
    metadata.sort_tags();
    Ok(metadata)
}

/// Validate the source, stream a JPEG rewrite to the destination, and keep
/// every segment that is not explicitly targeted by `edits` byte-for-byte.
pub fn rewrite_jpeg<R: Read + Seek, W: Write>(
    reader: &mut R,
    writer: &mut W,
    file_info: FileInfo,
    limits: ParseLimits,
    edits: &[JpegEdit],
) -> Result<()> {
    read_jpeg(reader, file_info.clone(), limits)?;
    reader
        .seek(SeekFrom::Start(0))
        .map_err(|source| io_error(&file_info.path, source))?;
    rewrite_jpeg_stream(reader, writer, &file_info.path, limits, edits)
}

/// Rewrite a JPEG in memory and validate the resulting container before
/// returning it. This is useful for callers that want to preview or stage a
/// change without touching the source file.
pub fn rewrite_jpeg_to_vec(
    bytes: &[u8],
    file_info: FileInfo,
    limits: ParseLimits,
    edits: &[JpegEdit],
) -> Result<Vec<u8>> {
    let mut reader = std::io::Cursor::new(bytes);
    let mut output = Vec::new();
    rewrite_jpeg(&mut reader, &mut output, file_info.clone(), limits, edits)?;
    let mut validation_reader = std::io::Cursor::new(output.as_slice());
    read_jpeg(&mut validation_reader, file_info, limits)?;
    Ok(output)
}

/// Atomically replace a JPEG after validating both the source and rewritten
/// output. The temporary file is created beside the source so the final rename
/// stays on the same filesystem.
pub fn rewrite_jpeg_path(
    path: impl AsRef<Path>,
    limits: ParseLimits,
    edits: &[JpegEdit],
) -> Result<()> {
    let path = path.as_ref().to_path_buf();
    let source_metadata = fs::metadata(&path).map_err(|source| MetraError::Io {
        path: path.clone(),
        source,
    })?;
    let file_info = FileInfo::new(path.clone(), source_metadata.len(), FileFormat::Jpeg);
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
        rewrite_jpeg(&mut input, &mut output, file_info.clone(), limits, edits)?;
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
        let written_info = FileInfo::new(temp_path.clone(), written_size, FileFormat::Jpeg);
        read_jpeg(&mut validation, written_info, limits)?;
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
enum CommentAction {
    Set(Vec<u8>),
    Delete,
}

#[derive(Debug)]
enum XmpAction {
    Set(Vec<u8>),
    Delete,
}

fn rewrite_jpeg_stream<R: Read, W: Write>(
    reader: &mut R,
    writer: &mut W,
    path: &Path,
    limits: ParseLimits,
    edits: &[JpegEdit],
) -> Result<()> {
    let comment = comment_action(edits)?;
    let iptc = iptc_action(edits, limits)?;
    let xmp = xmp_action(edits, limits)?;
    let exif = exif_ascii_action(edits, limits)?;
    let mut soi = [0_u8; 2];
    read_exact(reader, &mut soi, path)?;
    if soi != [0xFF, 0xD8] {
        return Err(MetraError::InvalidHeader {
            context: "JPEG".to_owned(),
            message: "missing SOI marker".to_owned(),
        });
    }
    write_all(writer, &soi)?;

    let mut segments = 0_usize;
    let mut metadata_bytes = 0_usize;
    let mut comment_inserted = false;
    let mut iptc_inserted = false;
    let mut xmp_inserted = false;
    let mut exif_rewritten = false;
    loop {
        if segments >= limits.max_jpeg_segments {
            return Err(MetraError::ResourceLimitExceeded {
                resource: "JPEG segments during rewrite".to_owned(),
                limit: limits.max_jpeg_segments,
            });
        }
        let Some(marker_bytes) = read_marker_bytes(reader, path)? else {
            return Err(MetraError::CorruptMetadata {
                message: "JPEG ended before SOS or EOI during rewrite".to_owned(),
            });
        };
        let marker = *marker_bytes.last().unwrap_or(&0);
        if marker == 0xD9 || marker == 0xDA {
            if let Some(CommentAction::Set(comment)) = comment.as_ref()
                && !comment_inserted
            {
                write_comment(writer, comment)?;
            }
            if let Some(IptcAction::Set { .. }) = iptc.as_ref()
                && !iptc_inserted
            {
                let segment = new_photoshop_app13(iptc.as_ref().expect("IPTC action"), limits)?;
                write_all(writer, &segment)?;
            }
            if let Some(XmpAction::Set(value)) = xmp.as_ref()
                && !xmp_inserted
            {
                write_xmp(writer, value)?;
            }
            if !exif.is_empty() && !exif_rewritten {
                return Err(MetraError::WriteFailure {
                    message: "JPEG EXIF edit did not match an existing ASCII field".to_owned(),
                });
            }
            write_all(writer, &marker_bytes)?;
            if marker == 0xDA {
                copy_remainder(reader, writer, path)?;
            }
            return Ok(());
        }
        if (0xD0..=0xD7).contains(&marker) || marker == 0x01 {
            write_all(writer, &marker_bytes)?;
            segments += 1;
            continue;
        }

        let mut length_bytes = [0_u8; 2];
        read_exact(reader, &mut length_bytes, path)?;
        let segment_length = usize::from(u16::from_be_bytes(length_bytes));
        if segment_length < 2 {
            return Err(MetraError::InvalidTag {
                context: format!("JPEG marker 0xFF{marker:02X}"),
                message: "segment length must include its two length bytes".to_owned(),
            });
        }
        let data_length = segment_length - 2;
        metadata_bytes = metadata_bytes.saturating_add(data_length);
        if metadata_bytes > limits.max_metadata_bytes {
            return Err(MetraError::ResourceLimitExceeded {
                resource: "JPEG metadata during rewrite".to_owned(),
                limit: limits.max_metadata_bytes,
            });
        }
        let mut data = vec![0_u8; data_length];
        read_exact(reader, &mut data, path)?;
        if marker == 0xFE {
            match comment.as_ref() {
                Some(CommentAction::Set(comment)) if !comment_inserted => {
                    write_comment(writer, comment)?;
                    comment_inserted = true;
                }
                Some(CommentAction::Set(_)) | Some(CommentAction::Delete) => {}
                None => {
                    write_all(writer, &marker_bytes)?;
                    write_all(writer, &length_bytes)?;
                    write_all(writer, &data)?;
                }
            }
        } else if marker == 0xE1 && data.starts_with(b"Exif\0\0") {
            if !exif.is_empty() && !exif_rewritten {
                if let Some(updated) = rewrite_exif_ascii_segment(&data, limits, &exif)? {
                    write_segment(writer, &marker_bytes, &length_bytes, &updated)?;
                    exif_rewritten = true;
                } else {
                    write_segment(writer, &marker_bytes, &length_bytes, &data)?;
                }
            } else {
                write_segment(writer, &marker_bytes, &length_bytes, &data)?;
            }
        } else if marker == 0xE1 && data.starts_with(XMP_PREFIX) {
            if let Some(action) = xmp.as_ref() {
                match action {
                    XmpAction::Set(value) if !xmp_inserted => {
                        write_xmp(writer, value)?;
                        xmp_inserted = true;
                    }
                    XmpAction::Set(_) | XmpAction::Delete => {
                        xmp_inserted = true;
                    }
                }
            } else {
                write_all(writer, &marker_bytes)?;
                write_all(writer, &length_bytes)?;
                write_all(writer, &data)?;
            }
        } else if marker == 0xED {
            if let Some(iptc_action) = iptc.as_ref() {
                if let Some(updated) = rewrite_photoshop_app13(&data, iptc_action, limits)? {
                    write_app13_payload(writer, &updated)?;
                    iptc_inserted = true;
                } else {
                    write_all(writer, &marker_bytes)?;
                    write_all(writer, &length_bytes)?;
                    write_all(writer, &data)?;
                }
            } else {
                write_all(writer, &marker_bytes)?;
                write_all(writer, &length_bytes)?;
                write_all(writer, &data)?;
            }
        } else {
            write_all(writer, &marker_bytes)?;
            write_all(writer, &length_bytes)?;
            write_all(writer, &data)?;
        }
        segments += 1;
    }
}

fn comment_action(edits: &[JpegEdit]) -> Result<Option<CommentAction>> {
    let mut action = None;
    for edit in edits {
        match edit {
            JpegEdit::SetComment(comment) => {
                let bytes = comment.as_bytes().to_vec();
                let length =
                    bytes
                        .len()
                        .checked_add(2)
                        .ok_or_else(|| MetraError::WriteFailure {
                            message: "JPEG comment length overflowed".to_owned(),
                        })?;
                if u16::try_from(length).is_err() {
                    return Err(MetraError::WriteFailure {
                        message: "JPEG comment exceeds the 65533-byte segment limit".to_owned(),
                    });
                }
                action = Some(CommentAction::Set(bytes));
            }
            JpegEdit::DeleteComments => action = Some(CommentAction::Delete),
            JpegEdit::SetXmp(_) | JpegEdit::DeleteXmp => {}
            JpegEdit::SetIptc { .. } | JpegEdit::DeleteIptc { .. } => {}
            JpegEdit::SetExifAscii { .. } => {}
        }
    }
    Ok(action)
}

fn iptc_action(edits: &[JpegEdit], limits: ParseLimits) -> Result<Option<IptcAction>> {
    let mut action = None;
    for edit in edits {
        match edit {
            JpegEdit::SetIptc { name, value } => {
                action = Some(set_iptc_action(name, value, limits)?);
            }
            JpegEdit::DeleteIptc { name } => {
                action = Some(delete_iptc_action(name)?);
            }
            JpegEdit::SetComment(_)
            | JpegEdit::DeleteComments
            | JpegEdit::SetXmp(_)
            | JpegEdit::DeleteXmp
            | JpegEdit::SetExifAscii { .. } => {}
        }
    }
    Ok(action)
}

fn xmp_action(edits: &[JpegEdit], limits: ParseLimits) -> Result<Option<XmpAction>> {
    let mut action = None;
    for edit in edits {
        action = Some(match edit {
            JpegEdit::SetXmp(value) => {
                let bytes = value.as_bytes().to_vec();
                if bytes.len() > limits.max_value_bytes {
                    return Err(MetraError::ResourceLimitExceeded {
                        resource: "JPEG XMP packet".to_owned(),
                        limit: limits.max_value_bytes,
                    });
                }
                let mut validation = Metadata::new(FileInfo::new(
                    PathBuf::from("<memory>"),
                    bytes.len() as u64,
                    FileFormat::Jpeg,
                ));
                parse_xmp(&bytes, 0, "JPEG/XMP", &mut validation, limits)?;
                XmpAction::Set(bytes)
            }
            JpegEdit::DeleteXmp => XmpAction::Delete,
            JpegEdit::SetComment(_)
            | JpegEdit::DeleteComments
            | JpegEdit::SetIptc { .. }
            | JpegEdit::DeleteIptc { .. }
            | JpegEdit::SetExifAscii { .. } => continue,
        });
    }
    Ok(action)
}

#[derive(Debug, Clone)]
struct ExifAsciiEdit {
    key: String,
    value: String,
}

fn exif_ascii_action(edits: &[JpegEdit], limits: ParseLimits) -> Result<Vec<ExifAsciiEdit>> {
    let mut actions = Vec::new();
    for edit in edits {
        let JpegEdit::SetExifAscii { key, value } = edit else {
            continue;
        };
        if value.contains('\0') {
            return Err(MetraError::WriteFailure {
                message: format!("JPEG EXIF ASCII value for {key} cannot contain NUL"),
            });
        }
        let encoded_length =
            value
                .len()
                .checked_add(1)
                .ok_or_else(|| MetraError::WriteFailure {
                    message: format!("JPEG EXIF ASCII value for {key} length overflowed"),
                })?;
        if encoded_length > limits.max_value_bytes {
            return Err(MetraError::ResourceLimitExceeded {
                resource: "JPEG EXIF ASCII value".to_owned(),
                limit: limits.max_value_bytes,
            });
        }
        if actions
            .iter()
            .any(|action: &ExifAsciiEdit| action.key == *key)
        {
            return Err(MetraError::WriteFailure {
                message: format!("JPEG EXIF edit for {key} is repeated"),
            });
        }
        actions.push(ExifAsciiEdit {
            key: key.clone(),
            value: value.clone(),
        });
    }
    Ok(actions)
}

#[derive(Debug, Clone, Copy)]
enum ExifEndian {
    Little,
    Big,
}

#[derive(Debug, Clone, Copy)]
enum ExifLayout {
    Classic { endian: ExifEndian },
    Big { endian: ExifEndian },
}

impl ExifLayout {
    const fn endian(self) -> ExifEndian {
        match self {
            Self::Classic { endian } | Self::Big { endian } => endian,
        }
    }

    const fn entry_size(self) -> usize {
        match self {
            Self::Classic { .. } => 12,
            Self::Big { .. } => 20,
        }
    }

    const fn inline_size(self) -> usize {
        match self {
            Self::Classic { .. } => 4,
            Self::Big { .. } => 8,
        }
    }

    const fn value_field_start(self) -> usize {
        match self {
            Self::Classic { .. } => 8,
            Self::Big { .. } => 12,
        }
    }
}

#[derive(Debug, Clone)]
struct ExifPatch {
    offset: usize,
    span: usize,
    bytes: Vec<u8>,
}

fn rewrite_exif_ascii_segment(
    data: &[u8],
    limits: ParseLimits,
    edits: &[ExifAsciiEdit],
) -> Result<Option<Vec<u8>>> {
    if data.len() < 6 {
        return Err(MetraError::UnexpectedEof {
            context: "JPEG EXIF APP1".to_owned(),
        });
    }
    let tiff_data = &data[6..];
    let mut cursor = std::io::Cursor::new(tiff_data);
    let file_info = FileInfo::new(
        PathBuf::from("<jpeg-exif>"),
        tiff_data.len() as u64,
        FileFormat::Tiff,
    );
    let mut metadata = Metadata::new(file_info.clone());
    parse_tiff_from_reader(
        &mut cursor,
        0,
        tiff_data.len() as u64,
        6,
        &mut metadata,
        limits,
    )
    .map_err(|error| MetraError::WriteFailure {
        message: format!("cannot parse JPEG EXIF for rewrite: {error}"),
    })?;
    let layout = exif_layout(tiff_data)?;
    let mut patches = Vec::with_capacity(edits.len());
    let mut missing = Vec::new();
    for edit in edits {
        let matches = metadata.find_all(&edit.key);
        let Some(tag) = matches.as_slice().first().copied() else {
            missing.push(edit.key.as_str());
            continue;
        };
        if matches.len() != 1 {
            return Err(MetraError::WriteFailure {
                message: format!("JPEG EXIF tag {} is repeated", edit.key),
            });
        }
        if !matches!(tag.value, TagValue::String(_)) {
            return Err(MetraError::WriteFailure {
                message: format!("JPEG EXIF tag {} is not an ASCII string", edit.key),
            });
        }
        let source_offset = tag.source.offset.ok_or_else(|| MetraError::WriteFailure {
            message: format!("JPEG EXIF tag {} has no source offset", edit.key),
        })?;
        let entry_offset = usize::try_from(source_offset.checked_sub(6).ok_or_else(|| {
            MetraError::WriteFailure {
                message: format!("JPEG EXIF tag {} has an invalid source offset", edit.key),
            }
        })?)
        .map_err(|_| MetraError::InvalidOffset {
            context: format!("JPEG EXIF {} entry", edit.key),
            offset: source_offset,
        })?;
        let entry_end =
            entry_offset
                .checked_add(layout.entry_size())
                .ok_or(MetraError::InvalidOffset {
                    context: format!("JPEG EXIF {} entry", edit.key),
                    offset: source_offset,
                })?;
        let entry =
            tiff_data
                .get(entry_offset..entry_end)
                .ok_or_else(|| MetraError::UnexpectedEof {
                    context: format!("JPEG EXIF {} entry", edit.key),
                })?;
        let endian = layout.endian();
        if read_exif_u16(entry, 2, endian) != Some(2) {
            return Err(MetraError::WriteFailure {
                message: format!("JPEG EXIF tag {} is not an existing ASCII value", edit.key),
            });
        }
        let count = read_exif_count(entry, layout)?;
        let count_usize =
            usize::try_from(count).map_err(|_| MetraError::ResourceLimitExceeded {
                resource: "JPEG EXIF ASCII value".to_owned(),
                limit: limits.max_value_bytes,
            })?;
        if count == 0 || count_usize > limits.max_value_bytes {
            return Err(MetraError::ResourceLimitExceeded {
                resource: "JPEG EXIF ASCII value".to_owned(),
                limit: limits.max_value_bytes,
            });
        }
        let mut replacement = edit.value.as_bytes().to_vec();
        replacement.push(0);
        if replacement.len() > count_usize {
            return Err(MetraError::WriteFailure {
                message: format!(
                    "JPEG EXIF ASCII value for {} needs {} bytes but the field stores {count} bytes",
                    edit.key,
                    replacement.len()
                ),
            });
        }
        replacement.resize(count_usize, 0);
        let value_offset = if count_usize <= layout.inline_size() {
            entry_offset.checked_add(layout.value_field_start()).ok_or(
                MetraError::InvalidOffset {
                    context: format!("JPEG EXIF inline value {}", edit.key),
                    offset: source_offset,
                },
            )?
        } else {
            read_exif_offset(entry, layout)?
        };
        let value_end = value_offset
            .checked_add(count_usize)
            .ok_or(MetraError::InvalidOffset {
                context: format!("JPEG EXIF value {}", edit.key),
                offset: value_offset as u64,
            })?;
        if value_end > tiff_data.len() {
            return Err(MetraError::UnexpectedEof {
                context: format!("JPEG EXIF value {}", edit.key),
            });
        }
        patches.push(ExifPatch {
            offset: value_offset + 6,
            span: count_usize,
            bytes: replacement,
        });
    }
    if patches.is_empty() {
        return Ok(None);
    }
    if let Some(key) = missing.first() {
        return Err(MetraError::WriteFailure {
            message: format!("JPEG EXIF tag {key} does not exist; insertion is not supported"),
        });
    }
    patches.sort_by_key(|patch| patch.offset);
    for pair in patches.windows(2) {
        let previous_end =
            pair[0]
                .offset
                .checked_add(pair[0].span)
                .ok_or(MetraError::InvalidOffset {
                    context: "JPEG EXIF rewrite patch".to_owned(),
                    offset: pair[0].offset as u64,
                })?;
        if previous_end > pair[1].offset {
            return Err(MetraError::WriteFailure {
                message: "JPEG EXIF rewrite patches overlap".to_owned(),
            });
        }
    }
    let mut updated = data.to_vec();
    for patch in patches {
        let end = patch.offset + patch.span;
        updated[patch.offset..end].copy_from_slice(&patch.bytes);
    }
    Ok(Some(updated))
}

fn exif_layout(bytes: &[u8]) -> Result<ExifLayout> {
    let header = bytes.get(..8).ok_or_else(|| MetraError::UnexpectedEof {
        context: "JPEG EXIF TIFF header".to_owned(),
    })?;
    let endian = match &header[..2] {
        b"II" => ExifEndian::Little,
        b"MM" => ExifEndian::Big,
        _ => {
            return Err(MetraError::InvalidHeader {
                context: "JPEG EXIF TIFF".to_owned(),
                message: "byte order must be II or MM".to_owned(),
            });
        }
    };
    match read_exif_u16(header, 2, endian) {
        Some(42) => Ok(ExifLayout::Classic { endian }),
        Some(43) => {
            let extended = bytes.get(4..16).ok_or_else(|| MetraError::UnexpectedEof {
                context: "JPEG EXIF BigTIFF header".to_owned(),
            })?;
            if read_exif_u16(extended, 0, endian) != Some(8)
                || read_exif_u16(extended, 2, endian) != Some(0)
            {
                return Err(MetraError::InvalidHeader {
                    context: "JPEG EXIF BigTIFF".to_owned(),
                    message: "invalid offset-size or reserved field".to_owned(),
                });
            }
            Ok(ExifLayout::Big { endian })
        }
        Some(magic) => Err(MetraError::InvalidHeader {
            context: "JPEG EXIF TIFF".to_owned(),
            message: format!("expected magic 42 or 43, got {magic}"),
        }),
        None => Err(MetraError::UnexpectedEof {
            context: "JPEG EXIF TIFF magic".to_owned(),
        }),
    }
}

fn read_exif_count(entry: &[u8], layout: ExifLayout) -> Result<u64> {
    match layout {
        ExifLayout::Classic { endian } => read_exif_u32(entry, 4, endian)
            .map(u64::from)
            .ok_or_else(|| MetraError::UnexpectedEof {
                context: "JPEG EXIF ASCII count".to_owned(),
            }),
        ExifLayout::Big { endian } => {
            read_exif_u64(entry, 4, endian).ok_or_else(|| MetraError::UnexpectedEof {
                context: "JPEG EXIF BigTIFF ASCII count".to_owned(),
            })
        }
    }
}

fn read_exif_offset(entry: &[u8], layout: ExifLayout) -> Result<usize> {
    let offset = match layout {
        ExifLayout::Classic { endian } => read_exif_u32(entry, 8, endian).map(u64::from),
        ExifLayout::Big { endian } => read_exif_u64(entry, 12, endian),
    }
    .ok_or_else(|| MetraError::UnexpectedEof {
        context: "JPEG EXIF ASCII offset".to_owned(),
    })?;
    usize::try_from(offset).map_err(|_| MetraError::InvalidOffset {
        context: "JPEG EXIF ASCII offset".to_owned(),
        offset,
    })
}

fn read_exif_u16(bytes: &[u8], offset: usize, endian: ExifEndian) -> Option<u16> {
    let bytes = bytes.get(offset..offset.checked_add(2)?)?;
    let bytes: [u8; 2] = bytes.try_into().ok()?;
    Some(match endian {
        ExifEndian::Little => u16::from_le_bytes(bytes),
        ExifEndian::Big => u16::from_be_bytes(bytes),
    })
}

fn read_exif_u32(bytes: &[u8], offset: usize, endian: ExifEndian) -> Option<u32> {
    let bytes = bytes.get(offset..offset.checked_add(4)?)?;
    let bytes: [u8; 4] = bytes.try_into().ok()?;
    Some(match endian {
        ExifEndian::Little => u32::from_le_bytes(bytes),
        ExifEndian::Big => u32::from_be_bytes(bytes),
    })
}

fn read_exif_u64(bytes: &[u8], offset: usize, endian: ExifEndian) -> Option<u64> {
    let bytes = bytes.get(offset..offset.checked_add(8)?)?;
    let bytes: [u8; 8] = bytes.try_into().ok()?;
    Some(match endian {
        ExifEndian::Little => u64::from_le_bytes(bytes),
        ExifEndian::Big => u64::from_be_bytes(bytes),
    })
}

fn write_segment<W: Write>(
    writer: &mut W,
    marker: &[u8],
    length: &[u8; 2],
    data: &[u8],
) -> Result<()> {
    write_all(writer, marker)?;
    write_all(writer, length)?;
    write_all(writer, data)
}

fn write_comment<W: Write>(writer: &mut W, comment: &[u8]) -> Result<()> {
    let length = u16::try_from(comment.len() + 2).map_err(|_| MetraError::WriteFailure {
        message: "JPEG comment exceeds the 65533-byte segment limit".to_owned(),
    })?;
    write_all(writer, &[0xFF, 0xFE])?;
    write_all(writer, &length.to_be_bytes())?;
    write_all(writer, comment)
}

fn write_xmp<W: Write>(writer: &mut W, value: &[u8]) -> Result<()> {
    let data_length =
        XMP_PREFIX
            .len()
            .checked_add(value.len())
            .ok_or_else(|| MetraError::WriteFailure {
                message: "JPEG XMP packet length overflowed".to_owned(),
            })?;
    let segment_length = data_length
        .checked_add(2)
        .ok_or_else(|| MetraError::WriteFailure {
            message: "JPEG XMP segment length overflowed".to_owned(),
        })?;
    let segment_length = u16::try_from(segment_length).map_err(|_| MetraError::WriteFailure {
        message: "JPEG XMP packet exceeds the 65533-byte segment limit".to_owned(),
    })?;
    write_all(writer, &[0xFF, 0xE1])?;
    write_all(writer, &segment_length.to_be_bytes())?;
    write_all(writer, XMP_PREFIX)?;
    write_all(writer, value)
}

fn write_app13_payload<W: Write>(writer: &mut W, data: &[u8]) -> Result<()> {
    let segment_length = data
        .len()
        .checked_add(2)
        .ok_or_else(|| MetraError::WriteFailure {
            message: "JPEG APP13 length overflowed".to_owned(),
        })?;
    let segment_length = u16::try_from(segment_length).map_err(|_| MetraError::WriteFailure {
        message: "JPEG IPTC APP13 exceeds the 65533-byte segment limit".to_owned(),
    })?;
    write_all(writer, &[0xFF, 0xED])?;
    write_all(writer, &segment_length.to_be_bytes())?;
    write_all(writer, data)
}

fn read_marker_bytes<R: Read>(reader: &mut R, path: &Path) -> Result<Option<Vec<u8>>> {
    let mut first = [0_u8; 1];
    match reader.read(&mut first) {
        Ok(0) => return Ok(None),
        Ok(1) => {}
        Ok(_) => unreachable!("a one-byte buffer cannot read more than one byte"),
        Err(error) => return Err(io_error(path, error)),
    }
    if first[0] != 0xFF {
        return Err(MetraError::CorruptMetadata {
            message: "expected JPEG marker during rewrite".to_owned(),
        });
    }
    let mut marker = vec![first[0]];
    loop {
        let mut byte = [0_u8; 1];
        read_exact(reader, &mut byte, path)?;
        marker.push(byte[0]);
        if byte[0] != 0xFF {
            break;
        }
    }
    if marker.last() == Some(&0x00) {
        return Err(MetraError::CorruptMetadata {
            message: "entropy-coded data appeared before SOS during rewrite".to_owned(),
        });
    }
    Ok(Some(marker))
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

fn write_all<W: Write>(writer: &mut W, bytes: &[u8]) -> Result<()> {
    writer
        .write_all(bytes)
        .map_err(|source| MetraError::WriteFailure {
            message: source.to_string(),
        })
}

fn temporary_path(path: &Path) -> Result<PathBuf> {
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("metadata.jpg");
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

fn read_marker<R: Read>(reader: &mut R, offset: &mut u64, path: &Path) -> Result<Option<u8>> {
    let mut byte = [0_u8; 1];
    match reader.read_exact(&mut byte) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => {
            return Ok(None);
        }
        Err(error) => return Err(io_error(path, error)),
    }
    *offset = offset.saturating_add(1);
    if byte[0] != 0xFF {
        return Err(MetraError::CorruptMetadata {
            message: format!(
                "expected JPEG marker at offset {}",
                offset.saturating_sub(1)
            ),
        });
    }
    loop {
        read_exact(reader, &mut byte, path)?;
        *offset = offset.saturating_add(1);
        if byte[0] != 0xFF {
            break;
        }
    }
    if byte[0] == 0x00 {
        return Err(MetraError::CorruptMetadata {
            message: "entropy-coded data appeared before SOS".to_owned(),
        });
    }
    Ok(Some(byte[0]))
}

impl IccAssembler {
    fn add(&mut self, data: &[u8], data_offset: u64, metadata: &mut Metadata, limits: ParseLimits) {
        let Some(sequence_offset) = ICC_PREFIX.len().checked_add(1) else {
            return;
        };
        if data.len() < sequence_offset + 1 {
            metadata.add_warning(
                Warning::new("truncated-icc", "ICC APP2 header is truncated").at(data_offset),
            );
            return;
        }
        let sequence = data[sequence_offset - 1];
        let total = data[sequence_offset];
        if total == 0 {
            metadata.add_warning(
                Warning::new("invalid-icc-fragment", "ICC profile fragment count is zero")
                    .at(data_offset),
            );
            return;
        }
        if sequence == 0 || sequence > total {
            metadata.add_warning(
                Warning::new(
                    "invalid-icc-fragment",
                    format!("ICC profile sequence {sequence} is outside 1..={total}"),
                )
                .at(data_offset),
            );
            return;
        }
        if let Some(expected_total) = self.total {
            if expected_total != total {
                metadata.add_warning(
                    Warning::new(
                        "icc-fragment-count",
                        format!(
                            "ICC profile fragment {sequence} declares {total} parts; expected {expected_total}"
                        ),
                    )
                    .at(data_offset),
                );
                return;
            }
        } else {
            self.total = Some(total);
        }
        if self.fragments.contains_key(&sequence) {
            metadata.add_warning(
                Warning::new(
                    "duplicate-icc-fragment",
                    format!("ICC profile fragment {sequence} was repeated"),
                )
                .at(data_offset),
            );
            return;
        }

        let payload = &data[sequence_offset + 1..];
        let Some(new_size) = self.bytes.checked_add(payload.len()) else {
            self.oversized = true;
            metadata.add_warning(
                Warning::new(
                    "icc-profile-limit",
                    "ICC profile size overflowed the read budget",
                )
                .at(data_offset),
            );
            return;
        };
        if new_size > limits.max_value_bytes {
            self.oversized = true;
            metadata.add_warning(
                Warning::new(
                    "icc-profile-limit",
                    format!(
                        "ICC profile exceeds the {}-byte value budget",
                        limits.max_value_bytes
                    ),
                )
                .at(data_offset),
            );
            return;
        }
        self.bytes = new_size;
        self.first_data_offset.get_or_insert(
            data_offset.saturating_add(u64::try_from(sequence_offset + 1).unwrap_or(u64::MAX)),
        );
        self.fragments.insert(sequence, payload.to_vec());
    }

    fn finish(&self, metadata: &mut Metadata, limits: ParseLimits) {
        let Some(total) = self.total else {
            return;
        };
        if self.oversized {
            return;
        }
        let expected = usize::from(total);
        if self.fragments.len() != expected {
            metadata.add_warning(
                Warning::new(
                    "missing-icc-fragment",
                    format!(
                        "ICC profile contains {} of {expected} fragments",
                        self.fragments.len()
                    ),
                )
                .at(self.first_data_offset.unwrap_or_default()),
            );
            return;
        }
        let mut profile = Vec::with_capacity(self.bytes);
        for sequence in 1..=total {
            let Some(fragment) = self.fragments.get(&sequence) else {
                metadata.add_warning(
                    Warning::new(
                        "missing-icc-fragment",
                        format!("ICC profile is missing fragment {sequence} of {total}"),
                    )
                    .at(self.first_data_offset.unwrap_or_default()),
                );
                return;
            };
            profile.extend_from_slice(fragment);
        }
        if let Err(error) = parse_icc_profile(
            &profile,
            self.first_data_offset.unwrap_or_default(),
            metadata,
            limits,
        ) {
            metadata.add_warning(
                Warning::new("invalid-icc", error.to_string())
                    .at(self.first_data_offset.unwrap_or_default()),
            );
        }
    }
}

fn process_segment(
    marker: u8,
    data: &[u8],
    data_offset: u64,
    metadata: &mut Metadata,
    limits: ParseLimits,
    icc: &mut IccAssembler,
) -> Result<()> {
    match marker {
        0xE1 if data.starts_with(b"Exif\0\0") => {
            if data.len() <= 6 {
                metadata.add_warning(Warning::new(
                    "empty-exif",
                    "JPEG APP1 segment has an EXIF header but no TIFF payload",
                ));
                return Ok(());
            }
            let tiff_data = &data[6..];
            let mut cursor = std::io::Cursor::new(tiff_data);
            if let Err(error) = parse_tiff_from_reader(
                &mut cursor,
                0,
                tiff_data.len() as u64,
                data_offset.saturating_add(6),
                metadata,
                limits,
            ) {
                metadata.add_warning(
                    Warning::new("invalid-exif", error.to_string()).at(data_offset + 6),
                );
            }
        }
        0xE1 if data.starts_with(XMP_PREFIX) => {
            let prefix_len = XMP_PREFIX.len();
            if let Err(error) = parse_xmp(
                &data[prefix_len..],
                data_offset.saturating_add(prefix_len as u64),
                "JPEG/APP1-XMP",
                metadata,
                limits,
            ) {
                metadata
                    .add_warning(Warning::new("invalid-xmp", error.to_string()).at(data_offset));
            }
        }
        0xE2 if data.starts_with(ICC_PREFIX) => icc.add(data, data_offset, metadata, limits),
        0xED if data.starts_with(b"Photoshop 3.0\0") => {
            if let Err(error) = parse_photoshop_resources(data, data_offset, metadata, limits) {
                metadata.add_warning(
                    Warning::new("invalid-photoshop", error.to_string()).at(data_offset),
                );
            }
        }
        0xE0 if data.starts_with(b"JFIF\0") => parse_jfif(data, data_offset, metadata),
        0xFE => {
            let value = String::from_utf8_lossy(data).into_owned();
            metadata.add_tag(Tag {
                namespace: "JPEG".to_owned(),
                group: "COM".to_owned(),
                id: None,
                name: "Comment".to_owned(),
                description: Some("JPEG comment".to_owned()),
                raw_value: Some(data.to_vec()),
                value: TagValue::String(value),
                value_type: ValueType::String,
                source: Source::new("JPEG/COM", Some(data_offset), Some(data.len() as u64)),
                writable: false,
            });
        }
        _ => {}
    }
    Ok(())
}

fn parse_jfif(data: &[u8], data_offset: u64, metadata: &mut Metadata) {
    if data.len() < 14 {
        metadata.add_warning(
            Warning::new(
                "truncated-jfif",
                "JFIF segment is shorter than its fixed header",
            )
            .at(data_offset),
        );
        return;
    }
    let version = format!("{}.{}", data[5], data[6]);
    let units = match data[7] {
        0 => "None",
        1 => "inches",
        2 => "cm",
        other => {
            metadata.add_warning(
                Warning::new(
                    "invalid-jfif-units",
                    format!("unknown JFIF density unit {other}"),
                )
                .at(data_offset.saturating_add(7)),
            );
            "Unknown"
        }
    };
    add_jpeg_tag(
        metadata,
        "JFIFVersion",
        TagValue::String(version),
        ValueType::String,
        data_offset.saturating_add(5),
        2,
    );
    add_jpeg_tag(
        metadata,
        "ResolutionUnit",
        TagValue::String(units.to_owned()),
        ValueType::String,
        data_offset.saturating_add(7),
        1,
    );
    add_jpeg_tag(
        metadata,
        "XResolution",
        TagValue::Unsigned(u64::from(u16::from_be_bytes([data[8], data[9]]))),
        ValueType::UnsignedInteger,
        data_offset.saturating_add(8),
        2,
    );
    add_jpeg_tag(
        metadata,
        "YResolution",
        TagValue::Unsigned(u64::from(u16::from_be_bytes([data[10], data[11]]))),
        ValueType::UnsignedInteger,
        data_offset.saturating_add(10),
        2,
    );
}

fn add_jpeg_tag(
    metadata: &mut Metadata,
    name: &str,
    value: TagValue,
    value_type: ValueType,
    offset: u64,
    length: u64,
) {
    metadata.add_tag(Tag {
        namespace: "JFIF".to_owned(),
        group: "APP0".to_owned(),
        id: None,
        name: name.to_owned(),
        description: Some("JFIF container property".to_owned()),
        raw_value: None,
        value,
        value_type,
        source: Source::new("JPEG/APP0", Some(offset), Some(length)),
        writable: false,
    });
}

fn seek_forward<R: Seek>(reader: &mut R, length: usize, path: &Path) -> Result<()> {
    let distance = i64::try_from(length).map_err(|_| MetraError::InvalidOffset {
        context: "JPEG segment skip".to_owned(),
        offset: length as u64,
    })?;
    reader
        .seek(SeekFrom::Current(distance))
        .map(|_| ())
        .map_err(|source| io_error(path, source))
}

fn read_exact<R: Read>(reader: &mut R, buffer: &mut [u8], path: &Path) -> Result<()> {
    reader
        .read_exact(buffer)
        .map_err(|source| io_error(path, source))
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
    use metra_core::FileFormat;

    fn jpeg_with_comment() -> Vec<u8> {
        let comment = b"hello from Metra";
        let length = u16::try_from(comment.len() + 2).unwrap();
        let mut bytes = vec![0xFF, 0xD8, 0xFF, 0xFE];
        bytes.extend_from_slice(&length.to_be_bytes());
        bytes.extend_from_slice(comment);
        bytes.extend_from_slice(&[0xFF, 0xD9]);
        bytes
    }

    fn jpeg_with_unknown_segment_and_comment() -> Vec<u8> {
        let comment = b"before";
        let comment_length = u16::try_from(comment.len() + 2).unwrap();
        let mut bytes = vec![0xFF, 0xD8, 0xFF, 0xE2, 0, 4, 1, 2];
        bytes.extend_from_slice(&[0xFF, 0xFE]);
        bytes.extend_from_slice(&comment_length.to_be_bytes());
        bytes.extend_from_slice(comment);
        bytes.extend_from_slice(&[0xFF, 0xD9]);
        bytes
    }

    fn photoshop_resource(id: u16, value: &[u8]) -> Vec<u8> {
        let mut resource = b"8BIM".to_vec();
        resource.extend_from_slice(&id.to_be_bytes());
        resource.extend_from_slice(&[0, 0]);
        resource.extend_from_slice(&(value.len() as u32).to_be_bytes());
        resource.extend_from_slice(value);
        if value.len() & 1 == 1 {
            resource.push(0);
        }
        resource
    }

    fn iptc_dataset(number: u8, value: &[u8]) -> Vec<u8> {
        let mut dataset = vec![0x1C, 2, number];
        dataset.extend_from_slice(&(value.len() as u16).to_be_bytes());
        dataset.extend_from_slice(value);
        dataset
    }

    fn jpeg_with_iptc() -> Vec<u8> {
        let mut app13 = b"Photoshop 3.0\0".to_vec();
        app13.extend_from_slice(&photoshop_resource(0x0400, b"preserve me"));
        let mut iptc = iptc_dataset(25, b"before");
        iptc.extend_from_slice(&iptc_dataset(120, b"old caption"));
        app13.extend_from_slice(&photoshop_resource(0x0404, &iptc));
        let length = u16::try_from(app13.len() + 2).unwrap();
        let mut bytes = vec![0xFF, 0xD8, 0xFF, 0xED];
        bytes.extend_from_slice(&length.to_be_bytes());
        bytes.extend_from_slice(&app13);
        bytes.extend_from_slice(&[0xFF, 0xD9]);
        bytes
    }

    fn jpeg_with_xmp(format: &str) -> Vec<u8> {
        let packet = format!(
            "<x:xmpmeta><rdf:RDF><rdf:Description xmlns:dc=\"urn:dc\" dc:format=\"{format}\"/></rdf:RDF></x:xmpmeta>"
        );
        let mut app1 = XMP_PREFIX.to_vec();
        app1.extend_from_slice(packet.as_bytes());
        let length = u16::try_from(app1.len() + 2).unwrap();
        let mut bytes = vec![0xFF, 0xD8, 0xFF, 0xE1];
        bytes.extend_from_slice(&length.to_be_bytes());
        bytes.extend_from_slice(&app1);
        bytes.extend_from_slice(&[0xFF, 0xD9]);
        bytes
    }

    fn jpeg_with_exif_make(make: &str) -> Vec<u8> {
        let mut tiff = vec![
            b'I', b'I', 42, 0, 8, 0, 0, 0, // little-endian TIFF header
            1, 0, // one IFD0 entry
            0x0F, 0x01, 2, 0,
        ];
        let count = u32::try_from(make.len() + 1).unwrap();
        tiff.extend_from_slice(&count.to_le_bytes());
        tiff.extend_from_slice(&26_u32.to_le_bytes());
        tiff.extend_from_slice(&[0, 0, 0, 0]);
        assert_eq!(tiff.len(), 26);
        tiff.extend_from_slice(make.as_bytes());
        tiff.push(0);

        let mut app1 = b"Exif\0\0".to_vec();
        app1.extend_from_slice(&tiff);
        let length = u16::try_from(app1.len() + 2).unwrap();
        let mut bytes = vec![0xFF, 0xD8, 0xFF, 0xE1];
        bytes.extend_from_slice(&length.to_be_bytes());
        bytes.extend_from_slice(&app1);
        bytes.extend_from_slice(&[0xFF, 0xD9]);
        bytes
    }

    fn jpeg_with_fragmented_icc() -> Vec<u8> {
        let mut profile = [0_u8; 132];
        let profile_size = profile.len() as u32;
        profile[0..4].copy_from_slice(&profile_size.to_be_bytes());
        profile[8] = 4;
        profile[9] = 0x30;
        profile[12..16].copy_from_slice(b"mntr");
        profile[16..20].copy_from_slice(b"RGB ");
        profile[20..24].copy_from_slice(b"XYZ ");
        profile[36..40].copy_from_slice(b"acsp");
        profile[40..44].copy_from_slice(b"APPL");
        profile[48..52].copy_from_slice(b"TEST");
        profile[52..56].copy_from_slice(b"MODL");
        profile[64..68].copy_from_slice(&1_u32.to_be_bytes());

        let split = 60;
        let mut bytes = vec![0xFF, 0xD8];
        for (sequence, fragment) in [(1_u8, &profile[..split]), (2_u8, &profile[split..])] {
            let mut app2 = ICC_PREFIX.to_vec();
            app2.extend_from_slice(&[sequence, 2]);
            app2.extend_from_slice(fragment);
            let length = u16::try_from(app2.len() + 2).unwrap();
            bytes.extend_from_slice(&[0xFF, 0xE2]);
            bytes.extend_from_slice(&length.to_be_bytes());
            bytes.extend_from_slice(&app2);
        }
        bytes.extend_from_slice(&[0xFF, 0xD9]);
        bytes
    }

    #[test]
    fn reads_jpeg_comment_without_decoding_pixels() {
        let bytes = jpeg_with_comment();
        let info = FileInfo::new("comment.jpg".into(), bytes.len() as u64, FileFormat::Jpeg);
        let metadata = read_jpeg(&mut Cursor::new(bytes), info, ParseLimits::default()).unwrap();
        assert_eq!(
            metadata.find("JPEG:Comment").unwrap().display_value(),
            "hello from Metra"
        );
    }

    #[test]
    fn reads_structured_xmp_from_app1() {
        let packet = br#"<x:xmpmeta><rdf:RDF><rdf:Description dc:format="image/jpeg" xmlns:dc="urn:dc"/></rdf:RDF></x:xmpmeta>"#;
        let mut data = b"http://ns.adobe.com/xap/1.0/\0".to_vec();
        data.extend_from_slice(packet);
        let length = u16::try_from(data.len() + 2).unwrap();
        let mut bytes = vec![0xFF, 0xD8, 0xFF, 0xE1];
        bytes.extend_from_slice(&length.to_be_bytes());
        bytes.extend_from_slice(&data);
        bytes.extend_from_slice(&[0xFF, 0xD9]);
        let info = FileInfo::new("xmp.jpg".into(), bytes.len() as u64, FileFormat::Jpeg);
        let metadata = read_jpeg(&mut Cursor::new(bytes), info, ParseLimits::default()).unwrap();
        assert_eq!(
            metadata.find("XMP:dc:format").unwrap().display_value(),
            "image/jpeg"
        );
    }

    #[test]
    fn rewrites_existing_exif_ascii_without_changing_jpeg_layout() {
        let bytes = jpeg_with_exif_make("Canon");
        let output = rewrite_jpeg_to_vec(
            &bytes,
            FileInfo::new("exif.jpg".into(), bytes.len() as u64, FileFormat::Jpeg),
            ParseLimits::default(),
            &[JpegEdit::SetExifAscii {
                key: "EXIF:Make".to_owned(),
                value: "Sony".to_owned(),
            }],
        )
        .unwrap();
        assert_eq!(output.len(), bytes.len());
        assert_eq!(output[0..6], bytes[0..6]);
        assert_eq!(output[output.len() - 2..], bytes[bytes.len() - 2..]);
        let metadata = read_jpeg(
            &mut Cursor::new(output),
            FileInfo::new("exif.jpg".into(), 0, FileFormat::Jpeg),
            ParseLimits::default(),
        )
        .unwrap();
        assert_eq!(metadata.find("EXIF:Make").unwrap().display_value(), "Sony");
    }

    #[test]
    fn rejects_exif_ascii_growth_and_missing_fields() {
        let bytes = jpeg_with_exif_make("Canon");
        let growth = rewrite_jpeg_to_vec(
            &bytes,
            FileInfo::new("exif.jpg".into(), bytes.len() as u64, FileFormat::Jpeg),
            ParseLimits::default(),
            &[JpegEdit::SetExifAscii {
                key: "EXIF:Make".to_owned(),
                value: "Sony Alpha".to_owned(),
            }],
        )
        .unwrap_err();
        assert!(growth.to_string().contains("needs"));

        let missing = rewrite_jpeg_to_vec(
            &bytes,
            FileInfo::new("exif.jpg".into(), bytes.len() as u64, FileFormat::Jpeg),
            ParseLimits::default(),
            &[JpegEdit::SetExifAscii {
                key: "EXIF:Software".to_owned(),
                value: "Metra".to_owned(),
            }],
        )
        .unwrap_err();
        assert!(
            missing
                .to_string()
                .contains("did not match an existing ASCII field")
        );
    }

    #[test]
    fn reassembles_fragmented_icc_profiles() {
        let bytes = jpeg_with_fragmented_icc();
        let metadata = read_jpeg(
            &mut Cursor::new(bytes),
            FileInfo::new("fragmented.icc.jpg".into(), 0, FileFormat::Jpeg),
            ParseLimits::default(),
        )
        .unwrap();
        assert_eq!(
            metadata.find("ICC:DeviceClass").unwrap().display_value(),
            "mntr"
        );
        assert_eq!(
            metadata.find("ICC:ColorSpace").unwrap().display_value(),
            "RGB"
        );
        assert!(
            !metadata
                .warnings
                .iter()
                .any(|warning| warning.code == "icc-fragment")
        );
    }

    #[test]
    fn replaces_deletes_and_inserts_jpeg_xmp() {
        let bytes = jpeg_with_xmp("before");
        let replacement = r#"<x:xmpmeta><rdf:RDF><rdf:Description xmlns:dc="urn:dc" dc:format="after"/></rdf:RDF></x:xmpmeta>"#;
        let output = rewrite_jpeg_to_vec(
            &bytes,
            FileInfo::new("xmp.jpg".into(), bytes.len() as u64, FileFormat::Jpeg),
            ParseLimits::default(),
            &[JpegEdit::SetXmp(replacement.to_owned())],
        )
        .unwrap();
        assert!(
            !output
                .windows(b"before".len())
                .any(|window| window == b"before")
        );
        let metadata = read_jpeg(
            &mut Cursor::new(output),
            FileInfo::new("xmp.jpg".into(), 0, FileFormat::Jpeg),
            ParseLimits::default(),
        )
        .unwrap();
        assert_eq!(
            metadata.find("XMP:dc:format").unwrap().display_value(),
            "after"
        );

        let deleted = rewrite_jpeg_to_vec(
            &bytes,
            FileInfo::new("xmp.jpg".into(), bytes.len() as u64, FileFormat::Jpeg),
            ParseLimits::default(),
            &[JpegEdit::DeleteXmp],
        )
        .unwrap();
        assert!(
            !deleted
                .windows(XMP_PREFIX.len())
                .any(|window| window == XMP_PREFIX)
        );
        assert!(
            read_jpeg(
                &mut Cursor::new(deleted),
                FileInfo::new("xmp.jpg".into(), 0, FileFormat::Jpeg),
                ParseLimits::default(),
            )
            .unwrap()
            .find("XMP:Packet")
            .is_none()
        );

        let inserted = rewrite_jpeg_to_vec(
            &[0xFF, 0xD8, 0xFF, 0xD9],
            FileInfo::new("new-xmp.jpg".into(), 4, FileFormat::Jpeg),
            ParseLimits::default(),
            &[JpegEdit::SetXmp(replacement.to_owned())],
        )
        .unwrap();
        assert!(
            inserted
                .windows(XMP_PREFIX.len())
                .any(|window| window == XMP_PREFIX)
        );
    }

    #[test]
    fn rejects_invalid_jpeg_xmp_before_writing() {
        let error = rewrite_jpeg_to_vec(
            &[0xFF, 0xD8, 0xFF, 0xD9],
            FileInfo::new("xmp.jpg".into(), 4, FileFormat::Jpeg),
            ParseLimits::default(),
            &[JpegEdit::SetXmp("<broken>".to_owned())],
        )
        .unwrap_err();
        assert!(error.to_string().contains("XML"));
    }

    #[test]
    fn malformed_segment_length_is_rejected() {
        let bytes = vec![0xFF, 0xD8, 0xFF, 0xE1, 0, 1];
        let info = FileInfo::new("bad.jpg".into(), bytes.len() as u64, FileFormat::Jpeg);
        let result = read_jpeg(&mut Cursor::new(bytes), info, ParseLimits::default());
        assert!(matches!(result, Err(MetraError::InvalidTag { .. })));
    }

    #[test]
    fn rewrites_comment_and_preserves_unknown_segments() {
        let bytes = jpeg_with_unknown_segment_and_comment();
        let info = FileInfo::new("rewrite.jpg".into(), bytes.len() as u64, FileFormat::Jpeg);
        let output = rewrite_jpeg_to_vec(
            &bytes,
            info.clone(),
            ParseLimits::default(),
            &[JpegEdit::SetComment("after".to_owned())],
        )
        .unwrap();
        assert!(
            output
                .windows(6)
                .any(|window| window == [0xFF, 0xE2, 0, 4, 1, 2])
        );
        let metadata = read_jpeg(
            &mut Cursor::new(output),
            FileInfo::new("rewrite.jpg".into(), 0, FileFormat::Jpeg),
            ParseLimits::default(),
        )
        .unwrap();
        assert_eq!(
            metadata.find("JPEG:Comment").unwrap().display_value(),
            "after"
        );
    }

    #[test]
    fn deletes_all_comments_and_can_insert_a_new_one() {
        let bytes = jpeg_with_unknown_segment_and_comment();
        let info = FileInfo::new("rewrite.jpg".into(), bytes.len() as u64, FileFormat::Jpeg);
        let deleted = rewrite_jpeg_to_vec(
            &bytes,
            info.clone(),
            ParseLimits::default(),
            &[JpegEdit::DeleteComments],
        )
        .unwrap();
        let deleted_metadata = read_jpeg(
            &mut Cursor::new(deleted),
            FileInfo::new("rewrite.jpg".into(), 0, FileFormat::Jpeg),
            ParseLimits::default(),
        )
        .unwrap();
        assert!(deleted_metadata.find("JPEG:Comment").is_none());

        let inserted = rewrite_jpeg_to_vec(
            &[0xFF, 0xD8, 0xFF, 0xD9],
            FileInfo::new("empty.jpg".into(), 4, FileFormat::Jpeg),
            ParseLimits::default(),
            &[JpegEdit::SetComment("inserted".to_owned())],
        )
        .unwrap();
        assert!(inserted.windows(2).any(|window| window == [0xFF, 0xFE]));
    }

    #[test]
    fn rewrites_iptc_and_preserves_other_photoshop_resources() {
        let bytes = jpeg_with_iptc();
        let output = rewrite_jpeg_to_vec(
            &bytes,
            FileInfo::new("iptc.jpg".into(), bytes.len() as u64, FileFormat::Jpeg),
            ParseLimits::default(),
            &[JpegEdit::SetIptc {
                name: "Keywords".to_owned(),
                value: "after".to_owned(),
            }],
        )
        .unwrap();
        assert!(
            output
                .windows(b"preserve me".len())
                .any(|window| window == b"preserve me")
        );
        let metadata = read_jpeg(
            &mut Cursor::new(output),
            FileInfo::new("iptc.jpg".into(), 0, FileFormat::Jpeg),
            ParseLimits::default(),
        )
        .unwrap();
        assert_eq!(
            metadata.find("IPTC:Keywords").unwrap().display_value(),
            "after"
        );
        assert_eq!(
            metadata
                .find("IPTC:CaptionAbstract")
                .unwrap()
                .display_value(),
            "old caption"
        );
    }

    #[test]
    fn deletes_and_inserts_iptc_datasets() {
        let bytes = jpeg_with_iptc();
        let deleted = rewrite_jpeg_to_vec(
            &bytes,
            FileInfo::new("iptc.jpg".into(), bytes.len() as u64, FileFormat::Jpeg),
            ParseLimits::default(),
            &[JpegEdit::DeleteIptc {
                name: "Keywords".to_owned(),
            }],
        )
        .unwrap();
        let deleted_metadata = read_jpeg(
            &mut Cursor::new(deleted),
            FileInfo::new("iptc.jpg".into(), 0, FileFormat::Jpeg),
            ParseLimits::default(),
        )
        .unwrap();
        assert!(deleted_metadata.find("IPTC:Keywords").is_none());
        assert!(deleted_metadata.find("IPTC:CaptionAbstract").is_some());

        let inserted = rewrite_jpeg_to_vec(
            &[0xFF, 0xD8, 0xFF, 0xD9],
            FileInfo::new("new-iptc.jpg".into(), 4, FileFormat::Jpeg),
            ParseLimits::default(),
            &[JpegEdit::SetIptc {
                name: "CaptionAbstract".to_owned(),
                value: "new caption".to_owned(),
            }],
        )
        .unwrap();
        let inserted_metadata = read_jpeg(
            &mut Cursor::new(inserted),
            FileInfo::new("new-iptc.jpg".into(), 0, FileFormat::Jpeg),
            ParseLimits::default(),
        )
        .unwrap();
        assert_eq!(
            inserted_metadata
                .find("IPTC:CaptionAbstract")
                .unwrap()
                .display_value(),
            "new caption"
        );
    }

    #[test]
    fn rejects_unknown_iptc_datasets_before_writing() {
        let error = rewrite_jpeg_to_vec(
            &jpeg_with_iptc(),
            FileInfo::new("iptc.jpg".into(), 0, FileFormat::Jpeg),
            ParseLimits::default(),
            &[JpegEdit::SetIptc {
                name: "Unknown".to_owned(),
                value: "value".to_owned(),
            }],
        )
        .unwrap_err();
        assert!(error.to_string().contains("unsupported IPTC dataset"));
    }

    #[test]
    fn path_rewrite_validates_before_atomic_replace() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("metra-jpeg-rewrite-{unique}"));
        fs::create_dir(&directory).unwrap();
        let path = directory.join("atomic.jpg");
        fs::write(&path, jpeg_with_comment()).unwrap();

        rewrite_jpeg_path(
            &path,
            ParseLimits::default(),
            &[JpegEdit::SetComment("atomic".to_owned())],
        )
        .unwrap();
        let rewritten = fs::read(&path).unwrap();
        let metadata = read_jpeg(
            &mut Cursor::new(rewritten),
            FileInfo::new("atomic.jpg".into(), 0, FileFormat::Jpeg),
            ParseLimits::default(),
        )
        .unwrap();
        assert_eq!(
            metadata.find("JPEG:Comment").unwrap().display_value(),
            "atomic"
        );
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn rejects_oversized_comment_before_writing() {
        let info = FileInfo::new("rewrite.jpg".into(), 4, FileFormat::Jpeg);
        let error = rewrite_jpeg_to_vec(
            &[0xFF, 0xD8, 0xFF, 0xD9],
            info,
            ParseLimits::default(),
            &[JpegEdit::SetComment("x".repeat(65_534))],
        )
        .unwrap_err();
        assert!(error.to_string().contains("65533-byte"));
    }
}
