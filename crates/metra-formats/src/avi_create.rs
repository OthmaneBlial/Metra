use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use metra_core::{FileFormat, FileInfo, MetraError, ParseLimits, Result};

use crate::atomic::atomic_replace;
use crate::avi::read_avi;

/// One bounded `LIST/INFO` value for a new AVI file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AviCreateEntry {
    pub name: String,
    pub value: String,
}

impl AviCreateEntry {
    /// Create one named AVI INFO field. The name is validated when built.
    pub fn info(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
        }
    }
}

/// Options for creating a bounded 1x1 uncompressed-video AVI seed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AviCreateOptions {
    pub info: Vec<AviCreateEntry>,
}

impl AviCreateOptions {
    /// Start with no optional INFO fields.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add one INFO field and return the updated options.
    pub fn with_info(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.info.push(AviCreateEntry::info(name, value));
        self
    }

    /// Add one INFO field in place.
    pub fn push_info(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.info.push(AviCreateEntry::info(name, value));
    }
}

/// Create a bounded 1x1 uncompressed-video AVI seed with optional INFO text.
///
/// The result contains one 24-bit BGR DIB frame and a small index. It is a
/// deliberately minimal seed rather than a general-purpose video encoder.
pub fn create_avi_to_vec(options: &AviCreateOptions, limits: ParseLimits) -> Result<Vec<u8>> {
    if options.info.len() > limits.max_ifd_entries {
        return Err(MetraError::ResourceLimitExceeded {
            resource: "AVI creation INFO entries".to_owned(),
            limit: limits.max_ifd_entries,
        });
    }
    let mut info_payload = b"INFO".to_vec();
    let mut seen = Vec::with_capacity(options.info.len());
    for entry in &options.info {
        let kind = info_kind(&entry.name).ok_or_else(|| MetraError::InvalidTag {
            context: "AVI creation".to_owned(),
            message: format!("unsupported INFO field {}", entry.name),
        })?;
        if entry.value.as_bytes().contains(&0) {
            return Err(MetraError::InvalidTag {
                context: format!("AVI creation {}", entry.name),
                message: "INFO values may not contain NUL bytes".to_owned(),
            });
        }
        if entry.value.len() >= limits.max_value_bytes {
            return Err(MetraError::ResourceLimitExceeded {
                resource: format!("AVI creation value {}", entry.name),
                limit: limits.max_value_bytes,
            });
        }
        if seen.contains(&kind) {
            return Err(MetraError::InvalidTag {
                context: "AVI creation".to_owned(),
                message: format!("duplicate INFO field {}", entry.name),
            });
        }
        seen.push(kind);
        info_payload.extend_from_slice(&chunk(&kind, entry.value.as_bytes())?);
    }

    let mut avih = vec![0_u8; 56];
    avih[0..4].copy_from_slice(&1_000_000_u32.to_le_bytes());
    avih[4..8].copy_from_slice(&4_u32.to_le_bytes());
    avih[16..20].copy_from_slice(&1_u32.to_le_bytes());
    avih[24..28].copy_from_slice(&1_u32.to_le_bytes());
    avih[28..32].copy_from_slice(&4_u32.to_le_bytes());
    avih[32..36].copy_from_slice(&1_u32.to_le_bytes());
    avih[36..40].copy_from_slice(&1_u32.to_le_bytes());

    let mut strh = vec![0_u8; 56];
    strh[0..4].copy_from_slice(b"vids");
    strh[4..8].copy_from_slice(b"DIB ");
    strh[20..24].copy_from_slice(&1_u32.to_le_bytes());
    strh[24..28].copy_from_slice(&1_u32.to_le_bytes());
    strh[32..36].copy_from_slice(&1_u32.to_le_bytes());
    strh[36..40].copy_from_slice(&4_u32.to_le_bytes());
    strh[40..44].copy_from_slice(&0xFFFF_FFFF_u32.to_le_bytes());
    strh[52..54].copy_from_slice(&1_i16.to_le_bytes());
    strh[54..56].copy_from_slice(&1_i16.to_le_bytes());

    let mut strf = vec![0_u8; 40];
    strf[0..4].copy_from_slice(&40_u32.to_le_bytes());
    strf[4..8].copy_from_slice(&1_i32.to_le_bytes());
    strf[8..12].copy_from_slice(&1_i32.to_le_bytes());
    strf[12..14].copy_from_slice(&1_u16.to_le_bytes());
    strf[14..16].copy_from_slice(&24_u16.to_le_bytes());
    strf[20..24].copy_from_slice(&4_u32.to_le_bytes());

    let strl = list(
        b"strl",
        &[chunk(b"strh", &strh)?, chunk(b"strf", &strf)?].concat(),
    )?;
    let hdrl = list(b"hdrl", &[chunk(b"avih", &avih)?, strl].concat())?;
    let movi = list(b"movi", &chunk(b"00db", &[0, 0, 0, 0])?)?;
    let info = list(b"INFO", &info_payload[4..])?;
    let index = chunk(
        b"idx1",
        &[
            b"00db".as_slice(),
            &0x10_u32.to_le_bytes(),
            &4_u32.to_le_bytes(),
            &4_u32.to_le_bytes(),
        ]
        .concat(),
    )?;
    let body = [hdrl, info, movi, index].concat();
    let riff_size =
        4_usize
            .checked_add(body.len())
            .ok_or_else(|| MetraError::ResourceLimitExceeded {
                resource: "AVI creation output".to_owned(),
                limit: limits.max_metadata_bytes,
            })?;
    let riff_size = u32::try_from(riff_size).map_err(|_| MetraError::ResourceLimitExceeded {
        resource: "AVI creation output".to_owned(),
        limit: limits.max_metadata_bytes,
    })?;
    let mut output = b"RIFF".to_vec();
    output.extend_from_slice(&riff_size.to_le_bytes());
    output.extend_from_slice(b"AVI ");
    output.extend_from_slice(&body);
    if output.len() > limits.max_metadata_bytes {
        return Err(MetraError::ResourceLimitExceeded {
            resource: "AVI creation output".to_owned(),
            limit: limits.max_metadata_bytes,
        });
    }
    validate_created_avi(&output, limits)?;
    Ok(output)
}

/// Create a new AVI path without overwriting an existing destination.
pub fn create_avi_path(
    path: impl AsRef<Path>,
    options: &AviCreateOptions,
    limits: ParseLimits,
) -> Result<()> {
    let path = path.as_ref().to_path_buf();
    if path.exists() {
        return Err(MetraError::WriteFailure {
            message: format!("refusing to overwrite existing AVI {}", path.display()),
        });
    }
    let bytes = create_avi_to_vec(options, limits)?;
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
        validate_created_avi(&bytes, limits)?;
        if path.exists() {
            return Err(MetraError::WriteFailure {
                message: format!("refusing to overwrite existing AVI {}", path.display()),
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

fn info_kind(name: &str) -> Option<[u8; 4]> {
    match name {
        "Title" => Some(*b"INAM"),
        "Artist" => Some(*b"IART"),
        "Comment" => Some(*b"ICMT"),
        "Copyright" => Some(*b"ICOP"),
        "Software" => Some(*b"ISFT"),
        "Genre" => Some(*b"IGNR"),
        "Product" => Some(*b"IPRD"),
        "Keywords" => Some(*b"IKEY"),
        "DateTime" => Some(*b"IDIT"),
        _ => None,
    }
}

fn chunk(kind: &[u8; 4], data: &[u8]) -> Result<Vec<u8>> {
    let length = u32::try_from(data.len()).map_err(|_| MetraError::ResourceLimitExceeded {
        resource: "AVI creation chunk".to_owned(),
        limit: u32::MAX as usize,
    })?;
    let mut output = kind.to_vec();
    output.extend_from_slice(&length.to_le_bytes());
    output.extend_from_slice(data);
    if data.len() % 2 == 1 {
        output.push(0);
    }
    Ok(output)
}

fn list(form: &[u8; 4], data: &[u8]) -> Result<Vec<u8>> {
    let mut payload = form.to_vec();
    payload.extend_from_slice(data);
    chunk(b"LIST", &payload)
}

fn validate_created_avi(bytes: &[u8], limits: ParseLimits) -> Result<()> {
    read_avi(
        &mut std::io::Cursor::new(bytes),
        FileInfo::new("created.avi".into(), bytes.len() as u64, FileFormat::Avi),
        limits,
    )?;
    Ok(())
}

fn temporary_path(path: &Path) -> Result<PathBuf> {
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("created.avi");
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
    fn creates_valid_one_frame_avi_with_info() {
        let bytes = create_avi_to_vec(
            &AviCreateOptions::new()
                .with_info("Title", "Metra")
                .with_info("Software", "Metra"),
            ParseLimits::default(),
        )
        .unwrap();
        let metadata = read_avi(
            &mut std::io::Cursor::new(bytes.clone()),
            FileInfo::new("created.avi".into(), bytes.len() as u64, FileFormat::Avi),
            ParseLimits::default(),
        )
        .unwrap();
        assert_eq!(
            metadata.find("AVI:ImageWidth").unwrap().display_value(),
            "1"
        );
        assert_eq!(
            metadata.find("AVI:ImageHeight").unwrap().display_value(),
            "1"
        );
        assert_eq!(metadata.find("AVI:Title").unwrap().display_value(), "Metra");
        assert_eq!(
            metadata.find("AVI:Software").unwrap().display_value(),
            "Metra"
        );
    }

    #[test]
    fn rejects_duplicate_info_fields() {
        let error = create_avi_to_vec(
            &AviCreateOptions::new()
                .with_info("Title", "one")
                .with_info("Title", "two"),
            ParseLimits::default(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("duplicate"));
    }
}
