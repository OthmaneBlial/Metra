use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use metra_core::{FileFormat, FileInfo, MetraError, ParseLimits, Result};

use crate::atomic::atomic_replace;
use crate::matroska::read_matroska;

/// The EBML document type emitted by the bounded Matroska/WebM creator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatroskaCreateKind {
    Mkv,
    Webm,
}

impl MatroskaCreateKind {
    fn file_format(self) -> FileFormat {
        match self {
            Self::Mkv => FileFormat::Mkv,
            Self::Webm => FileFormat::Webm,
        }
    }

    fn document_type(self) -> &'static [u8] {
        match self {
            Self::Mkv => b"matroska",
            Self::Webm => b"webm",
        }
    }
}

/// One bounded text entry for a new Matroska/WebM metadata seed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatroskaCreateEntry {
    pub name: String,
    pub value: String,
}

impl MatroskaCreateEntry {
    /// Create one named metadata entry.
    pub fn text(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
        }
    }
}

/// Options for creating a bounded Matroska/WebM metadata seed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatroskaCreateOptions {
    pub kind: MatroskaCreateKind,
    pub info: Vec<MatroskaCreateEntry>,
    pub tags: Vec<MatroskaCreateEntry>,
}

impl Default for MatroskaCreateOptions {
    fn default() -> Self {
        Self::new(MatroskaCreateKind::Mkv)
    }
}

impl MatroskaCreateOptions {
    /// Start a seed for the selected EBML document type.
    pub fn new(kind: MatroskaCreateKind) -> Self {
        Self {
            kind,
            info: Vec::new(),
            tags: Vec::new(),
        }
    }

    /// Add an `Info` text field and return the updated options.
    pub fn with_info(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.info.push(MatroskaCreateEntry::text(name, value));
        self
    }

    /// Add a `SimpleTag` text field and return the updated options.
    pub fn with_tag(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.tags.push(MatroskaCreateEntry::text(name, value));
        self
    }

    /// Add an `Info` text field in place.
    pub fn push_info(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.info.push(MatroskaCreateEntry::text(name, value));
    }

    /// Add a `SimpleTag` text field in place.
    pub fn push_tag(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.tags.push(MatroskaCreateEntry::text(name, value));
    }
}

/// Create a bounded Matroska/WebM metadata seed without media clusters.
pub fn create_matroska_to_vec(
    options: &MatroskaCreateOptions,
    limits: ParseLimits,
) -> Result<Vec<u8>> {
    if options.info.len() + options.tags.len() > limits.max_ifd_entries {
        return Err(MetraError::ResourceLimitExceeded {
            resource: "Matroska creation metadata entries".to_owned(),
            limit: limits.max_ifd_entries,
        });
    }
    let mut info_data = element(&[0x2A, 0xD7, 0xB1], &1_000_000_u32.to_be_bytes())?;
    let mut seen_info = Vec::with_capacity(options.info.len());
    for entry in &options.info {
        let id = info_id(&entry.name).ok_or_else(|| MetraError::InvalidTag {
            context: "Matroska creation".to_owned(),
            message: format!("unsupported Info field {}", entry.name),
        })?;
        validate_text(entry, "Info", limits)?;
        if seen_info.contains(&id) {
            return Err(MetraError::InvalidTag {
                context: "Matroska creation".to_owned(),
                message: format!("duplicate Info field {}", entry.name),
            });
        }
        seen_info.push(id);
        info_data.extend_from_slice(&element(&id, entry.value.as_bytes())?);
    }
    let info = element(&[0x15, 0x49, 0xA9, 0x66], &info_data)?;

    let mut tags_data = Vec::new();
    for entry in &options.tags {
        validate_text(entry, "SimpleTag", limits)?;
        if entry.name.is_empty() {
            return Err(MetraError::InvalidTag {
                context: "Matroska creation".to_owned(),
                message: "SimpleTag names may not be empty".to_owned(),
            });
        }
        let simple_tag = [
            element(&[0x45, 0xA3], entry.name.as_bytes())?,
            element(&[0x44, 0x87], entry.value.as_bytes())?,
        ]
        .concat();
        let tag = element(&[0x73, 0x73], &element(&[0x67, 0xC8], &simple_tag)?)?;
        tags_data.extend_from_slice(&tag);
    }
    let tags = if tags_data.is_empty() {
        Vec::new()
    } else {
        element(&[0x12, 0x54, 0xC3, 0x67], &tags_data)?
    };

    let ebml_header_data = [
        element(&[0x42, 0x86], &[1])?,
        element(&[0x42, 0xF7], &[1])?,
        element(&[0x42, 0xF2], &[4])?,
        element(&[0x42, 0xF3], &[8])?,
        element(&[0x42, 0x82], options.kind.document_type())?,
        element(&[0x42, 0x87], &[4])?,
        element(&[0x42, 0x85], &[2])?,
    ]
    .concat();
    let ebml = element(&[0x1A, 0x45, 0xDF, 0xA3], &ebml_header_data)?;
    let segment = element(&[0x18, 0x53, 0x80, 0x67], &[info, tags].concat())?;
    let output = [ebml, segment].concat();
    if output.len() > limits.max_metadata_bytes || output.len() > limits.max_value_bytes {
        return Err(MetraError::ResourceLimitExceeded {
            resource: "Matroska creation output".to_owned(),
            limit: limits.max_metadata_bytes.min(limits.max_value_bytes),
        });
    }
    validate_created_matroska(&output, options.kind, limits)?;
    Ok(output)
}

/// Create a new Matroska/WebM path without overwriting an existing destination.
pub fn create_matroska_path(
    path: impl AsRef<Path>,
    options: &MatroskaCreateOptions,
    limits: ParseLimits,
) -> Result<()> {
    let path = path.as_ref().to_path_buf();
    if path.exists() {
        return Err(MetraError::WriteFailure {
            message: format!("refusing to overwrite {}", path.display()),
        });
    }
    let bytes = create_matroska_to_vec(options, limits)?;
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
        validate_created_matroska(&bytes, options.kind, limits)?;
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

fn info_id(name: &str) -> Option<[u8; 2]> {
    match name {
        "Title" => Some([0x7B, 0xA9]),
        "MuxingApp" => Some([0x4D, 0x80]),
        "WritingApp" => Some([0x57, 0x41]),
        _ => None,
    }
}

fn validate_text(entry: &MatroskaCreateEntry, context: &str, limits: ParseLimits) -> Result<()> {
    if entry.name.contains('\0') || entry.value.contains('\0') {
        return Err(MetraError::InvalidTag {
            context: format!("Matroska creation {context}"),
            message: "names and values may not contain NUL bytes".to_owned(),
        });
    }
    if entry.value.len() >= limits.max_value_bytes {
        return Err(MetraError::ResourceLimitExceeded {
            resource: format!("Matroska creation {context} value"),
            limit: limits.max_value_bytes,
        });
    }
    Ok(())
}

fn element(id: &[u8], data: &[u8]) -> Result<Vec<u8>> {
    let size = encode_size(data.len())?;
    let mut output = id.to_vec();
    output.extend_from_slice(&size);
    output.extend_from_slice(data);
    Ok(output)
}

fn encode_size(size: usize) -> Result<Vec<u8>> {
    let size = u64::try_from(size).map_err(|_| MetraError::ResourceLimitExceeded {
        resource: "EBML element size".to_owned(),
        limit: u64::MAX as usize,
    })?;
    for width in 1..=8 {
        let payload_bits = width * 7;
        let max = if payload_bits == 64 {
            u64::MAX - 1
        } else {
            (1_u64 << payload_bits) - 2
        };
        if size <= max {
            let value = (1_u64 << payload_bits) | size;
            return Ok(value.to_be_bytes()[8 - width..].to_vec());
        }
    }
    Err(MetraError::ResourceLimitExceeded {
        resource: "EBML element size".to_owned(),
        limit: u64::MAX as usize,
    })
}

fn validate_created_matroska(
    bytes: &[u8],
    kind: MatroskaCreateKind,
    limits: ParseLimits,
) -> Result<()> {
    read_matroska(
        &mut std::io::Cursor::new(bytes),
        FileInfo::new("created.mkv".into(), bytes.len() as u64, kind.file_format()),
        limits,
    )?;
    Ok(())
}

fn temporary_path(path: &Path) -> Result<PathBuf> {
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("created.mkv");
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
    fn creates_valid_mkv_seed_with_info_and_simple_tag() {
        let options = MatroskaCreateOptions::new(MatroskaCreateKind::Mkv)
            .with_info("Title", "Metra")
            .with_tag("TITLE", "Metra");
        let bytes = create_matroska_to_vec(&options, ParseLimits::default()).unwrap();
        let metadata = read_matroska(
            &mut std::io::Cursor::new(bytes.clone()),
            FileInfo::new("created.mkv".into(), bytes.len() as u64, FileFormat::Mkv),
            ParseLimits::default(),
        )
        .unwrap();
        assert_eq!(
            metadata.find("Matroska:Title").unwrap().display_value(),
            "Metra"
        );
        assert_eq!(
            metadata.find("Matroska:Tag:TITLE").unwrap().display_value(),
            "Metra"
        );
    }

    #[test]
    fn creates_webm_seed_with_the_webm_document_type() {
        let options =
            MatroskaCreateOptions::new(MatroskaCreateKind::Webm).with_info("WritingApp", "Metra");
        let bytes = create_matroska_to_vec(&options, ParseLimits::default()).unwrap();
        let detected = crate::detect_format(&bytes).unwrap();
        assert_eq!(detected.format, FileFormat::Webm);
    }
}
