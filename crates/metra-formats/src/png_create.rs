use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use flate2::{Compression, write::ZlibEncoder};
use metra_core::{FileFormat, FileInfo, MetraError, ParseLimits, Result};

use crate::atomic::atomic_replace;
use crate::png::{PNG_SIGNATURE, crc32, read_png};

/// One bounded PNG `tEXt` keyword/value pair for a new image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PngCreateEntry {
    pub keyword: String,
    pub value: String,
}

impl PngCreateEntry {
    /// Create one text entry. The keyword is validated when the PNG is built.
    pub fn text(keyword: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            keyword: keyword.into(),
            value: value.into(),
        }
    }
}

/// Options for creating a minimal 1x1 RGBA PNG with bounded text metadata.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PngCreateOptions {
    pub text: Vec<PngCreateEntry>,
}

impl PngCreateOptions {
    /// Start with no optional text metadata.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add one PNG `tEXt` entry and return the updated options.
    pub fn with_text(mut self, keyword: impl Into<String>, value: impl Into<String>) -> Self {
        self.text.push(PngCreateEntry::text(keyword, value));
        self
    }

    /// Add one PNG `tEXt` entry in place.
    pub fn push_text(&mut self, keyword: impl Into<String>, value: impl Into<String>) {
        self.text.push(PngCreateEntry::text(keyword, value));
    }
}

/// Create a minimal 1x1 RGBA PNG in memory.
pub fn create_png_to_vec(options: &PngCreateOptions, limits: ParseLimits) -> Result<Vec<u8>> {
    let mut text_entries = Vec::with_capacity(options.text.len());
    for entry in &options.text {
        validate_keyword(&entry.keyword)?;
        if entry.value.as_bytes().contains(&0) {
            return Err(MetraError::InvalidTag {
                context: format!("PNG creation {}", entry.keyword),
                message: "tEXt values may not contain NUL bytes".to_owned(),
            });
        }
        if entry.value.len() > limits.max_value_bytes {
            return Err(MetraError::ResourceLimitExceeded {
                resource: format!("PNG creation value {}", entry.keyword),
                limit: limits.max_value_bytes,
            });
        }
        if text_entries
            .iter()
            .any(|existing: &(String, String)| existing.0 == entry.keyword)
        {
            return Err(MetraError::InvalidTag {
                context: "PNG creation".to_owned(),
                message: format!("duplicate tEXt keyword {}", entry.keyword),
            });
        }
        text_entries.push((entry.keyword.clone(), entry.value.clone()));
    }

    let mut output = PNG_SIGNATURE.to_vec();
    write_chunk(
        &mut output,
        b"IHDR",
        &[0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0],
    )?;
    for (keyword, value) in &text_entries {
        let mut data = Vec::with_capacity(keyword.len() + 1 + value.len());
        data.extend_from_slice(keyword.as_bytes());
        data.push(0);
        data.extend_from_slice(value.as_bytes());
        write_chunk(&mut output, b"tEXt", &data)?;
    }

    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(&[0, 0, 0, 0, 0])
        .map_err(|source| write_error("PNG creation compression", source))?;
    let compressed = encoder
        .finish()
        .map_err(|source| write_error("PNG creation compression", source))?;
    write_chunk(&mut output, b"IDAT", &compressed)?;
    write_chunk(&mut output, b"IEND", &[])?;

    if output.len() > limits.max_metadata_bytes {
        return Err(MetraError::ResourceLimitExceeded {
            resource: "PNG creation metadata".to_owned(),
            limit: limits.max_metadata_bytes,
        });
    }
    validate_created_png(&output, limits)?;
    Ok(output)
}

/// Create a new PNG path without overwriting an existing destination.
pub fn create_png_path(
    path: impl AsRef<Path>,
    options: &PngCreateOptions,
    limits: ParseLimits,
) -> Result<()> {
    let path = path.as_ref().to_path_buf();
    if path.exists() {
        return Err(MetraError::WriteFailure {
            message: format!("refusing to overwrite existing PNG {}", path.display()),
        });
    }
    let bytes = create_png_to_vec(options, limits)?;
    let temp_path = temporary_path(&path)?;
    let result = (|| {
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
            .map_err(|source| write_error(&temp_path.display().to_string(), source))?;
        output
            .write_all(&bytes)
            .map_err(|source| write_error(&temp_path.display().to_string(), source))?;
        output
            .sync_all()
            .map_err(|source| write_error(&temp_path.display().to_string(), source))?;
        drop(output);
        validate_created_png(&bytes, limits)?;
        if path.exists() {
            return Err(MetraError::WriteFailure {
                message: format!("refusing to overwrite existing PNG {}", path.display()),
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

fn validate_created_png(bytes: &[u8], limits: ParseLimits) -> Result<()> {
    read_png(
        &mut std::io::Cursor::new(bytes),
        FileInfo::new("created.png".into(), bytes.len() as u64, FileFormat::Png),
        limits,
    )?;
    Ok(())
}

fn validate_keyword(keyword: &str) -> Result<()> {
    let bytes = keyword.as_bytes();
    if bytes.is_empty()
        || bytes.len() > 79
        || !bytes.iter().all(|byte| (0x20..=0x7E).contains(byte))
    {
        return Err(MetraError::InvalidTag {
            context: "PNG creation keyword".to_owned(),
            message: "tEXt keywords must contain 1-79 printable ASCII bytes".to_owned(),
        });
    }
    Ok(())
}

fn write_chunk(output: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) -> Result<()> {
    let length = u32::try_from(data.len()).map_err(|_| MetraError::ResourceLimitExceeded {
        resource: "PNG creation chunk".to_owned(),
        limit: u32::MAX as usize,
    })?;
    output.extend_from_slice(&length.to_be_bytes());
    output.extend_from_slice(kind);
    output.extend_from_slice(data);
    output.extend_from_slice(&crc32(kind, data).to_be_bytes());
    Ok(())
}

fn temporary_path(path: &Path) -> Result<PathBuf> {
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("created.png");
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

fn write_error(context: &str, source: std::io::Error) -> MetraError {
    MetraError::WriteFailure {
        message: format!("{context}: {source}"),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;
    use metra_core::TagValue;

    #[test]
    fn creates_readable_one_pixel_png_with_text_metadata() {
        let options = PngCreateOptions::new()
            .with_text("Comment", "Metra")
            .with_text("Author", "Othmane");
        let bytes = create_png_to_vec(&options, ParseLimits::default())
            .expect("minimal PNG creation should succeed");
        let metadata = read_png(
            &mut std::io::Cursor::new(bytes.clone()),
            FileInfo::new("created.png".into(), bytes.len() as u64, FileFormat::Png),
            ParseLimits::default(),
        )
        .expect("created PNG should remain readable");
        assert_eq!(
            metadata.find("PNG:Text:Comment").unwrap().value,
            TagValue::String("Metra".into())
        );
        assert_eq!(
            metadata.find("PNG:Text:Author").unwrap().display_value(),
            "Othmane"
        );
        assert_eq!(
            metadata.find("PNG:ImageWidth").unwrap().display_value(),
            "1"
        );
    }

    #[test]
    fn rejects_invalid_duplicate_and_oversized_text() {
        let invalid = PngCreateOptions::new().with_text("bad\0key", "value");
        assert!(matches!(
            create_png_to_vec(&invalid, ParseLimits::default()),
            Err(MetraError::InvalidTag { .. })
        ));

        let duplicate = PngCreateOptions::new()
            .with_text("Comment", "one")
            .with_text("Comment", "two");
        assert!(matches!(
            create_png_to_vec(&duplicate, ParseLimits::default()),
            Err(MetraError::InvalidTag { .. })
        ));

        let oversized = PngCreateOptions::new().with_text("Comment", "value");
        let limits = ParseLimits {
            max_value_bytes: 4,
            ..ParseLimits::default()
        };
        assert!(matches!(
            create_png_to_vec(&oversized, limits),
            Err(MetraError::ResourceLimitExceeded { .. })
        ));
    }

    #[test]
    fn creates_new_path_without_overwriting_existing_file() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after Unix epoch")
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("metra-png-create-{unique}"));
        fs::create_dir(&directory).expect("temporary directory should be created");
        let path = directory.join("created.png");
        let options = PngCreateOptions::new().with_text("Software", "Metra");

        create_png_path(&path, &options, ParseLimits::default())
            .expect("new PNG path should be created");
        assert!(path.is_file());
        assert!(matches!(
            create_png_path(&path, &options, ParseLimits::default()),
            Err(MetraError::WriteFailure { .. })
        ));

        fs::remove_file(&path).expect("created PNG should be removed");
        fs::remove_dir(&directory).expect("temporary directory should be removed");
    }
}
