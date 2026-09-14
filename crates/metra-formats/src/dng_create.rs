use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use metra_core::{FileFormat, FileInfo, MetraError, ParseLimits, Result};

use crate::atomic::atomic_replace;
use crate::raw::read_raw;
use crate::tiff_create::{TiffCreateEntry, TiffCreateOptions, create_tiff_to_vec};

/// Options for creating a bounded 1x1 DNG/TIFF-like RAW seed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DngCreateOptions {
    pub entries: Vec<TiffCreateEntry>,
}

impl DngCreateOptions {
    /// Start an empty DNG seed.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add one bounded EXIF ASCII entry and return the updated options.
    pub fn with_ascii(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.entries.push(TiffCreateEntry::ascii(key, value));
        self
    }

    /// Add one bounded EXIF ASCII entry in place.
    pub fn push_ascii(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.entries.push(TiffCreateEntry::ascii(key, value));
    }
}

/// Create a readable DNG seed using a classic little-endian TIFF container.
pub fn create_dng_to_vec(options: &DngCreateOptions, limits: ParseLimits) -> Result<Vec<u8>> {
    let tiff_options = TiffCreateOptions {
        entries: options.entries.clone(),
    };
    let mut output = create_tiff_to_vec(&tiff_options, limits)?;
    let next_ifd_offset = 8_usize
        .checked_add(2)
        .and_then(|value| value.checked_add(options.entries.len().saturating_add(9) * 12))
        .ok_or_else(|| MetraError::ResourceLimitExceeded {
            resource: "DNG creation IFD".to_owned(),
            limit: limits.max_metadata_bytes,
        })?;
    if next_ifd_offset.checked_add(4).is_none() || next_ifd_offset + 4 > output.len() {
        return Err(MetraError::InvalidOffset {
            context: "DNG creation IFD chain".to_owned(),
            offset: next_ifd_offset as u64,
        });
    }
    let dng_ifd_offset =
        u32::try_from(output.len()).map_err(|_| MetraError::ResourceLimitExceeded {
            resource: "DNG creation IFD offset".to_owned(),
            limit: u32::MAX as usize,
        })?;
    output[next_ifd_offset..next_ifd_offset + 4].copy_from_slice(&dng_ifd_offset.to_le_bytes());
    output.extend_from_slice(&1_u16.to_le_bytes());
    output.extend_from_slice(&0xC612_u16.to_le_bytes());
    output.extend_from_slice(&1_u16.to_le_bytes());
    output.extend_from_slice(&4_u32.to_le_bytes());
    output.extend_from_slice(&[1, 4, 0, 0]);
    output.extend_from_slice(&0_u32.to_le_bytes());
    if output.len() > limits.max_metadata_bytes || output.len() > limits.max_value_bytes {
        return Err(MetraError::ResourceLimitExceeded {
            resource: "DNG creation output".to_owned(),
            limit: limits.max_metadata_bytes.min(limits.max_value_bytes),
        });
    }
    validate_created_dng(&output, limits)?;
    Ok(output)
}

/// Create a new DNG path without overwriting an existing destination.
pub fn create_dng_path(
    path: impl AsRef<Path>,
    options: &DngCreateOptions,
    limits: ParseLimits,
) -> Result<()> {
    let path = path.as_ref().to_path_buf();
    if path.exists() {
        return Err(MetraError::WriteFailure {
            message: format!("refusing to overwrite {}", path.display()),
        });
    }
    let bytes = create_dng_to_vec(options, limits)?;
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
        validate_created_dng(&bytes, limits)?;
        if path.exists() {
            return Err(MetraError::WriteFailure {
                message: format!("refusing to overwrite {}", path.display()),
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

fn validate_created_dng(bytes: &[u8], limits: ParseLimits) -> Result<()> {
    let metadata = read_raw(
        &mut std::io::Cursor::new(bytes),
        FileInfo::new("created.dng".into(), bytes.len() as u64, FileFormat::Raw),
        limits,
    )?;
    if metadata
        .find("RAW:Variant")
        .is_none_or(|tag| tag.display_value() != "DNG")
    {
        return Err(MetraError::WriteFailure {
            message: "created DNG did not validate as a DNG RAW variant".to_owned(),
        });
    }
    Ok(())
}

fn temporary_path(path: &Path) -> Result<PathBuf> {
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("created.dng");
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
    use super::*;

    #[test]
    fn creates_readable_dng_with_version_and_ascii_metadata() {
        let options = DngCreateOptions::new().with_ascii("Make", "Metra");
        let bytes = create_dng_to_vec(&options, ParseLimits::default()).unwrap();
        let metadata = read_raw(
            &mut std::io::Cursor::new(bytes.clone()),
            FileInfo::new("created.dng".into(), bytes.len() as u64, FileFormat::Raw),
            ParseLimits::default(),
        )
        .unwrap();
        assert_eq!(metadata.find("RAW:Variant").unwrap().display_value(), "DNG");
        assert_eq!(metadata.find("EXIF:Make").unwrap().display_value(), "Metra");
        assert_eq!(
            metadata.find("DNG:DNGVersion").unwrap().display_value(),
            "1, 4, 0, 0"
        );
    }

    #[test]
    fn rejects_overwrite_for_dng_paths() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("metra-dng-create-{unique}"));
        fs::create_dir(&directory).unwrap();
        let path = directory.join("created.dng");
        create_dng_path(&path, &DngCreateOptions::new(), ParseLimits::default()).unwrap();
        assert!(create_dng_path(&path, &DngCreateOptions::new(), ParseLimits::default()).is_err());
        fs::remove_dir_all(directory).unwrap();
    }
}
