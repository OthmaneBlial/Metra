use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use metra_core::{FileFormat, FileInfo, MetraError, ParseLimits, Result};

use crate::atomic::atomic_replace;
use crate::icc::read_icc;

/// One bounded text tag for a new ICC profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IccCreateEntry {
    pub name: String,
    pub value: String,
}

impl IccCreateEntry {
    /// Create one named ICC text tag. The name is validated when built.
    pub fn text(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
        }
    }
}

/// Options for creating a minimal RGB monitor ICC profile.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IccCreateOptions {
    pub text: Vec<IccCreateEntry>,
}

impl IccCreateOptions {
    /// Start with no optional text tags.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add one text tag and return the updated options.
    pub fn with_text(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.text.push(IccCreateEntry::text(name, value));
        self
    }

    /// Add one text tag in place.
    pub fn push_text(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.text.push(IccCreateEntry::text(name, value));
    }
}

/// Create a minimal valid RGB monitor ICC profile in memory.
pub fn create_icc_to_vec(options: &IccCreateOptions, limits: ParseLimits) -> Result<Vec<u8>> {
    if options.text.len() > limits.max_ifd_entries {
        return Err(MetraError::ResourceLimitExceeded {
            resource: "ICC creation tag entries".to_owned(),
            limit: limits.max_ifd_entries,
        });
    }

    let mut entries = Vec::with_capacity(options.text.len());
    for entry in &options.text {
        let signature = icc_text_signature(&entry.name).ok_or_else(|| MetraError::InvalidTag {
            context: "ICC creation".to_owned(),
            message: format!("unsupported text tag {}", entry.name),
        })?;
        let value = entry.value.as_bytes();
        if value.contains(&0) {
            return Err(MetraError::InvalidTag {
                context: format!("ICC creation {}", entry.name),
                message: "text values may not contain NUL bytes".to_owned(),
            });
        }
        if value.is_empty()
            || !value
                .iter()
                .all(|byte| *byte == b' ' || byte.is_ascii_graphic())
        {
            return Err(MetraError::InvalidTag {
                context: format!("ICC creation {}", entry.name),
                message: "text values must be non-empty printable ASCII".to_owned(),
            });
        }
        if entry.value.len() >= limits.max_value_bytes {
            return Err(MetraError::ResourceLimitExceeded {
                resource: format!("ICC creation value {}", entry.name),
                limit: limits.max_value_bytes,
            });
        }
        if entries
            .iter()
            .any(|existing: &([u8; 4], String)| existing.0 == signature)
        {
            return Err(MetraError::InvalidTag {
                context: "ICC creation".to_owned(),
                message: format!("duplicate text tag {}", entry.name),
            });
        }
        entries.push((signature, entry.value.clone()));
    }

    let table_end = 132_usize
        .checked_add(entries.len().checked_mul(12).ok_or_else(|| {
            MetraError::ResourceLimitExceeded {
                resource: "ICC creation tag table".to_owned(),
                limit: limits.max_metadata_bytes,
            }
        })?)
        .ok_or_else(|| MetraError::ResourceLimitExceeded {
            resource: "ICC creation tag table".to_owned(),
            limit: limits.max_metadata_bytes,
        })?;
    if table_end > limits.max_metadata_bytes || table_end > limits.max_value_bytes {
        return Err(MetraError::ResourceLimitExceeded {
            resource: "ICC creation tag table".to_owned(),
            limit: limits.max_metadata_bytes.min(limits.max_value_bytes),
        });
    }
    let tag_count =
        u32::try_from(entries.len()).map_err(|_| MetraError::ResourceLimitExceeded {
            resource: "ICC creation tag entries".to_owned(),
            limit: limits.max_ifd_entries,
        })?;
    let mut output = vec![0_u8; table_end];
    output[8] = 4;
    output[9] = 0x30;
    output[12..16].copy_from_slice(b"mntr");
    output[16..20].copy_from_slice(b"RGB ");
    output[20..24].copy_from_slice(b"XYZ ");
    output[24..36].copy_from_slice(&[0x07, 0xEA, 0, 1, 0, 1, 0, 0, 0, 0, 0, 0]);
    output[36..40].copy_from_slice(b"acsp");
    output[40..44].copy_from_slice(b"MSFT");
    output[48..52].copy_from_slice(b"METR");
    output[52..56].copy_from_slice(b"METR");
    output[64..68].copy_from_slice(&0_u32.to_be_bytes());
    output[68..72].copy_from_slice(&63_192_i32.to_be_bytes());
    output[72..76].copy_from_slice(&65_536_i32.to_be_bytes());
    output[76..80].copy_from_slice(&54_092_i32.to_be_bytes());
    output[80..84].copy_from_slice(b"METR");
    output[128..132].copy_from_slice(&tag_count.to_be_bytes());

    for (index, (signature, value)) in entries.iter().enumerate() {
        let data_offset =
            u32::try_from(output.len()).map_err(|_| MetraError::ResourceLimitExceeded {
                resource: "ICC creation profile".to_owned(),
                limit: limits.max_metadata_bytes,
            })?;
        let mut data = b"desc".to_vec();
        data.extend_from_slice(&[0; 4]);
        let length =
            value
                .len()
                .checked_add(1)
                .ok_or_else(|| MetraError::ResourceLimitExceeded {
                    resource: "ICC creation value".to_owned(),
                    limit: limits.max_value_bytes,
                })?;
        let length = u32::try_from(length).map_err(|_| MetraError::ResourceLimitExceeded {
            resource: "ICC creation value".to_owned(),
            limit: limits.max_value_bytes,
        })?;
        data.extend_from_slice(&length.to_be_bytes());
        data.extend_from_slice(value.as_bytes());
        data.push(0);
        let data_size =
            u32::try_from(data.len()).map_err(|_| MetraError::ResourceLimitExceeded {
                resource: "ICC creation value".to_owned(),
                limit: limits.max_value_bytes,
            })?;
        output.extend_from_slice(&data);
        while output.len() % 4 != 0 {
            output.push(0);
        }

        let table_offset = 132 + index * 12;
        output[table_offset..table_offset + 4].copy_from_slice(signature);
        output[table_offset + 4..table_offset + 8].copy_from_slice(&data_offset.to_be_bytes());
        output[table_offset + 8..table_offset + 12].copy_from_slice(&data_size.to_be_bytes());
    }

    let profile_size =
        u32::try_from(output.len()).map_err(|_| MetraError::ResourceLimitExceeded {
            resource: "ICC creation profile".to_owned(),
            limit: limits.max_metadata_bytes,
        })?;
    output[0..4].copy_from_slice(&profile_size.to_be_bytes());
    if output.len() > limits.max_metadata_bytes || output.len() > limits.max_value_bytes {
        return Err(MetraError::ResourceLimitExceeded {
            resource: "ICC creation profile".to_owned(),
            limit: limits.max_metadata_bytes.min(limits.max_value_bytes),
        });
    }
    validate_created_icc(&output, limits)?;
    Ok(output)
}

/// Create a new ICC profile path without overwriting an existing file.
pub fn create_icc_path(
    path: impl AsRef<Path>,
    options: &IccCreateOptions,
    limits: ParseLimits,
) -> Result<()> {
    let path = path.as_ref().to_path_buf();
    if path.exists() {
        return Err(MetraError::WriteFailure {
            message: format!("refusing to overwrite existing ICC {}", path.display()),
        });
    }
    let bytes = create_icc_to_vec(options, limits)?;
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
        validate_created_icc(&bytes, limits)?;
        if path.exists() {
            return Err(MetraError::WriteFailure {
                message: format!("refusing to overwrite existing ICC {}", path.display()),
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

fn icc_text_signature(name: &str) -> Option<[u8; 4]> {
    match name {
        "Description" | "desc" => Some(*b"desc"),
        "Copyright" | "cprt" => Some(*b"cprt"),
        "ManufacturerDescription" | "dmnd" => Some(*b"dmnd"),
        "ModelDescription" | "dmdd" => Some(*b"dmdd"),
        _ => None,
    }
}

fn validate_created_icc(bytes: &[u8], limits: ParseLimits) -> Result<()> {
    read_icc(
        &mut std::io::Cursor::new(bytes),
        FileInfo::new("created.icc".into(), bytes.len() as u64, FileFormat::Icc),
        limits,
    )?;
    Ok(())
}

fn temporary_path(path: &Path) -> Result<PathBuf> {
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("created.icc");
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
    fn creates_and_revalidates_profile_with_text_tags() {
        let options = IccCreateOptions::new()
            .with_text("Description", "Metra sRGB")
            .with_text("Copyright", "Copyright Metra");
        let bytes = create_icc_to_vec(&options, ParseLimits::default())
            .expect("ICC creation should succeed");
        let metadata = read_icc(
            &mut std::io::Cursor::new(bytes.clone()),
            FileInfo::new("created.icc".into(), bytes.len() as u64, FileFormat::Icc),
            ParseLimits::default(),
        )
        .expect("created ICC should remain readable");
        assert_eq!(metadata.file_info.format, FileFormat::Icc);
        assert_eq!(
            metadata.find("ICC:Description").unwrap().value,
            TagValue::String("Metra sRGB".to_owned())
        );
        assert_eq!(
            metadata.find("ICC:Copyright").unwrap().display_value(),
            "Copyright Metra"
        );
        assert!(metadata.warnings.is_empty());
    }

    #[test]
    fn rejects_invalid_duplicate_and_oversized_text() {
        let invalid = IccCreateOptions::new().with_text("Unknown", "value");
        assert!(matches!(
            create_icc_to_vec(&invalid, ParseLimits::default()),
            Err(MetraError::InvalidTag { .. })
        ));
        let duplicate = IccCreateOptions::new()
            .with_text("Description", "one")
            .with_text("desc", "two");
        assert!(matches!(
            create_icc_to_vec(&duplicate, ParseLimits::default()),
            Err(MetraError::InvalidTag { .. })
        ));
        let limits = ParseLimits {
            max_value_bytes: 4,
            ..ParseLimits::default()
        };
        let oversized = IccCreateOptions::new().with_text("Description", "long");
        assert!(matches!(
            create_icc_to_vec(&oversized, limits),
            Err(MetraError::ResourceLimitExceeded { .. })
        ));
    }

    #[test]
    fn creates_new_path_without_overwriting_existing_file() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after Unix epoch")
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("metra-icc-create-{unique}"));
        fs::create_dir(&directory).expect("temporary directory should be created");
        let path = directory.join("created.icc");
        let options = IccCreateOptions::new().with_text("Description", "Metra");

        create_icc_path(&path, &options, ParseLimits::default())
            .expect("new ICC path should be created");
        assert!(path.is_file());
        assert!(matches!(
            create_icc_path(&path, &options, ParseLimits::default()),
            Err(MetraError::WriteFailure { .. })
        ));

        fs::remove_file(&path).expect("created ICC should be removed");
        fs::remove_dir(&directory).expect("temporary directory should be removed");
    }
}
