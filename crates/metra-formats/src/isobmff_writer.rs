use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use metra_core::{FileFormat, FileInfo, Metadata, MetraError, ParseLimits, Result, TagValue};

use crate::atomic::atomic_replace;
use crate::isobmff::read_isobmff;

/// Safe in-place edits for existing ISO-BMFF text items.
///
/// The writer preserves every box and only replaces the value payload of one
/// existing QuickTime-style text item. A replacement may be shorter than the
/// original value and is NUL-padded; it may not require a box resize.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IsobmffEdit {
    SetText {
        key: String,
        value: String,
    },
    DeleteText {
        key: String,
    },
    /// Replace the first embedded XMP packet without changing its box size.
    SetXmp {
        value: String,
    },
    /// Clear the first embedded XMP packet while retaining its box layout.
    DeleteXmp,
}

pub fn rewrite_isobmff<R: Read + Seek, W: Write + Seek>(
    reader: &mut R,
    writer: &mut W,
    file_info: FileInfo,
    limits: ParseLimits,
    edits: &[IsobmffEdit],
) -> Result<()> {
    if !is_isobmff(file_info.format) {
        return Err(MetraError::WriteFailure {
            message: format!("ISO-BMFF writer cannot edit {}", file_info.format),
        });
    }
    let metadata = read_isobmff(reader, file_info.clone(), limits)?;
    let patches = collect_patches(&metadata, file_info.size, limits, edits)?;
    reader
        .seek(SeekFrom::Start(0))
        .map_err(|source| io_error(&file_info.path, source))?;
    rewrite_stream(reader, writer, file_info.size, &file_info.path, &patches)
}

pub fn rewrite_isobmff_to_vec(
    bytes: &[u8],
    file_info: FileInfo,
    limits: ParseLimits,
    edits: &[IsobmffEdit],
) -> Result<Vec<u8>> {
    let mut reader = std::io::Cursor::new(bytes);
    let mut output = std::io::Cursor::new(Vec::new());
    rewrite_isobmff(&mut reader, &mut output, file_info.clone(), limits, edits)?;
    let output = output.into_inner();
    let validation_info = FileInfo::new(file_info.path, output.len() as u64, file_info.format);
    read_isobmff(
        &mut std::io::Cursor::new(output.as_slice()),
        validation_info,
        limits,
    )?;
    Ok(output)
}

pub fn rewrite_isobmff_path(
    path: impl AsRef<Path>,
    limits: ParseLimits,
    edits: &[IsobmffEdit],
) -> Result<()> {
    let path = path.as_ref().to_path_buf();
    let metadata = crate::read_path_with_limits(&path, limits)?;
    let format = metadata.file_info.format;
    if !is_isobmff(format) {
        return Err(MetraError::WriteFailure {
            message: format!("{} is not an ISO-BMFF file", path.display()),
        });
    }
    let source_metadata = fs::metadata(&path).map_err(|source| MetraError::Io {
        path: path.clone(),
        source,
    })?;
    let file_info = FileInfo::new(path.clone(), source_metadata.len(), format);
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
        rewrite_isobmff(&mut input, &mut output, file_info.clone(), limits, edits)?;
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
        let written_info = FileInfo::new(temp_path.clone(), written_size, format);
        read_isobmff(&mut validation, written_info, limits)?;
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

fn is_isobmff(format: FileFormat) -> bool {
    matches!(
        format,
        FileFormat::Heif | FileFormat::Avif | FileFormat::Mp4 | FileFormat::Mov | FileFormat::M4a
    )
}

fn is_writable_text_key(key: &str) -> bool {
    matches!(
        key,
        "ISOBMFF:Title"
            | "ISOBMFF:Artist"
            | "ISOBMFF:Album"
            | "ISOBMFF:Year"
            | "ISOBMFF:Comment"
            | "ISOBMFF:AlbumArtist"
            | "ISOBMFF:Description"
            | "ISOBMFF:PurchaseDate"
            | "ISOBMFF:Encoder"
    )
}

fn collect_patches(
    metadata: &Metadata,
    file_length: u64,
    limits: ParseLimits,
    edits: &[IsobmffEdit],
) -> Result<Vec<Patch>> {
    let mut patches = Vec::with_capacity(edits.len());
    for edit in edits {
        if matches!(edit, IsobmffEdit::SetXmp { .. } | IsobmffEdit::DeleteXmp) {
            patches.push(collect_xmp_patch(metadata, file_length, limits, edit)?);
            continue;
        }
        let (key, value) = match edit {
            IsobmffEdit::SetText { key, value } => (key, value.as_str()),
            IsobmffEdit::DeleteText { key } => (key, ""),
            IsobmffEdit::SetXmp { .. } | IsobmffEdit::DeleteXmp => {
                unreachable!("XMP edits are handled before text edits")
            }
        };
        if !is_writable_text_key(key) {
            return Err(MetraError::WriteFailure {
                message: format!("ISO-BMFF metadata key {key} is not writable"),
            });
        }
        if value.contains('\0') {
            return Err(MetraError::WriteFailure {
                message: format!("ISO-BMFF text value for {key} cannot contain NUL"),
            });
        }
        if value.len() > limits.max_value_bytes {
            return Err(MetraError::ResourceLimitExceeded {
                resource: "ISO-BMFF text value".to_owned(),
                limit: limits.max_value_bytes,
            });
        }
        let matches = metadata.find_all(key);
        let tag = match matches.as_slice() {
            [tag] => *tag,
            [] => {
                return Err(MetraError::WriteFailure {
                    message: format!("ISO-BMFF metadata key {key} does not exist"),
                });
            }
            _ => {
                return Err(MetraError::WriteFailure {
                    message: format!(
                        "ISO-BMFF metadata key {key} is repeated; an unambiguous target is required"
                    ),
                });
            }
        };
        let TagValue::String(_) = &tag.value else {
            return Err(MetraError::WriteFailure {
                message: format!("ISO-BMFF metadata key {key} is not text"),
            });
        };
        let offset = tag.source.offset.ok_or_else(|| MetraError::WriteFailure {
            message: format!("ISO-BMFF metadata key {key} has no source offset"),
        })?;
        let span = tag.source.length.ok_or_else(|| MetraError::WriteFailure {
            message: format!("ISO-BMFF metadata key {key} has no source length"),
        })?;
        if span > u64::try_from(limits.max_value_bytes).unwrap_or(u64::MAX) {
            return Err(MetraError::ResourceLimitExceeded {
                resource: "ISO-BMFF text value".to_owned(),
                limit: limits.max_value_bytes,
            });
        }
        let end = offset.checked_add(span).ok_or(MetraError::InvalidOffset {
            context: format!("ISO-BMFF {key} text value"),
            offset,
        })?;
        if end > file_length {
            return Err(MetraError::UnexpectedEof {
                context: format!("ISO-BMFF {key} text value"),
            });
        }
        let mut bytes = value.as_bytes().to_vec();
        if bytes.len() > usize::try_from(span).unwrap_or(usize::MAX) {
            return Err(MetraError::WriteFailure {
                message: format!(
                    "ISO-BMFF text value for {key} needs {} bytes but the field stores {span} bytes",
                    bytes.len()
                ),
            });
        }
        bytes.resize(
            usize::try_from(span).map_err(|_| MetraError::InvalidOffset {
                context: format!("ISO-BMFF {key} text span"),
                offset: span,
            })?,
            0,
        );
        patches.push(Patch {
            offset,
            span,
            bytes,
        });
    }
    patches.sort_by_key(|patch| patch.offset);
    for pair in patches.windows(2) {
        let previous_end =
            pair[0]
                .offset
                .checked_add(pair[0].span)
                .ok_or(MetraError::InvalidOffset {
                    context: "ISO-BMFF rewrite patch".to_owned(),
                    offset: pair[0].offset,
                })?;
        if previous_end > pair[1].offset {
            return Err(MetraError::WriteFailure {
                message: "ISO-BMFF rewrite patches overlap".to_owned(),
            });
        }
    }
    if patches
        .iter()
        .any(|patch| patch.bytes.len() as u64 != patch.span)
    {
        return Err(MetraError::WriteFailure {
            message: "ISO-BMFF rewrite patch length mismatch".to_owned(),
        });
    }
    Ok(patches)
}

fn collect_xmp_patch(
    metadata: &Metadata,
    file_length: u64,
    limits: ParseLimits,
    edit: &IsobmffEdit,
) -> Result<Patch> {
    let matches = metadata
        .find_all("XMP:Packet")
        .into_iter()
        .filter(|tag| {
            matches!(
                tag.source.container.as_str(),
                "ISO-BMFF/xml" | "ISO-BMFF/uuid-XMP"
            )
        })
        .collect::<Vec<_>>();
    let tag = match matches.as_slice() {
        [tag] => *tag,
        [] => {
            return Err(MetraError::WriteFailure {
                message: "ISO-BMFF XMP packet does not exist".to_owned(),
            });
        }
        _ => {
            return Err(MetraError::WriteFailure {
                message:
                    "ISO-BMFF contains repeated XMP packets; an unambiguous target is required"
                        .to_owned(),
            });
        }
    };
    if !matches!(tag.value, TagValue::Bytes(_)) {
        return Err(MetraError::WriteFailure {
            message: "ISO-BMFF XMP packet is not a byte payload".to_owned(),
        });
    }
    let offset = tag.source.offset.ok_or_else(|| MetraError::WriteFailure {
        message: "ISO-BMFF XMP packet has no source offset".to_owned(),
    })?;
    let span = tag.source.length.ok_or_else(|| MetraError::WriteFailure {
        message: "ISO-BMFF XMP packet has no source length".to_owned(),
    })?;
    if span > u64::try_from(limits.max_value_bytes).unwrap_or(u64::MAX) {
        return Err(MetraError::ResourceLimitExceeded {
            resource: "ISO-BMFF XMP packet".to_owned(),
            limit: limits.max_value_bytes,
        });
    }
    let end = offset.checked_add(span).ok_or(MetraError::InvalidOffset {
        context: "ISO-BMFF XMP packet".to_owned(),
        offset,
    })?;
    if end > file_length {
        return Err(MetraError::UnexpectedEof {
            context: "ISO-BMFF XMP packet".to_owned(),
        });
    }
    let span_usize = usize::try_from(span).map_err(|_| MetraError::ResourceLimitExceeded {
        resource: "ISO-BMFF XMP packet".to_owned(),
        limit: limits.max_value_bytes,
    })?;
    let bytes = match edit {
        IsobmffEdit::SetXmp { value } => {
            if value.contains('\0') {
                return Err(MetraError::WriteFailure {
                    message: "ISO-BMFF XMP value cannot contain NUL".to_owned(),
                });
            }
            if value.len() != span_usize {
                return Err(MetraError::WriteFailure {
                    message: format!(
                        "ISO-BMFF XMP replacement requires exactly {span} bytes, got {}",
                        value.len()
                    ),
                });
            }
            if !crate::xmp::is_xmp_signature(value.as_bytes()) {
                return Err(MetraError::WriteFailure {
                    message: "invalid ISO-BMFF XMP replacement: missing xmpmeta or RDF root"
                        .to_owned(),
                });
            }
            let mut validation = Metadata::new(FileInfo::new(
                "<memory>".into(),
                value.len() as u64,
                FileFormat::Xmp,
            ));
            crate::xmp::parse_xmp(
                value.as_bytes(),
                0,
                "ISO-BMFF/XMP-rewrite",
                &mut validation,
                limits,
            )
            .map_err(|error| MetraError::WriteFailure {
                message: format!("invalid ISO-BMFF XMP replacement: {error}"),
            })?;
            value.as_bytes().to_vec()
        }
        IsobmffEdit::DeleteXmp => vec![0; span_usize],
        IsobmffEdit::SetText { .. } | IsobmffEdit::DeleteText { .. } => {
            unreachable!("text edits are handled by the text collector")
        }
    };
    Ok(Patch {
        offset,
        span,
        bytes,
    })
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
                message: "ISO-BMFF rewrite patches are not ordered".to_owned(),
            });
        }
        copy_exact(reader, writer, patch.offset - cursor, path)?;
        reader
            .seek(SeekFrom::Current(i64::try_from(patch.span).map_err(
                |_| MetraError::InvalidOffset {
                    context: "ISO-BMFF rewrite patch span".to_owned(),
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
                context: "ISO-BMFF rewrite cursor".to_owned(),
                offset: patch.offset,
            })?;
    }
    copy_exact(reader, writer, file_length.saturating_sub(cursor), path)
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
        .unwrap_or("metadata.mp4");
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

    fn box_with_kind(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let size = u32::try_from(data.len() + 8).expect("test box fits");
        let mut bytes = size.to_be_bytes().to_vec();
        bytes.extend_from_slice(kind);
        bytes.extend_from_slice(data);
        bytes
    }

    fn quicktime_fixture() -> Vec<u8> {
        let ftyp = box_with_kind(b"ftyp", b"isom\0\0\0\0mp42");
        let data = box_with_kind(
            b"data",
            &[0, 0, 0, 1, 0, 0, 0, 0, b'M', b'e', b't', b'r', b'a', 0, 0],
        );
        let title = box_with_kind(b"\xA9nam", &data);
        let ilst = box_with_kind(b"ilst", &title);
        let udta = box_with_kind(b"udta", &ilst);
        let moov = box_with_kind(b"moov", &udta);
        [ftyp, moov].concat()
    }

    fn xmp_packet(format: &str) -> Vec<u8> {
        format!(
            "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF><rdf:Description xmlns:dc=\"urn:dc\" dc:format=\"{format}\"/></rdf:RDF></x:xmpmeta>"
        )
        .into_bytes()
    }

    fn direct_xmp_fixture(format: &str) -> Vec<u8> {
        let ftyp = box_with_kind(b"ftyp", b"isom\0\0\0\0");
        let xmp = box_with_kind(b"xml ", &xmp_packet(format));
        [ftyp, xmp].concat()
    }

    fn uuid_xmp_fixture(format: &str) -> Vec<u8> {
        const ADOBE_XMP_UUID: [u8; 16] = [
            0xBE, 0x7A, 0xCF, 0xCB, 0x97, 0xA9, 0x42, 0xE8, 0x9C, 0x71, 0x99, 0x94, 0x91, 0xE3,
            0xAF, 0xAC,
        ];
        let ftyp = box_with_kind(b"ftyp", b"isom\0\0\0\0");
        let mut payload = ADOBE_XMP_UUID.to_vec();
        payload.extend_from_slice(&xmp_packet(format));
        let uuid = box_with_kind(b"uuid", &payload);
        [ftyp, uuid].concat()
    }

    #[test]
    fn replaces_existing_text_without_changing_box_sizes() {
        let bytes = quicktime_fixture();
        let info = FileInfo::new("movie.mp4".into(), bytes.len() as u64, FileFormat::Mp4);
        let output = rewrite_isobmff_to_vec(
            &bytes,
            info.clone(),
            ParseLimits::default(),
            &[IsobmffEdit::SetText {
                key: "ISOBMFF:Title".to_owned(),
                value: "Hi".to_owned(),
            }],
        )
        .expect("ISO-BMFF text edit should succeed");
        assert_eq!(output.len(), bytes.len());
        let metadata = read_isobmff(&mut Cursor::new(output), info, ParseLimits::default())
            .expect("edited ISO-BMFF should remain readable");
        assert_eq!(
            metadata.find("ISOBMFF:Title").unwrap().display_value(),
            "Hi"
        );
    }

    #[test]
    fn refuses_text_that_needs_a_larger_box() {
        let bytes = quicktime_fixture();
        let result = rewrite_isobmff_to_vec(
            &bytes,
            FileInfo::new("movie.mp4".into(), bytes.len() as u64, FileFormat::Mp4),
            ParseLimits::default(),
            &[IsobmffEdit::SetText {
                key: "ISOBMFF:Title".to_owned(),
                value: "a much longer title".to_owned(),
            }],
        );
        assert!(matches!(result, Err(MetraError::WriteFailure { .. })));
    }

    #[test]
    fn deletes_existing_text_by_zeroing_only_its_value_span() {
        let bytes = quicktime_fixture();
        let info = FileInfo::new("movie.mp4".into(), bytes.len() as u64, FileFormat::Mp4);
        let output = rewrite_isobmff_to_vec(
            &bytes,
            info.clone(),
            ParseLimits::default(),
            &[IsobmffEdit::DeleteText {
                key: "ISOBMFF:Title".to_owned(),
            }],
        )
        .expect("ISO-BMFF text deletion should succeed");
        assert_eq!(output.len(), bytes.len());
        let metadata = read_isobmff(&mut Cursor::new(output), info, ParseLimits::default())
            .expect("edited ISO-BMFF should remain readable");
        assert!(metadata.find("ISOBMFF:Title").is_none());
    }

    #[test]
    fn replaces_embedded_xmp_without_changing_direct_box_sizes() {
        let bytes = direct_xmp_fixture("old");
        let replacement = String::from_utf8(xmp_packet("new")).expect("XMP is UTF-8");
        let info = FileInfo::new("image.heic".into(), bytes.len() as u64, FileFormat::Heif);
        let output = rewrite_isobmff_to_vec(
            &bytes,
            info.clone(),
            ParseLimits::default(),
            &[IsobmffEdit::SetXmp { value: replacement }],
        )
        .expect("embedded XMP edit should succeed");
        assert_eq!(output.len(), bytes.len());
        assert_eq!(&output[..12], &bytes[..12]);
        let metadata = read_isobmff(&mut Cursor::new(output), info, ParseLimits::default())
            .expect("edited XMP container should remain readable");
        assert_eq!(
            metadata.find("XMP:dc:format").unwrap().display_value(),
            "new"
        );
    }

    #[test]
    fn replaces_embedded_xmp_inside_adobe_uuid_boxes() {
        let bytes = uuid_xmp_fixture("old");
        let replacement = String::from_utf8(xmp_packet("new")).expect("XMP is UTF-8");
        let info = FileInfo::new("image.avif".into(), bytes.len() as u64, FileFormat::Avif);
        let output = rewrite_isobmff_to_vec(
            &bytes,
            info.clone(),
            ParseLimits::default(),
            &[IsobmffEdit::SetXmp { value: replacement }],
        )
        .expect("UUID XMP edit should succeed");
        assert_eq!(output.len(), bytes.len());
        let metadata = read_isobmff(&mut Cursor::new(output), info, ParseLimits::default())
            .expect("edited UUID XMP container should remain readable");
        assert_eq!(
            metadata.find("ISOBMFF:UUIDKind").unwrap().display_value(),
            "XMP"
        );
        assert_eq!(
            metadata.find("XMP:dc:format").unwrap().display_value(),
            "new"
        );
    }

    #[test]
    fn deletes_embedded_xmp_by_zeroing_only_its_payload() {
        let bytes = direct_xmp_fixture("old");
        let info = FileInfo::new("image.heic".into(), bytes.len() as u64, FileFormat::Heif);
        let output = rewrite_isobmff_to_vec(
            &bytes,
            info.clone(),
            ParseLimits::default(),
            &[IsobmffEdit::DeleteXmp],
        )
        .expect("embedded XMP deletion should succeed");
        assert_eq!(output.len(), bytes.len());
        assert_eq!(&output[..12], &bytes[..12]);
        let metadata = read_isobmff(&mut Cursor::new(output), info, ParseLimits::default())
            .expect("deleted XMP container should remain readable");
        let packet = metadata
            .find("XMP:Packet")
            .expect("cleared packet is retained");
        assert!(
            matches!(&packet.value, TagValue::Bytes(bytes) if bytes.iter().all(|byte| *byte == 0))
        );
        assert!(metadata.find("XMP:dc:format").is_none());
    }

    #[test]
    fn rejects_invalid_or_resized_embedded_xmp_replacements() {
        let bytes = direct_xmp_fixture("old");
        let info = FileInfo::new("image.heic".into(), bytes.len() as u64, FileFormat::Heif);
        let packet_length = xmp_packet("old").len();
        let invalid = String::from_utf8(vec![b'x'; packet_length]).expect("ASCII is UTF-8");
        let invalid_result = rewrite_isobmff_to_vec(
            &bytes,
            info.clone(),
            ParseLimits::default(),
            &[IsobmffEdit::SetXmp { value: invalid }],
        );
        assert!(matches!(
            invalid_result,
            Err(MetraError::WriteFailure { .. })
        ));

        let resized = String::from_utf8(xmp_packet("old")).expect("XMP is UTF-8") + "x";
        let resized_result = rewrite_isobmff_to_vec(
            &bytes,
            info,
            ParseLimits::default(),
            &[IsobmffEdit::SetXmp { value: resized }],
        );
        assert!(matches!(
            resized_result,
            Err(MetraError::WriteFailure { .. })
        ));
    }
}
