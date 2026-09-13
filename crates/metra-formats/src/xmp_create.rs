use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use metra_core::{FileFormat, FileInfo, MetraError, ParseLimits, Result};

use crate::atomic::atomic_replace;
use crate::xmp::read_xmp;

/// Create and validate a standalone XMP packet in memory.
pub fn create_xmp_to_vec(packet: impl AsRef<[u8]>, limits: ParseLimits) -> Result<Vec<u8>> {
    let packet = packet.as_ref();
    if packet.is_empty() {
        return Err(MetraError::InvalidXml {
            message: "XMP packet may not be empty".to_owned(),
        });
    }
    if packet.len() > limits.max_metadata_bytes {
        return Err(MetraError::ResourceLimitExceeded {
            resource: "XMP creation packet".to_owned(),
            limit: limits.max_metadata_bytes,
        });
    }
    if packet.len() > limits.max_value_bytes {
        return Err(MetraError::ResourceLimitExceeded {
            resource: "XMP creation packet".to_owned(),
            limit: limits.max_value_bytes,
        });
    }
    let output = packet.to_vec();
    validate_created_xmp(&output, limits)?;
    Ok(output)
}

/// Create a new standalone XMP path without overwriting an existing file.
pub fn create_xmp_path(
    path: impl AsRef<Path>,
    packet: impl AsRef<[u8]>,
    limits: ParseLimits,
) -> Result<()> {
    let path = path.as_ref().to_path_buf();
    if path.exists() {
        return Err(MetraError::WriteFailure {
            message: format!("refusing to overwrite existing XMP {}", path.display()),
        });
    }
    let bytes = create_xmp_to_vec(packet, limits)?;
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
        validate_created_xmp(&bytes, limits)?;
        if path.exists() {
            return Err(MetraError::WriteFailure {
                message: format!("refusing to overwrite existing XMP {}", path.display()),
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

fn validate_created_xmp(bytes: &[u8], limits: ParseLimits) -> Result<()> {
    read_xmp(
        &mut std::io::Cursor::new(bytes),
        FileInfo::new("created.xmp".into(), bytes.len() as u64, FileFormat::Xmp),
        limits,
    )?;
    Ok(())
}

fn temporary_path(path: &Path) -> Result<PathBuf> {
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("created.xmp");
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

    const PACKET: &[u8] = br#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description xmlns:dc="http://purl.org/dc/elements/1.1/" dc:title="Metra"/></rdf:RDF></x:xmpmeta>"#;

    #[test]
    fn creates_and_revalidates_standalone_xmp() {
        let bytes = create_xmp_to_vec(PACKET, ParseLimits::default())
            .expect("standalone XMP creation should succeed");
        let metadata = read_xmp(
            &mut std::io::Cursor::new(bytes.clone()),
            FileInfo::new("created.xmp".into(), bytes.len() as u64, FileFormat::Xmp),
            ParseLimits::default(),
        )
        .expect("created XMP should remain readable");
        assert_eq!(
            metadata.find("XMP:Packet").unwrap().value,
            TagValue::Bytes(bytes)
        );
        assert!(metadata.find("XMP:dc:title").is_some());
    }

    #[test]
    fn rejects_invalid_packets_and_limits() {
        assert!(matches!(
            create_xmp_to_vec(b"<broken", ParseLimits::default()),
            Err(MetraError::InvalidXml { .. })
        ));
        let limits = ParseLimits {
            max_value_bytes: 4,
            ..ParseLimits::default()
        };
        assert!(matches!(
            create_xmp_to_vec(PACKET, limits),
            Err(MetraError::ResourceLimitExceeded { .. })
        ));
    }

    #[test]
    fn creates_new_path_without_overwriting_existing_file() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after Unix epoch")
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("metra-xmp-create-{unique}"));
        fs::create_dir(&directory).expect("temporary directory should be created");
        let path = directory.join("created.xmp");

        create_xmp_path(&path, PACKET, ParseLimits::default())
            .expect("new XMP path should be created");
        assert!(path.is_file());
        assert!(matches!(
            create_xmp_path(&path, PACKET, ParseLimits::default()),
            Err(MetraError::WriteFailure { .. })
        ));

        fs::remove_file(&path).expect("created XMP should be removed");
        fs::remove_dir(&directory).expect("temporary directory should be removed");
    }
}
