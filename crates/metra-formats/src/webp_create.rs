use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use metra_core::{FileFormat, FileInfo, Metadata, MetraError, ParseLimits, Result};

use crate::atomic::atomic_replace;
use crate::webp::read_webp;
use crate::xmp::parse_xmp;

const RIFF_HEADER_LENGTH: usize = 12;
const VP8L_SEED: &[u8] = &[
    0x2F, 0x00, 0x00, 0x00, 0x00, 0x07, 0x10, 0xF5, 0x8F, 0xFE, 0x07, 0x22, 0xA2, 0xFF, 0x01, 0x00,
];

/// Options for creating a minimal 1x1 lossless WebP metadata seed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WebpCreateOptions {
    pub xmp: Option<String>,
}

impl WebpCreateOptions {
    /// Start with a 1x1 WebP and no optional XMP packet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a validated XMP packet and return the updated options.
    pub fn with_xmp(mut self, packet: impl Into<String>) -> Self {
        self.xmp = Some(packet.into());
        self
    }

    /// Set the XMP packet in place.
    pub fn set_xmp(&mut self, packet: impl Into<String>) {
        self.xmp = Some(packet.into());
    }
}

/// Create a minimal 1x1 lossless WebP metadata seed in memory.
pub fn create_webp_to_vec(options: &WebpCreateOptions, limits: ParseLimits) -> Result<Vec<u8>> {
    let xmp = options.xmp.as_deref().map(|packet| {
        if packet.len() > limits.max_value_bytes {
            return Err(MetraError::ResourceLimitExceeded {
                resource: "WebP creation XMP packet".to_owned(),
                limit: limits.max_value_bytes,
            });
        }
        let mut metadata = Metadata::new(FileInfo::new(
            "created.webp".into(),
            packet.len() as u64,
            FileFormat::Xmp,
        ));
        parse_xmp(
            packet.as_bytes(),
            0,
            "WebP/creation-XMP",
            &mut metadata,
            limits,
        )?;
        Ok(packet.as_bytes().to_vec())
    });
    let xmp = xmp.transpose()?;

    let mut output = Vec::with_capacity(RIFF_HEADER_LENGTH + 8 + VP8L_SEED.len());
    output.extend_from_slice(b"RIFF");
    output.extend_from_slice(&0_u32.to_le_bytes());
    output.extend_from_slice(b"WEBP");
    append_chunk(&mut output, b"VP8L", VP8L_SEED)?;
    if let Some(xmp) = xmp.as_deref() {
        append_chunk(&mut output, b"XMP ", xmp)?;
    }
    let riff_size = output
        .len()
        .checked_sub(8)
        .ok_or_else(|| size_error("WebP creation RIFF"))?;
    let riff_size = u32::try_from(riff_size).map_err(|_| size_error("WebP creation RIFF"))?;
    output[4..8].copy_from_slice(&riff_size.to_le_bytes());
    if output.len() > limits.max_metadata_bytes {
        return Err(MetraError::ResourceLimitExceeded {
            resource: "WebP creation file".to_owned(),
            limit: limits.max_metadata_bytes,
        });
    }
    validate_created_webp(&output, limits)?;
    Ok(output)
}

/// Create a new WebP metadata seed without overwriting an existing path.
pub fn create_webp_path(
    path: impl AsRef<Path>,
    options: &WebpCreateOptions,
    limits: ParseLimits,
) -> Result<()> {
    let path = path.as_ref().to_path_buf();
    if path.exists() {
        return Err(MetraError::WriteFailure {
            message: format!("refusing to overwrite existing WebP {}", path.display()),
        });
    }
    let bytes = create_webp_to_vec(options, limits)?;
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
        validate_created_webp(&bytes, limits)?;
        if path.exists() {
            return Err(MetraError::WriteFailure {
                message: format!("refusing to overwrite existing WebP {}", path.display()),
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

fn append_chunk(output: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) -> Result<()> {
    let length = u32::try_from(data.len()).map_err(|_| size_error("WebP creation chunk"))?;
    output.extend_from_slice(kind);
    output.extend_from_slice(&length.to_le_bytes());
    output.extend_from_slice(data);
    if data.len() & 1 == 1 {
        output.push(0);
    }
    Ok(())
}

fn validate_created_webp(bytes: &[u8], limits: ParseLimits) -> Result<()> {
    read_webp(
        &mut std::io::Cursor::new(bytes),
        FileInfo::new("created.webp".into(), bytes.len() as u64, FileFormat::Webp),
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
        .unwrap_or("metadata.webp");
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
    fn creates_readable_one_pixel_webp_with_xmp() {
        let packet = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF/></x:xmpmeta>";
        let bytes = create_webp_to_vec(
            &WebpCreateOptions::new().with_xmp(packet),
            ParseLimits::default(),
        )
        .unwrap();
        let metadata = read_webp(
            &mut Cursor::new(bytes.clone()),
            FileInfo::new("created.webp".into(), bytes.len() as u64, FileFormat::Webp),
            ParseLimits::default(),
        )
        .unwrap();
        assert_eq!(
            metadata.find("WebP:ImageWidth").unwrap().display_value(),
            "1"
        );
        assert_eq!(
            metadata.find("WebP:ImageHeight").unwrap().display_value(),
            "1"
        );
        assert!(metadata.find("XMP:Packet").is_some());
    }

    #[test]
    fn rejects_invalid_xmp_and_oversized_output() {
        let invalid = WebpCreateOptions::new().with_xmp("<!DOCTYPE bad>");
        assert!(create_webp_to_vec(&invalid, ParseLimits::default()).is_err());
        let limits = ParseLimits {
            max_metadata_bytes: 20,
            ..ParseLimits::default()
        };
        assert!(create_webp_to_vec(&WebpCreateOptions::new(), limits).is_err());
    }

    #[test]
    fn path_creation_refuses_overwrite() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("metra-webp-create-{unique}"));
        fs::create_dir(&directory).unwrap();
        let path = directory.join("seed.webp");
        create_webp_path(&path, &WebpCreateOptions::new(), ParseLimits::default()).unwrap();
        assert!(
            create_webp_path(&path, &WebpCreateOptions::new(), ParseLimits::default()).is_err()
        );
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        fs::remove_dir_all(directory).unwrap();
    }
}
