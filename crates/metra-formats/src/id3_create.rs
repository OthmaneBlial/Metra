use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use metra_core::{FileFormat, FileInfo, MetraError, ParseLimits, Result};

use crate::atomic::atomic_replace;
use crate::id3::read_mp3;
use crate::id3_writer::{
    append_frame, comment_frame_id, encode_comment_payload, encode_text_payload, synchsafe,
    text_frame_id,
};

const ID3_VERSION: u8 = 4;
const ID3_HEADER_LENGTH: usize = 10;
const MPEG_FRAME_LENGTH: usize = 417;
const MPEG_FRAME_HEADER: [u8; 4] = [0xFF, 0xFB, 0x90, 0x64];

/// One bounded ID3v2.4 text field for a new MP3 seed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mp3CreateEntry {
    pub name: String,
    pub value: String,
}

impl Mp3CreateEntry {
    /// Create one canonical ID3 text field. The name is validated when encoded.
    pub fn text(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
        }
    }
}

/// Options for creating a minimal MP3 containing an ID3v2.4 tag and one
/// zeroed MPEG Layer III frame. The audio payload is intentionally minimal;
/// callers should treat this as a metadata seed, not an audio encoder.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Mp3CreateOptions {
    pub texts: Vec<Mp3CreateEntry>,
    pub comment: Option<String>,
}

impl Mp3CreateOptions {
    /// Start with an empty ID3v2.4 tag.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add one canonical ID3 text field and return the updated options.
    pub fn with_text(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.texts.push(Mp3CreateEntry::text(name, value));
        self
    }

    /// Add one canonical ID3 text field in place.
    pub fn push_text(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.texts.push(Mp3CreateEntry::text(name, value));
    }

    /// Set the English ID3 comment and return the updated options.
    pub fn with_comment(mut self, value: impl Into<String>) -> Self {
        self.comment = Some(value.into());
        self
    }

    /// Set the English ID3 comment in place.
    pub fn set_comment(&mut self, value: impl Into<String>) {
        self.comment = Some(value.into());
    }
}

/// Create a minimal MP3 seed in memory.
pub fn create_mp3_to_vec(options: &Mp3CreateOptions, limits: ParseLimits) -> Result<Vec<u8>> {
    let entry_count = options
        .texts
        .len()
        .checked_add(usize::from(options.comment.is_some()))
        .ok_or_else(|| size_error("MP3 creation entries"))?;
    if entry_count > limits.max_jpeg_segments {
        return Err(MetraError::ResourceLimitExceeded {
            resource: "MP3 creation ID3 frames".to_owned(),
            limit: limits.max_jpeg_segments,
        });
    }

    let mut body = Vec::new();
    let mut names: Vec<String> = Vec::with_capacity(entry_count);
    for entry in &options.texts {
        let name = entry.name.trim().to_owned();
        let frame_id = text_frame_id(&name, ID3_VERSION).ok_or_else(|| MetraError::InvalidTag {
            context: format!("MP3 creation {}", entry.name),
            message: "unsupported ID3v2.4 text field".to_owned(),
        })?;
        if names.contains(&name) {
            return Err(MetraError::InvalidTag {
                context: "MP3 creation".to_owned(),
                message: format!("duplicate ID3 field {name}"),
            });
        }
        names.push(name);
        let payload = encode_text_payload(ID3_VERSION, &entry.value, limits)?;
        append_frame(&mut body, ID3_VERSION, &frame_id, [0, 0], &payload)?;
    }
    if let Some(comment) = options.comment.as_deref() {
        if names.iter().any(|name| name == "Comment") {
            return Err(MetraError::InvalidTag {
                context: "MP3 creation".to_owned(),
                message: "duplicate ID3 field Comment".to_owned(),
            });
        }
        let payload = encode_comment_payload(ID3_VERSION, comment, limits)?;
        append_frame(
            &mut body,
            ID3_VERSION,
            &comment_frame_id(ID3_VERSION),
            [0, 0],
            &payload,
        )?;
    }
    if body.len() > limits.max_metadata_bytes {
        return Err(MetraError::ResourceLimitExceeded {
            resource: "MP3 creation ID3 tag".to_owned(),
            limit: limits.max_metadata_bytes,
        });
    }

    let tag_size = synchsafe(body.len())?;
    let total_size = ID3_HEADER_LENGTH
        .checked_add(body.len())
        .and_then(|size| size.checked_add(MPEG_FRAME_LENGTH))
        .ok_or_else(|| size_error("MP3 creation file"))?;
    if total_size > limits.max_metadata_bytes.saturating_add(MPEG_FRAME_LENGTH) {
        return Err(MetraError::ResourceLimitExceeded {
            resource: "MP3 creation file".to_owned(),
            limit: limits.max_metadata_bytes.saturating_add(MPEG_FRAME_LENGTH),
        });
    }

    let mut output = Vec::with_capacity(total_size);
    output.extend_from_slice(b"ID3");
    output.extend_from_slice(&[ID3_VERSION, 0, 0]);
    output.extend_from_slice(&tag_size);
    output.extend_from_slice(&body);
    output.extend_from_slice(&MPEG_FRAME_HEADER);
    output.resize(total_size, 0);
    validate_created_mp3(&output, limits)?;
    Ok(output)
}

/// Create a new MP3 seed without overwriting an existing path.
pub fn create_mp3_path(
    path: impl AsRef<Path>,
    options: &Mp3CreateOptions,
    limits: ParseLimits,
) -> Result<()> {
    let path = path.as_ref().to_path_buf();
    if path.exists() {
        return Err(MetraError::WriteFailure {
            message: format!("refusing to overwrite existing MP3 {}", path.display()),
        });
    }
    let bytes = create_mp3_to_vec(options, limits)?;
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
        validate_created_mp3(&bytes, limits)?;
        if path.exists() {
            return Err(MetraError::WriteFailure {
                message: format!("refusing to overwrite existing MP3 {}", path.display()),
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

fn validate_created_mp3(bytes: &[u8], limits: ParseLimits) -> Result<()> {
    read_mp3(
        &mut std::io::Cursor::new(bytes),
        FileInfo::new("created.mp3".into(), bytes.len() as u64, FileFormat::Mp3),
        limits,
    )?;
    Ok(())
}

fn size_error(resource: &str) -> MetraError {
    MetraError::ResourceLimitExceeded {
        resource: resource.to_owned(),
        limit: usize::MAX,
    }
}

fn write_error(path: &Path, source: std::io::Error) -> MetraError {
    MetraError::WriteFailure {
        message: format!("cannot write {}: {source}", path.display()),
    }
}

fn temporary_path(path: &Path) -> Result<PathBuf> {
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("metadata.mp3");
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

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    #[test]
    fn creates_id3v24_text_comment_and_mpeg_seed() {
        let options = Mp3CreateOptions::new()
            .with_text("Title", "Metra")
            .with_text("Artist", "Othmane")
            .with_comment("reviewed");
        let bytes = create_mp3_to_vec(&options, ParseLimits::default()).unwrap();
        assert_eq!(&bytes[..3], b"ID3");
        assert_eq!(
            &bytes[bytes.len() - MPEG_FRAME_LENGTH..bytes.len() - MPEG_FRAME_LENGTH + 4],
            &MPEG_FRAME_HEADER
        );
        let metadata = read_mp3(
            &mut Cursor::new(bytes.clone()),
            FileInfo::new("created.mp3".into(), bytes.len() as u64, FileFormat::Mp3),
            ParseLimits::default(),
        )
        .unwrap();
        assert_eq!(metadata.find("ID3:Title").unwrap().display_value(), "Metra");
        assert_eq!(
            metadata.find("ID3:Artist").unwrap().display_value(),
            "Othmane"
        );
        assert_eq!(
            metadata.find("ID3:Comment").unwrap().display_value(),
            "reviewed"
        );
        assert_eq!(
            metadata.find("ID3:SampleRateHz").unwrap().display_value(),
            "44100"
        );
    }

    #[test]
    fn rejects_duplicate_unsupported_and_nul_values() {
        let duplicate = Mp3CreateOptions::new()
            .with_text("Title", "one")
            .with_text("Title", "two");
        assert!(create_mp3_to_vec(&duplicate, ParseLimits::default()).is_err());

        let unsupported = Mp3CreateOptions::new().with_text("NoSuchField", "value");
        assert!(create_mp3_to_vec(&unsupported, ParseLimits::default()).is_err());

        let nul = Mp3CreateOptions::new().with_text("Title", "bad\0value");
        assert!(create_mp3_to_vec(&nul, ParseLimits::default()).is_err());
    }

    #[test]
    fn path_creation_refuses_overwrite() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("metra-mp3-create-{unique}"));
        fs::create_dir(&directory).unwrap();
        let path = directory.join("seed.mp3");
        create_mp3_path(
            &path,
            &Mp3CreateOptions::new().with_text("Title", "created"),
            ParseLimits::default(),
        )
        .unwrap();
        assert!(create_mp3_path(&path, &Mp3CreateOptions::new(), ParseLimits::default()).is_err());
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        fs::remove_dir_all(directory).unwrap();
    }
}
