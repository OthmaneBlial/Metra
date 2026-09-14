use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use metra_core::{FileFormat, FileInfo, MetraError, ParseLimits, Result};

use crate::atomic::atomic_replace;
use crate::gif::read_gif;

/// One bounded comment extension for a new GIF file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GifCreateEntry {
    pub value: String,
}

impl GifCreateEntry {
    /// Create one GIF comment extension. The value is validated when built.
    pub fn comment(value: impl Into<String>) -> Self {
        Self {
            value: value.into(),
        }
    }
}

/// Options for creating a minimal 1x1 GIF with comment extensions.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GifCreateOptions {
    pub comments: Vec<GifCreateEntry>,
}

impl GifCreateOptions {
    /// Start with no optional comment extensions.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add one comment and return the updated options.
    pub fn with_comment(mut self, value: impl Into<String>) -> Self {
        self.comments.push(GifCreateEntry::comment(value));
        self
    }

    /// Add one comment in place.
    pub fn push_comment(&mut self, value: impl Into<String>) {
        self.comments.push(GifCreateEntry::comment(value));
    }
}

/// Create a minimal valid 1x1 GIF in memory.
pub fn create_gif_to_vec(options: &GifCreateOptions, limits: ParseLimits) -> Result<Vec<u8>> {
    if options.comments.len() > limits.max_jpeg_segments {
        return Err(MetraError::ResourceLimitExceeded {
            resource: "GIF creation comment entries".to_owned(),
            limit: limits.max_jpeg_segments,
        });
    }
    for entry in &options.comments {
        if entry.value.as_bytes().contains(&0) {
            return Err(MetraError::InvalidTag {
                context: "GIF creation comment".to_owned(),
                message: "GIF comment values may not contain NUL bytes".to_owned(),
            });
        }
        if entry.value.len() > limits.max_value_bytes {
            return Err(MetraError::ResourceLimitExceeded {
                resource: "GIF creation comment value".to_owned(),
                limit: limits.max_value_bytes,
            });
        }
    }

    let mut output = Vec::with_capacity(43);
    output.extend_from_slice(b"GIF89a");
    output.extend_from_slice(&[1, 0, 1, 0, 0x80, 0, 0]);
    output.extend_from_slice(&[0, 0, 0, 0xFF, 0xFF, 0xFF]);
    for entry in &options.comments {
        output.extend_from_slice(&[0x21, 0xFE]);
        for chunk in entry.value.as_bytes().chunks(usize::from(u8::MAX)) {
            output.push(chunk.len() as u8);
            output.extend_from_slice(chunk);
        }
        output.push(0);
    }
    output.extend_from_slice(&[
        0x2C, 0, 0, 0, 0, 1, 0, 1, 0, 0, // image descriptor
        2, // LZW minimum code size
        2, 0x44, 0x01, 0,    // one-pixel image data and terminator
        0x3B, // trailer
    ]);
    if output.len() > limits.max_metadata_bytes || output.len() > limits.max_value_bytes {
        return Err(MetraError::ResourceLimitExceeded {
            resource: "GIF creation image".to_owned(),
            limit: limits.max_metadata_bytes.min(limits.max_value_bytes),
        });
    }
    validate_created_gif(&output, limits)?;
    Ok(output)
}

/// Create a new GIF path without overwriting an existing file.
pub fn create_gif_path(
    path: impl AsRef<Path>,
    options: &GifCreateOptions,
    limits: ParseLimits,
) -> Result<()> {
    let path = path.as_ref().to_path_buf();
    if path.exists() {
        return Err(MetraError::WriteFailure {
            message: format!("refusing to overwrite existing GIF {}", path.display()),
        });
    }
    let bytes = create_gif_to_vec(options, limits)?;
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
        validate_created_gif(&bytes, limits)?;
        if path.exists() {
            return Err(MetraError::WriteFailure {
                message: format!("refusing to overwrite existing GIF {}", path.display()),
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

fn validate_created_gif(bytes: &[u8], limits: ParseLimits) -> Result<()> {
    read_gif(
        &mut std::io::Cursor::new(bytes),
        FileInfo::new("created.gif".into(), bytes.len() as u64, FileFormat::Gif),
        limits,
    )?;
    Ok(())
}

fn temporary_path(path: &Path) -> Result<PathBuf> {
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("created.gif");
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
    fn creates_readable_one_pixel_gif_with_comments() {
        let options = GifCreateOptions::new()
            .with_comment("Metra")
            .with_comment("Othmane");
        let bytes = create_gif_to_vec(&options, ParseLimits::default())
            .expect("GIF creation should succeed");
        let metadata = read_gif(
            &mut std::io::Cursor::new(bytes.clone()),
            FileInfo::new("created.gif".into(), bytes.len() as u64, FileFormat::Gif),
            ParseLimits::default(),
        )
        .expect("created GIF should remain readable");
        assert_eq!(metadata.file_info.format, FileFormat::Gif);
        assert_eq!(metadata.find_all("GIF:Comment").len(), 2);
        assert_eq!(metadata.find_all("GIF:Comment")[0].display_value(), "Metra");
        assert_eq!(
            metadata.find("GIF:ImageWidth").unwrap().display_value(),
            "1"
        );
        assert_eq!(
            metadata.find("GIF:ImageHeight").unwrap().display_value(),
            "1"
        );
        assert!(metadata.warnings.is_empty());
    }

    #[test]
    fn rejects_invalid_and_oversized_comments() {
        let invalid = GifCreateOptions::new().with_comment("bad\0comment");
        assert!(matches!(
            create_gif_to_vec(&invalid, ParseLimits::default()),
            Err(MetraError::InvalidTag { .. })
        ));
        let limits = ParseLimits {
            max_value_bytes: 3,
            ..ParseLimits::default()
        };
        let oversized = GifCreateOptions::new().with_comment("long");
        assert!(matches!(
            create_gif_to_vec(&oversized, limits),
            Err(MetraError::ResourceLimitExceeded { .. })
        ));
    }

    #[test]
    fn creates_new_path_without_overwriting_existing_file() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after Unix epoch")
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("metra-gif-create-{unique}"));
        fs::create_dir(&directory).expect("temporary directory should be created");
        let path = directory.join("created.gif");
        let options = GifCreateOptions::new().with_comment("Metra");

        create_gif_path(&path, &options, ParseLimits::default())
            .expect("new GIF path should be created");
        assert!(path.is_file());
        assert!(matches!(
            create_gif_path(&path, &options, ParseLimits::default()),
            Err(MetraError::WriteFailure { .. })
        ));

        fs::remove_file(&path).expect("created GIF should be removed");
        fs::remove_dir(&directory).expect("temporary directory should be removed");
    }
}
