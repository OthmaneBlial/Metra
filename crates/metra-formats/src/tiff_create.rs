use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use metra_core::{FileFormat, FileInfo, MetraError, ParseLimits, Result};

use crate::atomic::atomic_replace;
use crate::tiff::read_tiff;

const BASE_ENTRY_COUNT: usize = 9;

/// One bounded ASCII value to place in the initial IFD of a new TIFF.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TiffCreateEntry {
    pub key: String,
    pub value: String,
}

impl TiffCreateEntry {
    /// Create a canonical EXIF ASCII entry such as `Make` or `Artist`.
    pub fn ascii(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            value: value.into(),
        }
    }
}

/// Options for creating a minimal, readable classic TIFF container.
///
/// The generated file contains a valid 1x1 monochrome image and an IFD0 with
/// the supplied ASCII EXIF fields. It is intended as a safe metadata seed,
/// not as a general image encoder.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TiffCreateOptions {
    pub entries: Vec<TiffCreateEntry>,
}

impl TiffCreateOptions {
    /// Start with no optional metadata entries.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add one ASCII EXIF entry and return the updated options.
    pub fn with_ascii(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.entries.push(TiffCreateEntry::ascii(key, value));
        self
    }

    /// Add one ASCII EXIF entry in place.
    pub fn push_ascii(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.entries.push(TiffCreateEntry::ascii(key, value));
    }
}

/// Create a minimal classic little-endian TIFF in memory.
pub fn create_tiff_to_vec(options: &TiffCreateOptions, limits: ParseLimits) -> Result<Vec<u8>> {
    let mut entries = Vec::with_capacity(options.entries.len());
    for entry in &options.entries {
        let tag = ascii_tag_id(&entry.key).ok_or_else(|| MetraError::InvalidTag {
            context: "TIFF creation".to_owned(),
            message: format!("unsupported ASCII creation key {}", entry.key),
        })?;
        if entry.value.as_bytes().contains(&0) {
            return Err(MetraError::InvalidTag {
                context: format!("TIFF creation {}", entry.key),
                message: "ASCII values may not contain NUL bytes".to_owned(),
            });
        }
        let value_len =
            entry
                .value
                .len()
                .checked_add(1)
                .ok_or_else(|| MetraError::ResourceLimitExceeded {
                    resource: "TIFF creation value".to_owned(),
                    limit: limits.max_value_bytes,
                })?;
        if value_len > limits.max_value_bytes {
            return Err(MetraError::ResourceLimitExceeded {
                resource: format!("TIFF creation value {}", entry.key),
                limit: limits.max_value_bytes,
            });
        }
        if entries.iter().any(|(existing, _, _)| *existing == tag) {
            return Err(MetraError::InvalidTag {
                context: "TIFF creation".to_owned(),
                message: format!("duplicate ASCII creation key {}", entry.key),
            });
        }
        entries.push((tag, entry.key.clone(), entry.value.clone()));
    }

    let entry_count = BASE_ENTRY_COUNT.checked_add(entries.len()).ok_or_else(|| {
        MetraError::ResourceLimitExceeded {
            resource: "TIFF creation IFD entries".to_owned(),
            limit: limits.max_ifd_entries,
        }
    })?;
    if entry_count > limits.max_ifd_entries || entry_count > u16::MAX as usize {
        return Err(MetraError::ResourceLimitExceeded {
            resource: "TIFF creation IFD entries".to_owned(),
            limit: limits.max_ifd_entries,
        });
    }
    entries.sort_by_key(|(tag, _, _)| *tag);

    let ifd_bytes = entry_count
        .checked_mul(12)
        .and_then(|bytes| bytes.checked_add(6))
        .ok_or_else(|| MetraError::ResourceLimitExceeded {
            resource: "TIFF creation metadata".to_owned(),
            limit: limits.max_metadata_bytes,
        })?;
    let data_offset =
        8usize
            .checked_add(ifd_bytes)
            .ok_or_else(|| MetraError::ResourceLimitExceeded {
                resource: "TIFF creation metadata".to_owned(),
                limit: limits.max_metadata_bytes,
            })?;
    let ascii_bytes = entries.iter().try_fold(0usize, |total, (_, _, value)| {
        total
            .checked_add(value.len() + 1)
            .ok_or(MetraError::ResourceLimitExceeded {
                resource: "TIFF creation metadata".to_owned(),
                limit: limits.max_metadata_bytes,
            })
    })?;
    let pixel_offset =
        data_offset
            .checked_add(ascii_bytes)
            .ok_or_else(|| MetraError::ResourceLimitExceeded {
                resource: "TIFF creation metadata".to_owned(),
                limit: limits.max_metadata_bytes,
            })?;
    let output_len =
        pixel_offset
            .checked_add(1)
            .ok_or_else(|| MetraError::ResourceLimitExceeded {
                resource: "TIFF creation metadata".to_owned(),
                limit: limits.max_metadata_bytes,
            })?;
    if output_len > limits.max_metadata_bytes || pixel_offset > u32::MAX as usize {
        return Err(MetraError::ResourceLimitExceeded {
            resource: "TIFF creation metadata".to_owned(),
            limit: limits.max_metadata_bytes,
        });
    }

    let mut output = Vec::with_capacity(output_len);
    output.extend_from_slice(b"II");
    output.extend_from_slice(&42u16.to_le_bytes());
    output.extend_from_slice(&8u32.to_le_bytes());
    output.extend_from_slice(&(entry_count as u16).to_le_bytes());

    push_long_entry(&mut output, 0x0100, 1); // ImageWidth
    push_long_entry(&mut output, 0x0101, 1); // ImageLength
    push_short_entry(&mut output, 0x0102, 8); // BitsPerSample
    push_short_entry(&mut output, 0x0103, 1); // Compression: none
    push_short_entry(&mut output, 0x0106, 1); // PhotometricInterpretation
    push_long_entry(&mut output, 0x0111, pixel_offset as u32); // StripOffsets
    push_short_entry(&mut output, 0x0115, 1); // SamplesPerPixel
    push_long_entry(&mut output, 0x0116, 1); // RowsPerStrip
    push_long_entry(&mut output, 0x0117, 1); // StripByteCounts

    let mut next_data_offset = data_offset;
    for (tag, _, value) in &entries {
        let length =
            value
                .len()
                .checked_add(1)
                .ok_or_else(|| MetraError::ResourceLimitExceeded {
                    resource: "TIFF creation value".to_owned(),
                    limit: limits.max_value_bytes,
                })?;
        output.extend_from_slice(&tag.to_le_bytes());
        output.extend_from_slice(&2u16.to_le_bytes()); // ASCII
        output.extend_from_slice(&(length as u32).to_le_bytes());
        output.extend_from_slice(&(next_data_offset as u32).to_le_bytes());
        next_data_offset += length;
    }
    output.extend_from_slice(&0u32.to_le_bytes());
    for (_, _, value) in &entries {
        output.extend_from_slice(value.as_bytes());
        output.push(0);
    }
    output.push(0);

    debug_assert_eq!(output.len(), output_len);
    validate_created_tiff(&output, limits)?;
    Ok(output)
}

/// Create a new TIFF path without overwriting an existing destination.
pub fn create_tiff_path(
    path: impl AsRef<Path>,
    options: &TiffCreateOptions,
    limits: ParseLimits,
) -> Result<()> {
    let path = path.as_ref().to_path_buf();
    if path.exists() {
        return Err(MetraError::WriteFailure {
            message: format!("refusing to overwrite existing TIFF {}", path.display()),
        });
    }
    let bytes = create_tiff_to_vec(options, limits)?;
    let temp_path = temporary_path(&path)?;
    let result = (|| {
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
            .map_err(|source| MetraError::WriteFailure {
                message: format!("cannot create {}: {source}", temp_path.display()),
            })?;
        output
            .write_all(&bytes)
            .map_err(|source| write_error(&temp_path, source))?;
        output
            .sync_all()
            .map_err(|source| write_error(&temp_path, source))?;
        drop(output);
        validate_created_tiff(&bytes, limits)?;
        if path.exists() {
            return Err(MetraError::WriteFailure {
                message: format!("refusing to overwrite existing TIFF {}", path.display()),
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

fn validate_created_tiff(bytes: &[u8], limits: ParseLimits) -> Result<()> {
    read_tiff(
        &mut std::io::Cursor::new(bytes),
        FileInfo::new("created.tif".into(), bytes.len() as u64, FileFormat::Tiff),
        limits,
    )?;
    Ok(())
}

fn ascii_tag_id(key: &str) -> Option<u16> {
    let key = key
        .strip_prefix("TIFF:")
        .or_else(|| key.strip_prefix("EXIF:"))
        .unwrap_or(key);
    match key {
        "ImageDescription" => Some(0x010E),
        "Make" => Some(0x010F),
        "Model" => Some(0x0110),
        "Software" => Some(0x0131),
        "Artist" => Some(0x013B),
        "Copyright" => Some(0x8298),
        _ => None,
    }
}

fn push_short_entry(output: &mut Vec<u8>, tag: u16, value: u16) {
    output.extend_from_slice(&tag.to_le_bytes());
    output.extend_from_slice(&3u16.to_le_bytes());
    output.extend_from_slice(&1u32.to_le_bytes());
    output.extend_from_slice(&value.to_le_bytes());
    output.extend_from_slice(&0u16.to_le_bytes());
}

fn push_long_entry(output: &mut Vec<u8>, tag: u16, value: u32) {
    output.extend_from_slice(&tag.to_le_bytes());
    output.extend_from_slice(&4u16.to_le_bytes());
    output.extend_from_slice(&1u32.to_le_bytes());
    output.extend_from_slice(&value.to_le_bytes());
}

fn temporary_path(path: &Path) -> Result<PathBuf> {
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("created.tif");
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
    use metra_core::TagValue;

    #[test]
    fn creates_readable_one_pixel_tiff_with_ascii_metadata() {
        let options = TiffCreateOptions::new()
            .with_ascii("EXIF:Make", "Metra")
            .with_ascii("Artist", "Othmane");
        let bytes = create_tiff_to_vec(&options, ParseLimits::default())
            .expect("minimal TIFF creation should succeed");
        let metadata = read_tiff(
            &mut std::io::Cursor::new(bytes.clone()),
            FileInfo::new("created.tif".into(), bytes.len() as u64, FileFormat::Tiff),
            ParseLimits::default(),
        )
        .expect("created TIFF should remain readable");
        assert_eq!(
            metadata.find("EXIF:Make").unwrap().value,
            TagValue::String("Metra".into())
        );
        assert_eq!(
            metadata.find("EXIF:Artist").unwrap().display_value(),
            "Othmane"
        );
        assert_eq!(
            metadata.find("EXIF:ImageWidth").unwrap().display_value(),
            "1"
        );
    }

    #[test]
    fn rejects_unknown_duplicate_and_nul_entries() {
        let unknown = TiffCreateOptions::new().with_ascii("EXIF:Rating", "5");
        assert!(matches!(
            create_tiff_to_vec(&unknown, ParseLimits::default()),
            Err(MetraError::InvalidTag { .. })
        ));

        let duplicate = TiffCreateOptions::new()
            .with_ascii("Make", "one")
            .with_ascii("EXIF:Make", "two");
        assert!(matches!(
            create_tiff_to_vec(&duplicate, ParseLimits::default()),
            Err(MetraError::InvalidTag { .. })
        ));

        let nul = TiffCreateOptions::new().with_ascii("Make", "bad\0value");
        assert!(matches!(
            create_tiff_to_vec(&nul, ParseLimits::default()),
            Err(MetraError::InvalidTag { .. })
        ));
    }

    #[test]
    fn enforces_creation_limits() {
        let options = TiffCreateOptions::new().with_ascii("Make", "Canon");
        let limits = ParseLimits {
            max_value_bytes: 4,
            ..ParseLimits::default()
        };
        assert!(matches!(
            create_tiff_to_vec(&options, limits),
            Err(MetraError::ResourceLimitExceeded { .. })
        ));
    }

    #[test]
    fn creates_new_path_without_overwriting_existing_file() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after Unix epoch")
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("metra-tiff-create-{unique}"));
        fs::create_dir(&directory).expect("temporary directory should be created");
        let path = directory.join("created.tif");
        let options = TiffCreateOptions::new().with_ascii("Software", "Metra");

        create_tiff_path(&path, &options, ParseLimits::default())
            .expect("new TIFF path should be created");
        assert!(path.is_file());
        assert!(matches!(
            create_tiff_path(&path, &options, ParseLimits::default()),
            Err(MetraError::WriteFailure { .. })
        ));

        fs::remove_file(&path).expect("created TIFF should be removed");
        fs::remove_dir(&directory).expect("temporary directory should be removed");
    }
}
