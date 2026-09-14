use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use metra_core::{FileFormat, FileInfo, MetraError, ParseLimits, Result};

use crate::atomic::atomic_replace;
use crate::tiff::read_tiff;
use crate::tiff_create::{
    EncodedGpsCoordinates, TiffCreateEntry, TiffCreateOptions, encode_gps_coordinates,
};

const BASE_ENTRY_COUNT: usize = 9;
const GPS_ENTRY_COUNT: usize = 5;
const GPS_IFD_BYTES: usize = 8 + GPS_ENTRY_COUNT * 20 + 8 + 48;

/// Create a minimal little-endian BigTIFF with one monochrome pixel.
///
/// The seed contains the standard image directory plus bounded ASCII EXIF
/// fields and an optional GPS coordinate IFD. It is intended for metadata
/// workflows, not general image authoring.
pub fn create_bigtiff_to_vec(options: &TiffCreateOptions, limits: ParseLimits) -> Result<Vec<u8>> {
    let gps = options
        .gps
        .as_ref()
        .map(|gps| encode_gps_coordinates(gps, limits))
        .transpose()?;
    let entries = collect_ascii_entries(&options.entries, limits)?;
    let entry_count = BASE_ENTRY_COUNT
        .checked_add(entries.len())
        .and_then(|count| count.checked_add(usize::from(gps.is_some())))
        .ok_or_else(|| MetraError::ResourceLimitExceeded {
            resource: "BigTIFF creation IFD entries".to_owned(),
            limit: limits.max_ifd_entries,
        })?;
    if entry_count > limits.max_ifd_entries {
        return Err(MetraError::ResourceLimitExceeded {
            resource: "BigTIFF creation IFD entries".to_owned(),
            limit: limits.max_ifd_entries,
        });
    }

    let directory_len = 8_usize
        .checked_add(entry_count.checked_mul(20).ok_or_else(|| {
            MetraError::ResourceLimitExceeded {
                resource: "BigTIFF creation metadata".to_owned(),
                limit: limits.max_metadata_bytes,
            }
        })?)
        .and_then(|value| value.checked_add(8))
        .ok_or_else(|| MetraError::ResourceLimitExceeded {
            resource: "BigTIFF creation metadata".to_owned(),
            limit: limits.max_metadata_bytes,
        })?;
    let ifd_end =
        16_usize
            .checked_add(directory_len)
            .ok_or_else(|| MetraError::ResourceLimitExceeded {
                resource: "BigTIFF creation metadata".to_owned(),
                limit: limits.max_metadata_bytes,
            })?;
    let data_offset = ifd_end
        .checked_add(usize::from(gps.is_some()).saturating_mul(GPS_IFD_BYTES))
        .ok_or_else(|| MetraError::ResourceLimitExceeded {
            resource: "BigTIFF creation metadata".to_owned(),
            limit: limits.max_metadata_bytes,
        })?;
    let ascii_bytes =
        entries.iter().try_fold(0_usize, |total, entry| {
            let value_len = entry.value.len().checked_add(1).ok_or_else(|| {
                MetraError::ResourceLimitExceeded {
                    resource: "BigTIFF creation value".to_owned(),
                    limit: limits.max_value_bytes,
                }
            })?;
            if value_len <= 8 {
                Ok(total)
            } else {
                total
                    .checked_add(value_len)
                    .ok_or_else(|| MetraError::ResourceLimitExceeded {
                        resource: "BigTIFF creation metadata".to_owned(),
                        limit: limits.max_metadata_bytes,
                    })
            }
        })?;
    let pixel_offset =
        data_offset
            .checked_add(ascii_bytes)
            .ok_or_else(|| MetraError::ResourceLimitExceeded {
                resource: "BigTIFF creation metadata".to_owned(),
                limit: limits.max_metadata_bytes,
            })?;
    let output_len =
        pixel_offset
            .checked_add(1)
            .ok_or_else(|| MetraError::ResourceLimitExceeded {
                resource: "BigTIFF creation metadata".to_owned(),
                limit: limits.max_metadata_bytes,
            })?;
    if output_len > limits.max_metadata_bytes {
        return Err(MetraError::ResourceLimitExceeded {
            resource: "BigTIFF creation metadata".to_owned(),
            limit: limits.max_metadata_bytes,
        });
    }
    let pixel_offset =
        u64::try_from(pixel_offset).map_err(|_| MetraError::ResourceLimitExceeded {
            resource: "BigTIFF creation offset".to_owned(),
            limit: u64::MAX as usize,
        })?;

    let mut output = Vec::with_capacity(output_len);
    output.extend_from_slice(b"II");
    output.extend_from_slice(&43_u16.to_le_bytes());
    output.extend_from_slice(&8_u16.to_le_bytes());
    output.extend_from_slice(&0_u16.to_le_bytes());
    output.extend_from_slice(&16_u64.to_le_bytes());
    output.extend_from_slice(&(entry_count as u64).to_le_bytes());

    push_long_entry(&mut output, 0x0100, 1);
    push_long_entry(&mut output, 0x0101, 1);
    push_short_entry(&mut output, 0x0102, 8);
    push_short_entry(&mut output, 0x0103, 1);
    push_short_entry(&mut output, 0x0106, 1);
    push_long8_entry(&mut output, 0x0111, pixel_offset);
    push_short_entry(&mut output, 0x0115, 1);
    push_long_entry(&mut output, 0x0116, 1);
    push_long8_entry(&mut output, 0x0117, 1);
    if gps.is_some() {
        let gps_offset = u64::try_from(ifd_end).map_err(|_| MetraError::ResourceLimitExceeded {
            resource: "BigTIFF creation offset".to_owned(),
            limit: u64::MAX as usize,
        })?;
        push_long8_entry(&mut output, 0x8825, gps_offset);
    }

    let mut next_data_offset =
        u64::try_from(data_offset).map_err(|_| MetraError::ResourceLimitExceeded {
            resource: "BigTIFF creation offset".to_owned(),
            limit: u64::MAX as usize,
        })?;
    for entry in &entries {
        let length = u64::try_from(entry.value.len().checked_add(1).ok_or_else(|| {
            MetraError::ResourceLimitExceeded {
                resource: "BigTIFF creation value".to_owned(),
                limit: limits.max_value_bytes,
            }
        })?)
        .map_err(|_| MetraError::ResourceLimitExceeded {
            resource: "BigTIFF creation value".to_owned(),
            limit: limits.max_value_bytes,
        })?;
        output.extend_from_slice(&entry.tag.to_le_bytes());
        output.extend_from_slice(&2_u16.to_le_bytes());
        output.extend_from_slice(&length.to_le_bytes());
        if length <= 8 {
            output.extend_from_slice(entry.value.as_bytes());
            output.push(0);
            output.resize(output.len() + (8 - length as usize), 0);
        } else {
            output.extend_from_slice(&next_data_offset.to_le_bytes());
            next_data_offset = next_data_offset.checked_add(length).ok_or_else(|| {
                MetraError::ResourceLimitExceeded {
                    resource: "BigTIFF creation offset".to_owned(),
                    limit: u64::MAX as usize,
                }
            })?;
        }
    }
    output.extend_from_slice(&0_u64.to_le_bytes());

    if let Some(gps) = &gps {
        write_gps_ifd(&mut output, ifd_end, gps);
    }

    for entry in &entries {
        if entry.value.len() + 1 > 8 {
            output.extend_from_slice(entry.value.as_bytes());
            output.push(0);
        }
    }
    output.push(0);

    debug_assert_eq!(output.len(), output_len);
    validate_created_bigtiff(&output, limits)?;
    Ok(output)
}

/// Create a new BigTIFF path without overwriting an existing destination.
pub fn create_bigtiff_path(
    path: impl AsRef<Path>,
    options: &TiffCreateOptions,
    limits: ParseLimits,
) -> Result<()> {
    let path = path.as_ref().to_path_buf();
    if path.exists() {
        return Err(MetraError::WriteFailure {
            message: format!("refusing to overwrite existing BigTIFF {}", path.display()),
        });
    }
    let bytes = create_bigtiff_to_vec(options, limits)?;
    let temp_path = temporary_path(&path)?;
    let result = (|| {
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
            .map_err(|source| write_error(&temp_path, source))?;
        output
            .write_all(&bytes)
            .map_err(|source| write_error(&temp_path, source))?;
        output
            .sync_all()
            .map_err(|source| write_error(&temp_path, source))?;
        drop(output);
        validate_created_bigtiff(&bytes, limits)?;
        if path.exists() {
            return Err(MetraError::WriteFailure {
                message: format!("refusing to overwrite existing BigTIFF {}", path.display()),
            });
        }
        atomic_replace(&temp_path, &path).map_err(|source| MetraError::WriteFailure {
            message: format!("cannot atomically create {}: {source}", path.display()),
        })?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result
}

fn validate_created_bigtiff(bytes: &[u8], limits: ParseLimits) -> Result<()> {
    read_tiff(
        &mut std::io::Cursor::new(bytes),
        FileInfo::new(
            "created.bigtiff".into(),
            bytes.len() as u64,
            FileFormat::Tiff,
        ),
        limits,
    )?;
    Ok(())
}

#[derive(Debug)]
struct ValidatedEntry {
    tag: u16,
    value: String,
}

fn collect_ascii_entries(
    entries: &[TiffCreateEntry],
    limits: ParseLimits,
) -> Result<Vec<ValidatedEntry>> {
    if entries.len() > limits.max_ifd_entries.saturating_sub(BASE_ENTRY_COUNT) {
        return Err(MetraError::ResourceLimitExceeded {
            resource: "BigTIFF creation IFD entries".to_owned(),
            limit: limits.max_ifd_entries,
        });
    }
    let mut validated: Vec<ValidatedEntry> = Vec::with_capacity(entries.len());
    for entry in entries {
        let tag =
            crate::tiff_create::ascii_tag_id(&entry.key).ok_or_else(|| MetraError::InvalidTag {
                context: "BigTIFF creation".to_owned(),
                message: format!("unsupported ASCII creation key {}", entry.key),
            })?;
        if entry.value.as_bytes().contains(&0) {
            return Err(MetraError::InvalidTag {
                context: format!("BigTIFF creation {}", entry.key),
                message: "ASCII values may not contain NUL bytes".to_owned(),
            });
        }
        let value_len =
            entry
                .value
                .len()
                .checked_add(1)
                .ok_or_else(|| MetraError::ResourceLimitExceeded {
                    resource: "BigTIFF creation value".to_owned(),
                    limit: limits.max_value_bytes,
                })?;
        if value_len > limits.max_value_bytes {
            return Err(MetraError::ResourceLimitExceeded {
                resource: format!("BigTIFF creation value {}", entry.key),
                limit: limits.max_value_bytes,
            });
        }
        if validated.iter().any(|existing| existing.tag == tag) {
            return Err(MetraError::InvalidTag {
                context: "BigTIFF creation".to_owned(),
                message: format!("duplicate ASCII creation key {}", entry.key),
            });
        }
        validated.push(ValidatedEntry {
            tag,
            value: entry.value.clone(),
        });
    }
    validated.sort_by_key(|entry| entry.tag);
    Ok(validated)
}

fn push_short_entry(output: &mut Vec<u8>, tag: u16, value: u16) {
    output.extend_from_slice(&tag.to_le_bytes());
    output.extend_from_slice(&3_u16.to_le_bytes());
    output.extend_from_slice(&1_u64.to_le_bytes());
    output.extend_from_slice(&value.to_le_bytes());
    output.extend_from_slice(&[0; 6]);
}

fn push_long_entry(output: &mut Vec<u8>, tag: u16, value: u32) {
    output.extend_from_slice(&tag.to_le_bytes());
    output.extend_from_slice(&4_u16.to_le_bytes());
    output.extend_from_slice(&1_u64.to_le_bytes());
    output.extend_from_slice(&value.to_le_bytes());
    output.extend_from_slice(&[0; 4]);
}

fn push_long8_entry(output: &mut Vec<u8>, tag: u16, value: u64) {
    output.extend_from_slice(&tag.to_le_bytes());
    output.extend_from_slice(&16_u16.to_le_bytes());
    output.extend_from_slice(&1_u64.to_le_bytes());
    output.extend_from_slice(&value.to_le_bytes());
}

fn write_gps_ifd(output: &mut Vec<u8>, ifd_offset: usize, gps: &EncodedGpsCoordinates) {
    debug_assert_eq!(output.len(), ifd_offset);
    let values_offset = ifd_offset + 8 + GPS_ENTRY_COUNT * 20 + 8;
    output.extend_from_slice(&(GPS_ENTRY_COUNT as u64).to_le_bytes());
    push_gps_byte_entry(output, 0x0000, [2, 3, 0, 0]);
    push_gps_ascii_reference(output, 0x0001, gps.latitude_reference);
    push_gps_rational_entry(output, 0x0002, values_offset as u64);
    push_gps_ascii_reference(output, 0x0003, gps.longitude_reference);
    push_gps_rational_entry(output, 0x0004, (values_offset + 24) as u64);
    output.extend_from_slice(&0_u64.to_le_bytes());
    output.extend_from_slice(&gps.rationals);
    debug_assert_eq!(output.len(), ifd_offset + GPS_IFD_BYTES);
}

fn push_gps_byte_entry(output: &mut Vec<u8>, tag: u16, value: [u8; 4]) {
    output.extend_from_slice(&tag.to_le_bytes());
    output.extend_from_slice(&1_u16.to_le_bytes());
    output.extend_from_slice(&4_u64.to_le_bytes());
    output.extend_from_slice(&value);
    output.extend_from_slice(&[0; 4]);
}

fn push_gps_ascii_reference(output: &mut Vec<u8>, tag: u16, reference: u8) {
    output.extend_from_slice(&tag.to_le_bytes());
    output.extend_from_slice(&2_u16.to_le_bytes());
    output.extend_from_slice(&2_u64.to_le_bytes());
    output.extend_from_slice(&[reference, 0, 0, 0, 0, 0, 0, 0]);
}

fn push_gps_rational_entry(output: &mut Vec<u8>, tag: u16, value_offset: u64) {
    output.extend_from_slice(&tag.to_le_bytes());
    output.extend_from_slice(&5_u16.to_le_bytes());
    output.extend_from_slice(&3_u64.to_le_bytes());
    output.extend_from_slice(&value_offset.to_le_bytes());
}

fn temporary_path(path: &Path) -> Result<PathBuf> {
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("created.bigtiff");
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

fn write_error(path: &Path, source: std::io::Error) -> MetraError {
    MetraError::WriteFailure {
        message: format!("{}: {source}", path.display()),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    #[test]
    fn creates_readable_bigtiff_with_ascii_metadata() {
        let options = TiffCreateOptions::new()
            .with_ascii("Make", "Metra")
            .with_ascii("Software", "BigTIFF metadata");
        let bytes = create_bigtiff_to_vec(&options, ParseLimits::default()).unwrap();
        assert_eq!(&bytes[..4], b"II+\0");
        let metadata = read_tiff(
            &mut std::io::Cursor::new(bytes.clone()),
            FileInfo::new(
                "created.bigtiff".into(),
                bytes.len() as u64,
                FileFormat::Tiff,
            ),
            ParseLimits::default(),
        )
        .unwrap();
        assert_eq!(metadata.find("EXIF:Make").unwrap().display_value(), "Metra");
        assert_eq!(
            metadata.find("EXIF:Software").unwrap().display_value(),
            "BigTIFF metadata"
        );
        assert_eq!(
            metadata.find("EXIF:ImageWidth").unwrap().display_value(),
            "1"
        );
    }

    #[test]
    fn creates_readable_bigtiff_with_gps_coordinates() {
        let options = TiffCreateOptions::new().with_gps_coordinates("48.8566", "2.3522");
        let bytes = create_bigtiff_to_vec(&options, ParseLimits::default())
            .expect("BigTIFF GPS creation should succeed");
        let metadata = read_tiff(
            &mut std::io::Cursor::new(bytes.clone()),
            FileInfo::new(
                "created-gps.bigtiff".into(),
                bytes.len() as u64,
                FileFormat::Tiff,
            ),
            ParseLimits::default(),
        )
        .expect("created GPS BigTIFF should remain readable");
        let latitude = metadata
            .find("GPS:LatitudeDecimal")
            .expect("derived latitude should be present")
            .display_value()
            .parse::<f64>()
            .expect("derived latitude should be numeric");
        let longitude = metadata
            .find("GPS:LongitudeDecimal")
            .expect("derived longitude should be present")
            .display_value()
            .parse::<f64>()
            .expect("derived longitude should be numeric");
        assert!((latitude - 48.8566).abs() < 0.000001);
        assert!((longitude - 2.3522).abs() < 0.000001);
        assert_eq!(
            metadata.find("GPS:GPSLatitudeRef").unwrap().display_value(),
            "N"
        );
        assert_eq!(
            metadata
                .find("GPS:GPSLongitudeRef")
                .unwrap()
                .display_value(),
            "E"
        );
    }

    #[test]
    fn rejects_overwrite_for_bigtiff_paths() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("metra-bigtiff-create-{unique}"));
        fs::create_dir(&directory).unwrap();
        let path = directory.join("created.btf");
        create_bigtiff_path(&path, &TiffCreateOptions::new(), ParseLimits::default()).unwrap();
        assert!(
            create_bigtiff_path(&path, &TiffCreateOptions::new(), ParseLimits::default()).is_err()
        );
        fs::remove_dir_all(directory).unwrap();
    }
}
