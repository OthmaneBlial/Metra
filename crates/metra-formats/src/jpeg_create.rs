use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use metra_core::{FileFormat, FileInfo, Metadata, MetraError, ParseLimits, Result};

use crate::atomic::atomic_replace;
use crate::jpeg::read_jpeg;
use crate::xmp::parse_xmp;

const XMP_PREFIX: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";

/// Options for creating a minimal JPEG metadata container seed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JpegCreateOptions {
    pub comment: Option<String>,
    pub xmp: Option<String>,
}

impl JpegCreateOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_comment(mut self, comment: impl Into<String>) -> Self {
        self.comment = Some(comment.into());
        self
    }

    pub fn with_xmp(mut self, packet: impl Into<String>) -> Self {
        self.xmp = Some(packet.into());
        self
    }

    pub fn set_comment(&mut self, comment: impl Into<String>) {
        self.comment = Some(comment.into());
    }

    pub fn set_xmp(&mut self, packet: impl Into<String>) {
        self.xmp = Some(packet.into());
    }
}

/// Create a minimal SOI/metadata/EOI JPEG container seed in memory.
pub fn create_jpeg_to_vec(options: &JpegCreateOptions, limits: ParseLimits) -> Result<Vec<u8>> {
    let comment = options.comment.as_deref().map(|value| {
        validate_text(value, "JPEG creation comment", limits)?;
        segment(b"\xff\xfe", value.as_bytes())
    });
    let xmp = options.xmp.as_deref().map(|packet| {
        if packet.len() > limits.max_value_bytes {
            return Err(MetraError::ResourceLimitExceeded {
                resource: "JPEG creation XMP packet".to_owned(),
                limit: limits.max_value_bytes,
            });
        }
        let mut metadata = Metadata::new(FileInfo::new(
            "created.jpg".into(),
            packet.len() as u64,
            FileFormat::Xmp,
        ));
        parse_xmp(
            packet.as_bytes(),
            0,
            "JPEG/creation-XMP",
            &mut metadata,
            limits,
        )?;
        let mut payload = XMP_PREFIX.to_vec();
        payload.extend_from_slice(packet.as_bytes());
        segment(b"\xff\xe1", &payload)
    });

    let mut output = Vec::with_capacity(4 + limits.max_value_bytes.min(1024));
    output.extend_from_slice(b"\xff\xd8");
    if let Some(comment) = comment {
        output.extend_from_slice(&comment?);
    }
    if let Some(xmp) = xmp {
        output.extend_from_slice(&xmp?);
    }
    output.extend_from_slice(b"\xff\xd9");
    if output.len() > limits.max_metadata_bytes {
        return Err(MetraError::ResourceLimitExceeded {
            resource: "JPEG creation file".to_owned(),
            limit: limits.max_metadata_bytes,
        });
    }
    validate_created_jpeg(&output, limits)?;
    Ok(output)
}

/// Create a new JPEG metadata seed without overwriting an existing path.
pub fn create_jpeg_path(
    path: impl AsRef<Path>,
    options: &JpegCreateOptions,
    limits: ParseLimits,
) -> Result<()> {
    let path = path.as_ref().to_path_buf();
    if path.exists() {
        return Err(MetraError::WriteFailure {
            message: format!("refusing to overwrite existing JPEG {}", path.display()),
        });
    }
    let bytes = create_jpeg_to_vec(options, limits)?;
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
        validate_created_jpeg(&bytes, limits)?;
        if path.exists() {
            return Err(MetraError::WriteFailure {
                message: format!("refusing to overwrite existing JPEG {}", path.display()),
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

fn validate_text(value: &str, resource: &str, limits: ParseLimits) -> Result<()> {
    if value.as_bytes().contains(&0) {
        return Err(MetraError::InvalidTag {
            context: resource.to_owned(),
            message: "NUL bytes are not allowed".to_owned(),
        });
    }
    if value.len() > limits.max_value_bytes {
        return Err(MetraError::ResourceLimitExceeded {
            resource: resource.to_owned(),
            limit: limits.max_value_bytes,
        });
    }
    Ok(())
}

fn segment(marker: &[u8; 2], payload: &[u8]) -> Result<Vec<u8>> {
    let length = payload
        .len()
        .checked_add(2)
        .ok_or_else(|| size_error("JPEG creation segment"))?;
    let length = u16::try_from(length).map_err(|_| size_error("JPEG creation segment"))?;
    let mut output = Vec::with_capacity(4 + payload.len());
    output.extend_from_slice(marker);
    output.extend_from_slice(&length.to_be_bytes());
    output.extend_from_slice(payload);
    Ok(output)
}

fn validate_created_jpeg(bytes: &[u8], limits: ParseLimits) -> Result<()> {
    read_jpeg(
        &mut std::io::Cursor::new(bytes),
        FileInfo::new("created.jpg".into(), bytes.len() as u64, FileFormat::Jpeg),
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
        .unwrap_or("metadata.jpg");
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
    fn creates_readable_jpeg_container_with_metadata() {
        let packet = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF/></x:xmpmeta>";
        let bytes = create_jpeg_to_vec(
            &JpegCreateOptions::new()
                .with_comment("Metra")
                .with_xmp(packet),
            ParseLimits::default(),
        )
        .unwrap();
        let metadata = read_jpeg(
            &mut Cursor::new(bytes.clone()),
            FileInfo::new("created.jpg".into(), bytes.len() as u64, FileFormat::Jpeg),
            ParseLimits::default(),
        )
        .unwrap();
        assert_eq!(
            metadata.find("JPEG:Comment").unwrap().display_value(),
            "Metra"
        );
        assert!(metadata.find("XMP:Packet").is_some());
    }

    #[test]
    fn rejects_invalid_and_oversized_jpeg_metadata() {
        let invalid = JpegCreateOptions::new().with_comment("bad\0comment");
        assert!(create_jpeg_to_vec(&invalid, ParseLimits::default()).is_err());
        let limits = ParseLimits {
            max_metadata_bytes: 3,
            ..ParseLimits::default()
        };
        assert!(create_jpeg_to_vec(&JpegCreateOptions::new(), limits).is_err());
    }

    #[test]
    fn path_creation_refuses_overwrite() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("metra-jpeg-create-{unique}"));
        fs::create_dir(&directory).unwrap();
        let path = directory.join("seed.jpg");
        create_jpeg_path(&path, &JpegCreateOptions::new(), ParseLimits::default()).unwrap();
        assert!(
            create_jpeg_path(&path, &JpegCreateOptions::new(), ParseLimits::default()).is_err()
        );
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        fs::remove_dir_all(directory).unwrap();
    }
}
