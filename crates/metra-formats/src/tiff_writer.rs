use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use metra_core::{FileFormat, FileInfo, Metadata, MetraError, ParseLimits, Result, TagValue};

use crate::atomic::atomic_replace;
use crate::tiff::read_tiff;

/// Safe in-place edits for existing TIFF/BigTIFF ASCII values.
///
/// The writer never changes an IFD layout or allocates a new value area. An
/// edit succeeds only when the replacement, including its terminating NUL,
/// fits in the original ASCII field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TiffEdit {
    SetAscii { key: String, value: String },
    DeleteAscii { key: String },
    SetGpsDecimal { key: String, value: String },
    DeleteGpsDecimal { key: String },
}

pub fn rewrite_tiff<R: Read + Seek, W: Write + Seek>(
    reader: &mut R,
    writer: &mut W,
    file_info: FileInfo,
    limits: ParseLimits,
    edits: &[TiffEdit],
) -> Result<()> {
    let metadata = read_tiff(reader, file_info.clone(), limits)?;
    let patches = collect_patches(reader, &metadata, &file_info, limits, edits)?;
    reader
        .seek(SeekFrom::Start(0))
        .map_err(|source| io_error(&file_info.path, source))?;
    rewrite_stream(reader, writer, file_info.size, &file_info.path, &patches)
}

pub fn rewrite_tiff_to_vec(
    bytes: &[u8],
    file_info: FileInfo,
    limits: ParseLimits,
    edits: &[TiffEdit],
) -> Result<Vec<u8>> {
    let mut reader = std::io::Cursor::new(bytes);
    let mut output = std::io::Cursor::new(Vec::new());
    rewrite_tiff(&mut reader, &mut output, file_info.clone(), limits, edits)?;
    let output = output.into_inner();
    let validation_info = FileInfo::new(file_info.path, output.len() as u64, file_info.format);
    read_tiff(
        &mut std::io::Cursor::new(output.as_slice()),
        validation_info,
        limits,
    )?;
    Ok(output)
}

pub fn rewrite_tiff_path(
    path: impl AsRef<Path>,
    limits: ParseLimits,
    edits: &[TiffEdit],
) -> Result<()> {
    let path = path.as_ref().to_path_buf();
    let source_metadata = fs::metadata(&path).map_err(|source| MetraError::Io {
        path: path.clone(),
        source,
    })?;
    let file_info = FileInfo::new(path.clone(), source_metadata.len(), FileFormat::Tiff);
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
        rewrite_tiff(&mut input, &mut output, file_info.clone(), limits, edits)?;
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
        let written_info = FileInfo::new(temp_path.clone(), written_size, FileFormat::Tiff);
        read_tiff(&mut validation, written_info, limits)?;
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

#[derive(Debug, Clone, Copy)]
enum Endian {
    Little,
    Big,
}

#[derive(Debug, Clone, Copy)]
enum Variant {
    Classic { endian: Endian },
    Big { endian: Endian },
}

impl Variant {
    const fn endian(self) -> Endian {
        match self {
            Self::Classic { endian } | Self::Big { endian } => endian,
        }
    }

    const fn inline_size(self) -> usize {
        match self {
            Self::Classic { .. } => 4,
            Self::Big { .. } => 8,
        }
    }

    const fn entry_size(self) -> usize {
        match self {
            Self::Classic { .. } => 12,
            Self::Big { .. } => 20,
        }
    }
}

fn collect_patches<R: Read + Seek>(
    reader: &mut R,
    metadata: &Metadata,
    file_info: &FileInfo,
    limits: ParseLimits,
    edits: &[TiffEdit],
) -> Result<Vec<Patch>> {
    let variant = read_variant(reader, file_info.size, &file_info.path)?;
    let mut patches = Vec::with_capacity(edits.len());
    for edit in edits {
        if matches!(
            edit,
            TiffEdit::SetGpsDecimal { .. } | TiffEdit::DeleteGpsDecimal { .. }
        ) {
            patches.extend(collect_gps_patches(
                reader, metadata, file_info, limits, variant, edit,
            )?);
            continue;
        }
        let (key, value) = match edit {
            TiffEdit::SetAscii { key, value } => (key, value.as_str()),
            TiffEdit::DeleteAscii { key } => (key, ""),
            TiffEdit::SetGpsDecimal { .. } | TiffEdit::DeleteGpsDecimal { .. } => {
                unreachable!("GPS edits are handled before ASCII edits")
            }
        };
        if value.contains('\0') {
            return Err(MetraError::WriteFailure {
                message: format!("TIFF ASCII value for {key} cannot contain NUL"),
            });
        }
        if value.len() >= limits.max_value_bytes {
            return Err(MetraError::ResourceLimitExceeded {
                resource: "TIFF ASCII value".to_owned(),
                limit: limits.max_value_bytes,
            });
        }
        let matches = metadata.find_all(key);
        let tag = match matches.as_slice() {
            [tag] => *tag,
            [] => {
                return Err(MetraError::WriteFailure {
                    message: format!("TIFF tag {key} does not exist; insertion is not supported"),
                });
            }
            _ => {
                return Err(MetraError::WriteFailure {
                    message: format!(
                        "TIFF tag {key} is repeated; an unambiguous target is required"
                    ),
                });
            }
        };
        let entry_offset = tag.source.offset.ok_or_else(|| MetraError::WriteFailure {
            message: format!("TIFF tag {key} has no source entry offset"),
        })?;
        let entry_size = variant.entry_size();
        let entry = read_at(
            reader,
            entry_offset,
            entry_size,
            file_info.size,
            &file_info.path,
            "TIFF IFD entry",
        )?;
        let endian = variant.endian();
        let type_id = read_u16(endian, &entry[2..4]);
        if type_id != 2
            || !matches!(
                tag.value,
                TagValue::String(_)
                    | TagValue::Date { .. }
                    | TagValue::Time { .. }
                    | TagValue::DateTime { .. }
            )
        {
            return Err(MetraError::WriteFailure {
                message: format!("TIFF tag {key} is not an existing ASCII value"),
            });
        }
        let count = match variant {
            Variant::Classic { .. } => u64::from(read_u32(endian, &entry[4..8])),
            Variant::Big { .. } => read_u64(endian, &entry[4..12]),
        };
        let count_usize =
            usize::try_from(count).map_err(|_| MetraError::ResourceLimitExceeded {
                resource: "TIFF ASCII value".to_owned(),
                limit: limits.max_value_bytes,
            })?;
        if count == 0 || count_usize > limits.max_value_bytes {
            return Err(MetraError::ResourceLimitExceeded {
                resource: "TIFF ASCII value".to_owned(),
                limit: limits.max_value_bytes,
            });
        }
        let mut replacement = value.as_bytes().to_vec();
        replacement.push(0);
        if replacement.len() > count_usize {
            return Err(MetraError::WriteFailure {
                message: format!(
                    "TIFF ASCII value for {key} needs {} bytes but the field stores {count} bytes",
                    replacement.len()
                ),
            });
        }
        replacement.resize(count_usize, 0);
        let value_field_start = match variant {
            Variant::Classic { .. } => 8,
            Variant::Big { .. } => 12,
        };
        let value_offset = if count_usize <= variant.inline_size() {
            entry_offset
                .checked_add(value_field_start as u64)
                .ok_or(MetraError::InvalidOffset {
                    context: "TIFF inline ASCII value".to_owned(),
                    offset: entry_offset,
                })?
        } else {
            match variant {
                Variant::Classic { .. } => read_u32(
                    endian,
                    &entry[value_field_start..value_field_start + variant.inline_size()],
                ) as u64,
                Variant::Big { .. } => read_u64(
                    endian,
                    &entry[value_field_start..value_field_start + variant.inline_size()],
                ),
            }
        };
        let value_end = value_offset
            .checked_add(count)
            .ok_or(MetraError::InvalidOffset {
                context: format!("TIFF {key} ASCII value"),
                offset: value_offset,
            })?;
        if value_end > file_info.size {
            return Err(MetraError::UnexpectedEof {
                context: format!("TIFF {key} ASCII value"),
            });
        }
        patches.push(Patch {
            offset: value_offset,
            span: count,
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
                    context: "TIFF rewrite patch".to_owned(),
                    offset: pair[0].offset,
                })?;
        if previous_end > pair[1].offset {
            return Err(MetraError::WriteFailure {
                message: "TIFF rewrite patches overlap".to_owned(),
            });
        }
    }
    Ok(patches)
}

#[derive(Debug, Clone, Copy)]
struct GpsCoordinateSpec {
    value_key: &'static str,
    reference_key: &'static str,
    maximum: f64,
    positive_reference: u8,
    negative_reference: u8,
}

fn gps_coordinate_spec(key: &str) -> Option<GpsCoordinateSpec> {
    match key {
        "GPS:GPSLatitude" => Some(GpsCoordinateSpec {
            value_key: "GPS:GPSLatitude",
            reference_key: "GPS:GPSLatitudeRef",
            maximum: 90.0,
            positive_reference: b'N',
            negative_reference: b'S',
        }),
        "GPS:GPSLongitude" => Some(GpsCoordinateSpec {
            value_key: "GPS:GPSLongitude",
            reference_key: "GPS:GPSLongitudeRef",
            maximum: 180.0,
            positive_reference: b'E',
            negative_reference: b'W',
        }),
        _ => None,
    }
}

fn collect_gps_patches<R: Read + Seek>(
    reader: &mut R,
    metadata: &Metadata,
    file_info: &FileInfo,
    limits: ParseLimits,
    variant: Variant,
    edit: &TiffEdit,
) -> Result<Vec<Patch>> {
    let (key, replacement) = match edit {
        TiffEdit::SetGpsDecimal { key, value } => (key.as_str(), Some(value.as_str())),
        TiffEdit::DeleteGpsDecimal { key } => (key.as_str(), None),
        TiffEdit::SetAscii { .. } | TiffEdit::DeleteAscii { .. } => {
            unreachable!("ASCII edits are handled by the ASCII collector")
        }
    };
    let spec = gps_coordinate_spec(key).ok_or_else(|| MetraError::WriteFailure {
        message: format!("TIFF GPS coordinate key {key} is not writable"),
    })?;
    if limits.max_value_bytes < 24 {
        return Err(MetraError::ResourceLimitExceeded {
            resource: "TIFF GPS coordinate".to_owned(),
            limit: limits.max_value_bytes,
        });
    }

    let coordinate_tag = match metadata.find_all(spec.value_key).as_slice() {
        [tag] => *tag,
        [] => {
            return Err(MetraError::WriteFailure {
                message: format!("TIFF GPS coordinate {} does not exist", spec.value_key),
            });
        }
        _ => {
            return Err(MetraError::WriteFailure {
                message: format!("TIFF GPS coordinate {} is repeated", spec.value_key),
            });
        }
    };
    if !matches!(
        &coordinate_tag.value,
        TagValue::Array(values)
            if values.len() == 3
                && values.iter().all(|value| matches!(
                    value,
                    TagValue::UnsignedRational { .. }
                ))
    ) {
        return Err(MetraError::WriteFailure {
            message: format!(
                "TIFF GPS coordinate {} is not an unsigned-rational triplet",
                spec.value_key
            ),
        });
    }
    let coordinate_entry_offset =
        coordinate_tag
            .source
            .offset
            .ok_or_else(|| MetraError::WriteFailure {
                message: format!("TIFF GPS coordinate {} has no source entry", spec.value_key),
            })?;
    let coordinate_entry = read_at(
        reader,
        coordinate_entry_offset,
        variant.entry_size(),
        file_info.size,
        &file_info.path,
        "TIFF GPS coordinate entry",
    )?;
    let endian = variant.endian();
    let coordinate_type = read_u16(endian, &coordinate_entry[2..4]);
    let coordinate_count = match variant {
        Variant::Classic { .. } => u64::from(read_u32(endian, &coordinate_entry[4..8])),
        Variant::Big { .. } => read_u64(endian, &coordinate_entry[4..12]),
    };
    if coordinate_type != 5 || coordinate_count != 3 {
        return Err(MetraError::WriteFailure {
            message: format!(
                "TIFF GPS coordinate {} must use exactly three unsigned rationals",
                spec.value_key,
            ),
        });
    }
    let coordinate_offset =
        entry_value_offset(variant, coordinate_entry_offset, &coordinate_entry, 24)?;
    let coordinate_end = coordinate_offset
        .checked_add(24)
        .ok_or(MetraError::InvalidOffset {
            context: "TIFF GPS coordinate value".to_owned(),
            offset: coordinate_offset,
        })?;
    if coordinate_end > file_info.size {
        return Err(MetraError::UnexpectedEof {
            context: "TIFF GPS coordinate value".to_owned(),
        });
    }

    let reference_tag = match metadata.find_all(spec.reference_key).as_slice() {
        [tag] => *tag,
        [] => {
            return Err(MetraError::WriteFailure {
                message: format!("TIFF GPS reference {} does not exist", spec.reference_key),
            });
        }
        _ => {
            return Err(MetraError::WriteFailure {
                message: format!("TIFF GPS reference {} is repeated", spec.reference_key),
            });
        }
    };
    if !matches!(reference_tag.value, TagValue::String(_)) {
        return Err(MetraError::WriteFailure {
            message: format!(
                "TIFF GPS reference {} is not ASCII text",
                spec.reference_key
            ),
        });
    }
    let reference_entry_offset =
        reference_tag
            .source
            .offset
            .ok_or_else(|| MetraError::WriteFailure {
                message: format!(
                    "TIFF GPS reference {} has no source entry",
                    spec.reference_key
                ),
            })?;
    let reference_entry = read_at(
        reader,
        reference_entry_offset,
        variant.entry_size(),
        file_info.size,
        &file_info.path,
        "TIFF GPS reference entry",
    )?;
    let reference_type = read_u16(endian, &reference_entry[2..4]);
    let reference_count = match variant {
        Variant::Classic { .. } => u64::from(read_u32(endian, &reference_entry[4..8])),
        Variant::Big { .. } => read_u64(endian, &reference_entry[4..12]),
    };
    let reference_count_usize =
        usize::try_from(reference_count).map_err(|_| MetraError::ResourceLimitExceeded {
            resource: "TIFF GPS reference".to_owned(),
            limit: limits.max_value_bytes,
        })?;
    if reference_type != 2
        || reference_count_usize < 2
        || reference_count_usize > limits.max_value_bytes
    {
        return Err(MetraError::WriteFailure {
            message: format!(
                "TIFF GPS reference {} must be an existing ASCII field with a NUL slot",
                spec.reference_key
            ),
        });
    }
    let reference_offset = entry_value_offset(
        variant,
        reference_entry_offset,
        &reference_entry,
        reference_count_usize,
    )?;
    let reference_end =
        reference_offset
            .checked_add(reference_count)
            .ok_or(MetraError::InvalidOffset {
                context: "TIFF GPS reference value".to_owned(),
                offset: reference_offset,
            })?;
    if reference_end > file_info.size {
        return Err(MetraError::UnexpectedEof {
            context: "TIFF GPS reference value".to_owned(),
        });
    }

    let mut coordinate_bytes = vec![0_u8; 24];
    let mut reference_bytes = vec![0_u8; reference_count_usize];
    if let Some(replacement) = replacement {
        let decimal = replacement
            .parse::<f64>()
            .map_err(|_| MetraError::WriteFailure {
                message: format!("TIFF GPS coordinate value {replacement:?} is not a number"),
            })?;
        let negative = decimal.is_sign_negative() && decimal != 0.0;
        let rationals = encode_gps_coordinate(decimal, spec.maximum, spec.value_key)?;
        for (index, (numerator, denominator)) in rationals.into_iter().enumerate() {
            let start = index * 8;
            write_u32(endian, &mut coordinate_bytes[start..start + 4], numerator);
            write_u32(
                endian,
                &mut coordinate_bytes[start + 4..start + 8],
                denominator,
            );
        }
        reference_bytes[0] = if negative {
            spec.negative_reference
        } else {
            spec.positive_reference
        };
    }

    Ok(vec![
        Patch {
            offset: coordinate_offset,
            span: 24,
            bytes: coordinate_bytes,
        },
        Patch {
            offset: reference_offset,
            span: reference_count,
            bytes: reference_bytes,
        },
    ])
}

fn encode_gps_coordinate(value: f64, maximum: f64, key: &str) -> Result<[(u32, u32); 3]> {
    if !value.is_finite() || value.abs() > maximum {
        return Err(MetraError::WriteFailure {
            message: format!("TIFF GPS coordinate {key} must be finite and within ±{maximum}"),
        });
    }
    const SCALE: f64 = 1_000_000.0;
    let magnitude = value.abs();
    let mut degrees = magnitude.floor() as u64;
    let minutes_total = (magnitude - degrees as f64) * 60.0;
    let mut minutes = minutes_total.floor() as u64;
    let mut seconds_scaled = ((minutes_total - minutes as f64) * 60.0 * SCALE).round() as u64;
    if seconds_scaled >= 60 * SCALE as u64 {
        seconds_scaled = 0;
        minutes += 1;
    }
    if minutes >= 60 {
        minutes = 0;
        degrees += 1;
    }
    if degrees > maximum as u64 {
        return Err(MetraError::WriteFailure {
            message: format!("TIFF GPS coordinate {key} rounded outside ±{maximum}"),
        });
    }
    Ok([
        (u32::try_from(degrees).expect("GPS degrees fit u32"), 1),
        (u32::try_from(minutes).expect("GPS minutes fit u32"), 1),
        (
            u32::try_from(seconds_scaled).expect("GPS seconds scale fits u32"),
            1_000_000,
        ),
    ])
}

fn read_variant<R: Read + Seek>(reader: &mut R, file_length: u64, path: &Path) -> Result<Variant> {
    let header = read_at(reader, 0, 8, file_length, path, "TIFF header")?;
    let endian = match &header[..2] {
        b"II" => Endian::Little,
        b"MM" => Endian::Big,
        _ => {
            return Err(MetraError::InvalidHeader {
                context: "TIFF".to_owned(),
                message: "byte order must be II or MM".to_owned(),
            });
        }
    };
    match read_u16(endian, &header[2..4]) {
        42 => Ok(Variant::Classic { endian }),
        43 => {
            let extended = read_at(reader, 4, 12, file_length, path, "BigTIFF header")?;
            if read_u16(endian, &extended[..2]) != 8 || read_u16(endian, &extended[2..4]) != 0 {
                return Err(MetraError::InvalidHeader {
                    context: "BigTIFF".to_owned(),
                    message: "invalid offset-size or reserved field".to_owned(),
                });
            }
            Ok(Variant::Big { endian })
        }
        magic => Err(MetraError::InvalidHeader {
            context: "TIFF".to_owned(),
            message: format!("expected magic 42 or 43, got {magic}"),
        }),
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
                message: "TIFF rewrite patches are not ordered".to_owned(),
            });
        }
        copy_exact(reader, writer, patch.offset - cursor, path)?;
        reader
            .seek(SeekFrom::Current(i64::try_from(patch.span).map_err(
                |_| MetraError::InvalidOffset {
                    context: "TIFF rewrite patch span".to_owned(),
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
                context: "TIFF rewrite cursor".to_owned(),
                offset: patch.offset,
            })?;
    }
    copy_exact(reader, writer, file_length.saturating_sub(cursor), path)
}

fn entry_value_offset(
    variant: Variant,
    entry_offset: u64,
    entry: &[u8],
    value_size: usize,
) -> Result<u64> {
    if value_size <= variant.inline_size() {
        entry_offset
            .checked_add(match variant {
                Variant::Classic { .. } => 8,
                Variant::Big { .. } => 12,
            })
            .ok_or(MetraError::InvalidOffset {
                context: "TIFF inline value".to_owned(),
                offset: entry_offset,
            })
    } else {
        match variant {
            Variant::Classic { endian } => Ok(u64::from(read_u32(endian, &entry[8..12]))),
            Variant::Big { endian } => Ok(read_u64(endian, &entry[12..20])),
        }
    }
}

fn read_u16(endian: Endian, bytes: &[u8]) -> u16 {
    let bytes: [u8; 2] = bytes.try_into().expect("caller validates u16 length");
    match endian {
        Endian::Little => u16::from_le_bytes(bytes),
        Endian::Big => u16::from_be_bytes(bytes),
    }
}

fn read_u32(endian: Endian, bytes: &[u8]) -> u32 {
    let bytes: [u8; 4] = bytes.try_into().expect("caller validates u32 length");
    match endian {
        Endian::Little => u32::from_le_bytes(bytes),
        Endian::Big => u32::from_be_bytes(bytes),
    }
}

fn read_u64(endian: Endian, bytes: &[u8]) -> u64 {
    let bytes: [u8; 8] = bytes.try_into().expect("caller validates u64 length");
    match endian {
        Endian::Little => u64::from_le_bytes(bytes),
        Endian::Big => u64::from_be_bytes(bytes),
    }
}

fn write_u32(endian: Endian, bytes: &mut [u8], value: u32) {
    let encoded = match endian {
        Endian::Little => value.to_le_bytes(),
        Endian::Big => value.to_be_bytes(),
    };
    bytes.copy_from_slice(&encoded);
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
        .unwrap_or("metadata.tif");
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
    use metra_core::FileFormat;

    fn tiff_with_make(value: &[u8]) -> Vec<u8> {
        let mut bytes = vec![
            b'I',
            b'I',
            42,
            0,
            8,
            0,
            0,
            0,
            1,
            0, // one IFD0 entry
            0x0F,
            0x01,
            2,
            0,
            value.len() as u8,
            0,
            0,
            0,
            26,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
        ];
        assert_eq!(bytes.len(), 26);
        bytes.extend_from_slice(value);
        bytes
    }

    fn tiff_with_datetime() -> Vec<u8> {
        let mut bytes = vec![
            b'I', b'I', 42, 0, 8, 0, 0, 0, 1, 0, // one IFD0 entry
            0x69, 0x87, 4, 0, 1, 0, 0, 0, 26, 0, 0, 0, // ExifIFD -> offset 26
            0, 0, 0, 0, // no next IFD
            1, 0, // one Exif IFD entry
            0x03, 0x90, 2, 0, 20, 0, 0, 0, 44, 0, 0, 0, // DateTimeOriginal
            0, 0, 0, 0, // no next IFD
        ];
        bytes.extend_from_slice(b"2026:09:13 12:34:56\0");
        assert_eq!(bytes.len(), 64);
        bytes
    }

    fn tiff_with_gps() -> Vec<u8> {
        let mut bytes = vec![
            b'I', b'I', 42, 0, 8, 0, 0, 0, 1, 0, // one IFD0 entry
            0x25, 0x88, 4, 0, 1, 0, 0, 0, 26, 0, 0, 0, // GPS IFD -> offset 26
            0, 0, 0, 0, // no next IFD
            2, 0, // two GPS IFD entries
            2, 0, 5, 0, 3, 0, 0, 0, 56, 0, 0, 0, // GPSLatitude -> offset 56
            1, 0, 2, 0, 2, 0, 0, 0, b'N', 0, 0, 0, // GPSLatitudeRef = N
            0, 0, 0, 0, // no next IFD
        ];
        bytes.extend_from_slice(&48_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u32.to_le_bytes());
        bytes.extend_from_slice(&51_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u32.to_le_bytes());
        bytes.extend_from_slice(&24_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u32.to_le_bytes());
        assert_eq!(bytes.len(), 80);
        bytes
    }

    fn info(bytes: &[u8]) -> FileInfo {
        FileInfo::new("editable.tif".into(), bytes.len() as u64, FileFormat::Tiff)
    }

    #[test]
    fn replaces_existing_ascii_without_changing_file_size() {
        let bytes = tiff_with_make(b"Canon\0");
        let output = rewrite_tiff_to_vec(
            &bytes,
            info(&bytes),
            ParseLimits::default(),
            &[TiffEdit::SetAscii {
                key: "EXIF:Make".to_owned(),
                value: "Sony".to_owned(),
            }],
        )
        .expect("in-place TIFF edit should succeed");
        assert_eq!(output.len(), bytes.len());
        let metadata = read_tiff(
            &mut Cursor::new(output),
            FileInfo::new("edited.tif".into(), bytes.len() as u64, FileFormat::Tiff),
            ParseLimits::default(),
        )
        .expect("edited TIFF should remain readable");
        assert_eq!(metadata.find("EXIF:Make").unwrap().display_value(), "Sony");
    }

    #[test]
    fn refuses_values_that_need_new_storage() {
        let bytes = tiff_with_make(b"A\0");
        let result = rewrite_tiff_to_vec(
            &bytes,
            info(&bytes),
            ParseLimits::default(),
            &[TiffEdit::SetAscii {
                key: "EXIF:Make".to_owned(),
                value: "Canon".to_owned(),
            }],
        );
        assert!(matches!(result, Err(MetraError::WriteFailure { .. })));
    }

    #[test]
    fn deletes_existing_ascii_without_changing_tiff_layout() {
        let bytes = tiff_with_make(b"Canon\0");
        let output = rewrite_tiff_to_vec(
            &bytes,
            info(&bytes),
            ParseLimits::default(),
            &[TiffEdit::DeleteAscii {
                key: "EXIF:Make".to_owned(),
            }],
        )
        .expect("TIFF ASCII deletion should succeed");
        assert_eq!(output.len(), bytes.len());
        let metadata = read_tiff(
            &mut Cursor::new(output),
            info(&bytes),
            ParseLimits::default(),
        )
        .expect("deleted TIFF should remain readable");
        assert!(metadata.find("EXIF:Make").is_none());
    }

    #[test]
    fn edits_bigtiff_inline_ascii_values() {
        let mut bytes = vec![b'I', b'I', 43, 0, 8, 0, 0, 0, 16, 0, 0, 0, 0, 0, 0, 0];
        bytes.extend_from_slice(&1_u64.to_le_bytes());
        bytes.extend_from_slice(&0x010F_u16.to_le_bytes());
        bytes.extend_from_slice(&2_u16.to_le_bytes());
        bytes.extend_from_slice(&5_u64.to_le_bytes());
        bytes.extend_from_slice(b"Canon\0\0\0");
        bytes.extend_from_slice(&0_u64.to_le_bytes());
        let output = rewrite_tiff_to_vec(
            &bytes,
            info(&bytes),
            ParseLimits::default(),
            &[TiffEdit::SetAscii {
                key: "EXIF:Make".to_owned(),
                value: "Sony".to_owned(),
            }],
        )
        .expect("BigTIFF in-place edit should succeed");
        let metadata = read_tiff(
            &mut Cursor::new(output),
            info(&bytes),
            ParseLimits::default(),
        )
        .expect("edited BigTIFF should remain readable");
        assert_eq!(metadata.find("EXIF:Make").unwrap().display_value(), "Sony");
    }

    #[test]
    fn rewrites_typed_datetime_values_backed_by_ascii_slots() {
        let bytes = tiff_with_datetime();
        let output = rewrite_tiff_to_vec(
            &bytes,
            info(&bytes),
            ParseLimits::default(),
            &[TiffEdit::SetAscii {
                key: "EXIF:DateTimeOriginal".to_owned(),
                value: "2027:10:14 13:35:57".to_owned(),
            }],
        )
        .expect("typed DateTimeOriginal should remain writable as ASCII");
        assert_eq!(output.len(), bytes.len());
        let metadata = read_tiff(
            &mut Cursor::new(output),
            info(&bytes),
            ParseLimits::default(),
        )
        .expect("edited DateTimeOriginal should remain readable");
        assert_eq!(
            metadata
                .find("EXIF:DateTimeOriginal")
                .unwrap()
                .display_value(),
            "2027:10:14 13:35:57"
        );
    }

    #[test]
    fn rewrites_gps_decimal_coordinates_and_reference_without_resizing() {
        let bytes = tiff_with_gps();
        let output = rewrite_tiff_to_vec(
            &bytes,
            info(&bytes),
            ParseLimits::default(),
            &[TiffEdit::SetGpsDecimal {
                key: "GPS:GPSLatitude".to_owned(),
                value: "-48.8566".to_owned(),
            }],
        )
        .expect("GPS decimal rewrite should succeed");
        assert_eq!(output.len(), bytes.len());
        let metadata = read_tiff(
            &mut Cursor::new(output),
            info(&bytes),
            ParseLimits::default(),
        )
        .expect("edited GPS should remain readable");
        let latitude = metadata
            .find("GPS:LatitudeDecimal")
            .expect("derived latitude should be present")
            .value
            .clone();
        let TagValue::Float(latitude) = latitude else {
            panic!("derived latitude should be a float");
        };
        assert!((latitude + 48.8566).abs() < 0.000001);
        assert_eq!(
            metadata.find("GPS:GPSLatitudeRef").unwrap().display_value(),
            "S"
        );

        let mut longitude_bytes = tiff_with_gps();
        longitude_bytes[28..30].copy_from_slice(&4_u16.to_le_bytes());
        longitude_bytes[40..42].copy_from_slice(&3_u16.to_le_bytes());
        longitude_bytes[48] = b'E';
        let longitude_output = rewrite_tiff_to_vec(
            &longitude_bytes,
            info(&longitude_bytes),
            ParseLimits::default(),
            &[TiffEdit::SetGpsDecimal {
                key: "GPS:GPSLongitude".to_owned(),
                value: "-122.4194".to_owned(),
            }],
        )
        .expect("GPS longitude rewrite should succeed");
        let longitude_metadata = read_tiff(
            &mut Cursor::new(longitude_output),
            info(&longitude_bytes),
            ParseLimits::default(),
        )
        .expect("edited GPS longitude should remain readable");
        let longitude = longitude_metadata
            .find("GPS:LongitudeDecimal")
            .expect("derived longitude should be present")
            .value
            .clone();
        let TagValue::Float(longitude) = longitude else {
            panic!("derived longitude should be a float");
        };
        assert!((longitude + 122.4194).abs() < 0.000001);
        assert_eq!(
            longitude_metadata
                .find("GPS:GPSLongitudeRef")
                .unwrap()
                .display_value(),
            "W"
        );
    }

    #[test]
    fn deletes_gps_decimal_coordinates_as_zeroed_tombstones() {
        let bytes = tiff_with_gps();
        let output = rewrite_tiff_to_vec(
            &bytes,
            info(&bytes),
            ParseLimits::default(),
            &[TiffEdit::DeleteGpsDecimal {
                key: "GPS:GPSLatitude".to_owned(),
            }],
        )
        .expect("GPS deletion should succeed");
        assert_eq!(output.len(), bytes.len());
        let metadata = read_tiff(
            &mut Cursor::new(output),
            info(&bytes),
            ParseLimits::default(),
        )
        .expect("deleted GPS should remain readable");
        assert!(metadata.find("GPS:GPSLatitude").is_none());
        assert!(metadata.find("GPS:GPSLatitudeRef").is_none());
        assert!(metadata.find("GPS:LatitudeDecimal").is_none());
    }
}
