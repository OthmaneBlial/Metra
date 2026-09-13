use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use metra_core::{FileFormat, FileInfo, MetraError, ParseLimits, Result};

use crate::atomic::atomic_replace;
use crate::flac::read_flac;

const STREAMINFO_LENGTH: usize = 34;
const VENDOR: &[u8] = b"Metra";

/// One bounded UTF-8 Vorbis comment for a new FLAC file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlacCreateEntry {
    pub key: String,
    pub value: String,
}

impl FlacCreateEntry {
    /// Create one named Vorbis comment. The key is validated when built.
    pub fn comment(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            value: value.into(),
        }
    }
}

/// Options for creating a minimal metadata-only FLAC stream.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FlacCreateOptions {
    pub comments: Vec<FlacCreateEntry>,
}

impl FlacCreateOptions {
    /// Start with no optional Vorbis comments.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add one Vorbis comment and return the updated options.
    pub fn with_comment(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.comments.push(FlacCreateEntry::comment(key, value));
        self
    }

    /// Add one Vorbis comment in place.
    pub fn push_comment(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.comments.push(FlacCreateEntry::comment(key, value));
    }
}

/// Create a minimal metadata-only FLAC stream in memory.
pub fn create_flac_to_vec(options: &FlacCreateOptions, limits: ParseLimits) -> Result<Vec<u8>> {
    if options.comments.len() > limits.max_jpeg_segments {
        return Err(MetraError::ResourceLimitExceeded {
            resource: "FLAC creation comment entries".to_owned(),
            limit: limits.max_jpeg_segments,
        });
    }

    let mut comments = Vec::with_capacity(options.comments.len());
    let mut vorbis_size = 4_usize
        .checked_add(VENDOR.len())
        .and_then(|size| size.checked_add(4))
        .ok_or_else(|| size_error("FLAC creation Vorbis comments", limits))?;
    for entry in &options.comments {
        let key = validate_key(&entry.key)?;
        if entry.value.contains('\0') {
            return Err(MetraError::InvalidTag {
                context: format!("FLAC creation {}", entry.key),
                message: "Vorbis comment values may not contain NUL bytes".to_owned(),
            });
        }
        if entry.value.len() > limits.max_value_bytes {
            return Err(MetraError::ResourceLimitExceeded {
                resource: format!("FLAC creation value {}", entry.key),
                limit: limits.max_value_bytes,
            });
        }
        if comments
            .iter()
            .any(|existing: &(Vec<u8>, String)| existing.0 == key)
        {
            return Err(MetraError::InvalidTag {
                context: "FLAC creation".to_owned(),
                message: format!("duplicate Vorbis comment key {}", entry.key),
            });
        }
        let comment_length = key
            .len()
            .checked_add(1)
            .and_then(|length| length.checked_add(entry.value.len()))
            .ok_or_else(|| size_error("FLAC creation comment", limits))?;
        u32::try_from(comment_length).map_err(|_| size_error("FLAC creation comment", limits))?;
        vorbis_size = vorbis_size
            .checked_add(4)
            .and_then(|size| size.checked_add(comment_length))
            .ok_or_else(|| size_error("FLAC creation Vorbis comments", limits))?;
        comments.push((key, entry.value.clone()));
    }
    let vorbis_size_u32 =
        u32::try_from(vorbis_size).map_err(|_| MetraError::ResourceLimitExceeded {
            resource: "FLAC creation Vorbis comments".to_owned(),
            limit: limits.max_metadata_bytes,
        })?;
    if vorbis_size_u32 > 0xFF_FFFF {
        return Err(MetraError::ResourceLimitExceeded {
            resource: "FLAC creation Vorbis comments".to_owned(),
            limit: 0xFF_FFFF,
        });
    }
    let total_size = 4_usize
        .checked_add(4 + STREAMINFO_LENGTH)
        .and_then(|size| size.checked_add(4 + vorbis_size))
        .ok_or_else(|| size_error("FLAC creation stream", limits))?;
    if total_size > limits.max_metadata_bytes || total_size > limits.max_value_bytes {
        return Err(MetraError::ResourceLimitExceeded {
            resource: "FLAC creation stream".to_owned(),
            limit: limits.max_metadata_bytes.min(limits.max_value_bytes),
        });
    }

    let mut streaminfo = vec![0_u8; STREAMINFO_LENGTH];
    streaminfo[0..2].copy_from_slice(&4096_u16.to_be_bytes());
    streaminfo[2..4].copy_from_slice(&4096_u16.to_be_bytes());
    let packed = (8_000_u64 << 44) | (7_u64 << 36);
    streaminfo[10..18].copy_from_slice(&packed.to_be_bytes());

    let mut vorbis = Vec::with_capacity(vorbis_size);
    vorbis.extend_from_slice(&(VENDOR.len() as u32).to_le_bytes());
    vorbis.extend_from_slice(VENDOR);
    vorbis.extend_from_slice(&(comments.len() as u32).to_le_bytes());
    for (key, value) in &comments {
        let comment_length = key.len() + 1 + value.len();
        vorbis.extend_from_slice(&(comment_length as u32).to_le_bytes());
        vorbis.extend_from_slice(key);
        vorbis.push(b'=');
        vorbis.extend_from_slice(value.as_bytes());
    }

    let mut output = Vec::with_capacity(total_size);
    output.extend_from_slice(b"fLaC");
    write_block(&mut output, false, 0, &streaminfo)?;
    write_block(&mut output, true, 4, &vorbis)?;
    validate_created_flac(&output, limits)?;
    Ok(output)
}

/// Create a new FLAC path without overwriting an existing file.
pub fn create_flac_path(
    path: impl AsRef<Path>,
    options: &FlacCreateOptions,
    limits: ParseLimits,
) -> Result<()> {
    let path = path.as_ref().to_path_buf();
    if path.exists() {
        return Err(MetraError::WriteFailure {
            message: format!("refusing to overwrite existing FLAC {}", path.display()),
        });
    }
    let bytes = create_flac_to_vec(options, limits)?;
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
        validate_created_flac(&bytes, limits)?;
        if path.exists() {
            return Err(MetraError::WriteFailure {
                message: format!("refusing to overwrite existing FLAC {}", path.display()),
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

fn validate_key(key: &str) -> Result<Vec<u8>> {
    if key.is_empty()
        || !key.is_ascii()
        || key.contains('=')
        || key.contains('\0')
        || key.bytes().any(|byte| byte < 0x20 || byte == 0x7F)
    {
        return Err(MetraError::InvalidTag {
            context: "FLAC creation".to_owned(),
            message: "Vorbis comment keys must be printable ASCII without '='".to_owned(),
        });
    }
    Ok(key.to_ascii_uppercase().into_bytes())
}

fn write_block(output: &mut Vec<u8>, is_last: bool, block_type: u8, data: &[u8]) -> Result<()> {
    let length = u32::try_from(data.len()).map_err(|_| MetraError::ResourceLimitExceeded {
        resource: "FLAC creation block".to_owned(),
        limit: 0xFF_FFFF,
    })?;
    if length > 0xFF_FFFF {
        return Err(MetraError::ResourceLimitExceeded {
            resource: "FLAC creation block".to_owned(),
            limit: 0xFF_FFFF,
        });
    }
    output.push(if is_last {
        0x80 | block_type
    } else {
        block_type
    });
    output.push((length >> 16) as u8);
    output.push((length >> 8) as u8);
    output.push(length as u8);
    output.extend_from_slice(data);
    Ok(())
}

fn validate_created_flac(bytes: &[u8], limits: ParseLimits) -> Result<()> {
    read_flac(
        &mut std::io::Cursor::new(bytes),
        FileInfo::new("created.flac".into(), bytes.len() as u64, FileFormat::Flac),
        limits,
    )?;
    Ok(())
}

fn size_error(resource: &str, limits: ParseLimits) -> MetraError {
    MetraError::ResourceLimitExceeded {
        resource: resource.to_owned(),
        limit: limits.max_metadata_bytes,
    }
}

fn temporary_path(path: &Path) -> Result<PathBuf> {
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("created.flac");
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
    fn creates_readable_flac_with_utf8_comments() {
        let options = FlacCreateOptions::new()
            .with_comment("TITLE", "Metra")
            .with_comment("ARTIST", "Othmane é");
        let bytes = create_flac_to_vec(&options, ParseLimits::default())
            .expect("FLAC creation should succeed");
        let metadata = read_flac(
            &mut std::io::Cursor::new(bytes.clone()),
            FileInfo::new("created.flac".into(), bytes.len() as u64, FileFormat::Flac),
            ParseLimits::default(),
        )
        .expect("created FLAC should remain readable");
        assert_eq!(metadata.file_info.format, FileFormat::Flac);
        assert_eq!(
            metadata.find("FLAC:Title").unwrap().display_value(),
            "Metra"
        );
        assert_eq!(
            metadata.find("FLAC:Artist").unwrap().display_value(),
            "Othmane é"
        );
        assert_eq!(
            metadata.find("FLAC:SampleRateHz").unwrap().display_value(),
            "8000"
        );
        assert!(metadata.warnings.is_empty());
    }

    #[test]
    fn rejects_invalid_duplicate_and_oversized_comments() {
        let invalid = FlacCreateOptions::new().with_comment("BAD=KEY", "value");
        assert!(matches!(
            create_flac_to_vec(&invalid, ParseLimits::default()),
            Err(MetraError::InvalidTag { .. })
        ));
        let duplicate = FlacCreateOptions::new()
            .with_comment("Title", "one")
            .with_comment("TITLE", "two");
        assert!(matches!(
            create_flac_to_vec(&duplicate, ParseLimits::default()),
            Err(MetraError::InvalidTag { .. })
        ));
        let limits = ParseLimits {
            max_value_bytes: 3,
            ..ParseLimits::default()
        };
        let oversized = FlacCreateOptions::new().with_comment("TITLE", "long");
        assert!(matches!(
            create_flac_to_vec(&oversized, limits),
            Err(MetraError::ResourceLimitExceeded { .. })
        ));
    }

    #[test]
    fn creates_new_path_without_overwriting_existing_file() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after Unix epoch")
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("metra-flac-create-{unique}"));
        fs::create_dir(&directory).expect("temporary directory should be created");
        let path = directory.join("created.flac");
        let options = FlacCreateOptions::new().with_comment("TITLE", "Metra");

        create_flac_path(&path, &options, ParseLimits::default())
            .expect("new FLAC path should be created");
        assert!(path.is_file());
        assert!(matches!(
            create_flac_path(&path, &options, ParseLimits::default()),
            Err(MetraError::WriteFailure { .. })
        ));

        fs::remove_file(&path).expect("created FLAC should be removed");
        fs::remove_dir(&directory).expect("temporary directory should be removed");
    }
}
