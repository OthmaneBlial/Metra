use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use metra_core::{FileFormat, FileInfo, MetraError, ParseLimits, Result};

use crate::atomic::atomic_replace;
use crate::psd::read_psd;
use crate::xmp::parse_xmp;

const PSD_HEADER_LENGTH: usize = 26;

/// Options for creating a minimal 1x1 RGB Photoshop document.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PsdCreateOptions {
    pub xmp: Option<String>,
}

impl PsdCreateOptions {
    /// Start an empty 1x1 RGB PSD seed.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add one validated XMP image resource to the seed.
    pub fn with_xmp(mut self, packet: impl Into<String>) -> Self {
        self.xmp = Some(packet.into());
        self
    }

    /// Set the optional XMP image resource in place.
    pub fn set_xmp(&mut self, packet: impl Into<String>) {
        self.xmp = Some(packet.into());
    }
}

/// Create a bounded, readable 1x1 RGB PSD seed without pixel decoding.
pub fn create_psd_to_vec(options: &PsdCreateOptions, limits: ParseLimits) -> Result<Vec<u8>> {
    let resources = if let Some(packet) = &options.xmp {
        let packet = packet.as_bytes();
        if packet.len() > limits.max_value_bytes {
            return Err(MetraError::ResourceLimitExceeded {
                resource: "PSD XMP packet".to_owned(),
                limit: limits.max_value_bytes,
            });
        }
        let mut metadata = metra_core::Metadata::new(FileInfo::new(
            "<memory>".into(),
            packet.len() as u64,
            FileFormat::Xmp,
        ));
        parse_xmp(packet, 0, "PSD/create", &mut metadata, limits)?;
        resource(0x0424, packet)
    } else {
        Vec::new()
    };
    if resources.len() > limits.max_metadata_bytes {
        return Err(MetraError::ResourceLimitExceeded {
            resource: "PSD image resources".to_owned(),
            limit: limits.max_metadata_bytes,
        });
    }

    let mut output = Vec::with_capacity(PSD_HEADER_LENGTH + 16 + resources.len() + 5);
    output.extend_from_slice(b"8BPS");
    output.extend_from_slice(&1_u16.to_be_bytes());
    output.extend_from_slice(&[0; 6]);
    output.extend_from_slice(&3_u16.to_be_bytes());
    output.extend_from_slice(&1_u32.to_be_bytes());
    output.extend_from_slice(&1_u32.to_be_bytes());
    output.extend_from_slice(&8_u16.to_be_bytes());
    output.extend_from_slice(&3_u16.to_be_bytes());
    output.extend_from_slice(&0_u32.to_be_bytes());
    output.extend_from_slice(&(resources.len() as u32).to_be_bytes());
    output.extend_from_slice(&resources);
    output.extend_from_slice(&0_u32.to_be_bytes());
    output.extend_from_slice(&0_u16.to_be_bytes());
    output.extend_from_slice(&[0, 0, 0]);

    if output.len() > limits.max_metadata_bytes || output.len() > limits.max_value_bytes {
        return Err(MetraError::ResourceLimitExceeded {
            resource: "PSD creation output".to_owned(),
            limit: limits.max_metadata_bytes.min(limits.max_value_bytes),
        });
    }
    validate_created_psd(&output, limits)?;
    Ok(output)
}

/// Create a new PSD path without overwriting an existing destination.
pub fn create_psd_path(
    path: impl AsRef<Path>,
    options: &PsdCreateOptions,
    limits: ParseLimits,
) -> Result<()> {
    let path = path.as_ref().to_path_buf();
    if path.exists() {
        return Err(MetraError::WriteFailure {
            message: format!("refusing to overwrite {}", path.display()),
        });
    }
    let bytes = create_psd_to_vec(options, limits)?;
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
        validate_created_psd(&bytes, limits)?;
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

fn resource(id: u16, payload: &[u8]) -> Vec<u8> {
    let mut output = b"8BIM".to_vec();
    output.extend_from_slice(&id.to_be_bytes());
    output.extend_from_slice(&[0, 0]);
    output.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    output.extend_from_slice(payload);
    if payload.len() % 2 == 1 {
        output.push(0);
    }
    output
}

fn validate_created_psd(bytes: &[u8], limits: ParseLimits) -> Result<()> {
    read_psd(
        &mut std::io::Cursor::new(bytes),
        FileInfo::new("created.psd".into(), bytes.len() as u64, FileFormat::Psd),
        limits,
    )?;
    Ok(())
}

fn temporary_path(path: &Path) -> Result<PathBuf> {
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("created.psd");
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

    const XMP: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF><rdf:Description xmlns:dc="urn:dc" dc:title="Metra"/></rdf:RDF></x:xmpmeta>"#;

    #[test]
    fn creates_readable_psd_with_optional_xmp() {
        let bytes = create_psd_to_vec(
            &PsdCreateOptions::new().with_xmp(XMP),
            ParseLimits::default(),
        )
        .unwrap();
        let metadata = read_psd(
            &mut std::io::Cursor::new(bytes.clone()),
            FileInfo::new("created.psd".into(), bytes.len() as u64, FileFormat::Psd),
            ParseLimits::default(),
        )
        .unwrap();
        assert_eq!(
            metadata.find("PSD:ImageWidth").unwrap().display_value(),
            "1"
        );
        assert_eq!(
            metadata.find("PSD:ImageHeight").unwrap().display_value(),
            "1"
        );
        assert_eq!(
            metadata.find("XMP:dc:title").unwrap().display_value(),
            "Metra"
        );
    }

    #[test]
    fn creates_empty_psd_without_overwriting() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("metra-psd-create-{unique}"));
        fs::create_dir(&directory).unwrap();
        let path = directory.join("created.psd");
        create_psd_path(&path, &PsdCreateOptions::new(), ParseLimits::default()).unwrap();
        assert!(create_psd_path(&path, &PsdCreateOptions::new(), ParseLimits::default()).is_err());
        fs::remove_dir_all(directory).unwrap();
    }
}
