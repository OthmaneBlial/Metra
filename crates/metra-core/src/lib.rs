//! Stable, format-independent data structures used by Metra readers and
//! consumers.

use std::collections::BTreeMap;
use std::fmt;
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const SCHEMA_VERSION: u8 = 1;

/// A detected container or file family. More formats can be added without
/// changing the shape of a metadata record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum FileFormat {
    Jpeg,
    Tiff,
    Png,
    Webp,
    Pdf,
    Gif,
    Unknown,
}

impl FileFormat {
    pub const fn mime_type(self) -> Option<&'static str> {
        match self {
            Self::Jpeg => Some("image/jpeg"),
            Self::Tiff => Some("image/tiff"),
            Self::Png => Some("image/png"),
            Self::Webp => Some("image/webp"),
            Self::Pdf => Some("application/pdf"),
            Self::Gif => Some("image/gif"),
            Self::Unknown => None,
        }
    }
}

impl fmt::Display for FileFormat {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Jpeg => "JPEG",
            Self::Tiff => "TIFF",
            Self::Png => "PNG",
            Self::Webp => "WebP",
            Self::Pdf => "PDF",
            Self::Gif => "GIF",
            Self::Unknown => "Unknown",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileInfo {
    pub path: PathBuf,
    pub size: u64,
    pub format: FileFormat,
    pub mime_type: Option<String>,
}

impl FileInfo {
    pub fn new(path: PathBuf, size: u64, format: FileFormat) -> Self {
        Self {
            path,
            size,
            mime_type: format.mime_type().map(str::to_owned),
            format,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Metadata {
    pub schema_version: u8,
    pub file_info: FileInfo,
    pub tags: Vec<Tag>,
    pub warnings: Vec<Warning>,
}

impl Metadata {
    pub fn new(file_info: FileInfo) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            file_info,
            tags: Vec::new(),
            warnings: Vec::new(),
        }
    }

    pub fn add_tag(&mut self, tag: Tag) {
        self.tags.push(tag);
    }

    pub fn add_warning(&mut self, warning: Warning) {
        self.warnings.push(warning);
    }

    pub fn warn(&mut self, code: impl Into<String>, message: impl Into<String>) {
        self.add_warning(Warning::new(code, message));
    }

    pub fn tags(&self) -> &[Tag] {
        &self.tags
    }

    pub fn warnings(&self) -> &[Warning] {
        &self.warnings
    }

    /// Sort output deterministically while retaining duplicate tags from
    /// different IFDs or metadata blocks.
    pub fn sort_tags(&mut self) {
        self.tags.sort_by(|left, right| {
            left.namespace
                .cmp(&right.namespace)
                .then_with(|| left.group.cmp(&right.group))
                .then_with(|| left.id.cmp(&right.id))
                .then_with(|| left.name.cmp(&right.name))
        });
    }

    /// Find the first tag by its stable `NAMESPACE:Name` key.
    pub fn find(&self, key: &str) -> Option<&Tag> {
        self.tags.iter().find(|tag| tag.key() == key)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tag {
    /// Explicit namespace such as `EXIF` or `GPS`.
    pub namespace: String,
    /// Physical/logical group such as `IFD0`, `ExifIFD`, or `GPS`.
    pub group: String,
    /// Underlying format identifier, when the format defines one.
    pub id: Option<u32>,
    /// Stable canonical name, not a localized display label.
    pub name: String,
    pub description: Option<String>,
    /// Raw bytes are retained for bounded lossless inspection and future
    /// writers. Values larger than the configured limit are reported as a
    /// warning and are not materialized.
    pub raw_value: Option<Vec<u8>>,
    pub value: TagValue,
    pub value_type: ValueType,
    pub source: Source,
    pub writable: bool,
}

impl Tag {
    pub fn key(&self) -> String {
        format!("{}:{}", self.namespace, self.name)
    }

    pub fn display_value(&self) -> String {
        self.value.to_display_string()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TagValue {
    String(String),
    Unsigned(u64),
    Signed(i64),
    Float(f64),
    Rational { numerator: i64, denominator: i64 },
    UnsignedRational { numerator: u64, denominator: u64 },
    Bytes(Vec<u8>),
    Array(Vec<TagValue>),
    Structure(BTreeMap<String, TagValue>),
    Unknown { type_id: u16, bytes: Vec<u8> },
}

impl TagValue {
    pub fn to_display_string(&self) -> String {
        match self {
            Self::String(value) => value.clone(),
            Self::Unsigned(value) => value.to_string(),
            Self::Signed(value) => value.to_string(),
            Self::Float(value) => value.to_string(),
            Self::Rational {
                numerator,
                denominator,
            } => format!("{numerator}/{denominator}"),
            Self::UnsignedRational {
                numerator,
                denominator,
            } => format!("{numerator}/{denominator}"),
            Self::Bytes(bytes) => format!("{} bytes [{}]", bytes.len(), hex_preview(bytes)),
            Self::Array(values) => values
                .iter()
                .map(Self::to_display_string)
                .collect::<Vec<_>>()
                .join(", "),
            Self::Structure(values) => format!("{} fields", values.len()),
            Self::Unknown { type_id, bytes } => {
                format!(
                    "type {type_id}, {} bytes [{}]",
                    bytes.len(),
                    hex_preview(bytes)
                )
            }
        }
    }
}

fn hex_preview(bytes: &[u8]) -> String {
    const MAX_PREVIEW_BYTES: usize = 24;
    let mut result = bytes
        .iter()
        .take(MAX_PREVIEW_BYTES)
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ");
    if bytes.len() > MAX_PREVIEW_BYTES {
        result.push_str(" …");
    }
    result
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueType {
    String,
    UnsignedInteger,
    SignedInteger,
    Float,
    Rational,
    UnsignedRational,
    Bytes,
    Array,
    Structure,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Source {
    pub container: String,
    pub offset: Option<u64>,
    pub length: Option<u64>,
}

impl Source {
    pub fn new(container: impl Into<String>, offset: Option<u64>, length: Option<u64>) -> Self {
        Self {
            container: container.into(),
            offset,
            length,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Warning {
    pub code: String,
    pub message: String,
    pub offset: Option<u64>,
}

impl Warning {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            offset: None,
        }
    }

    pub fn at(mut self, offset: u64) -> Self {
        self.offset = Some(offset);
        self
    }
}

/// Resource limits applied to untrusted metadata structures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParseLimits {
    pub max_metadata_bytes: usize,
    pub max_value_bytes: usize,
    pub max_ifd_entries: usize,
    pub max_recursion_depth: usize,
    pub max_jpeg_segments: usize,
}

impl Default for ParseLimits {
    fn default() -> Self {
        Self {
            max_metadata_bytes: 16 * 1024 * 1024,
            max_value_bytes: 4 * 1024 * 1024,
            max_ifd_entries: 16_384,
            max_recursion_depth: 16,
            max_jpeg_segments: 4_096,
        }
    }
}

#[derive(Debug, Error)]
pub enum MetraError {
    #[error("I/O error while reading {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("unsupported format: {description}")]
    UnsupportedFormat { description: String },
    #[error("invalid {context}: {message}")]
    InvalidHeader { context: String, message: String },
    #[error("unexpected end of input while reading {context}")]
    UnexpectedEof { context: String },
    #[error("invalid offset in {context}: {offset}")]
    InvalidOffset { context: String, offset: u64 },
    #[error("invalid metadata tag in {context}: {message}")]
    InvalidTag { context: String, message: String },
    #[error("resource limit exceeded for {resource}: {limit}")]
    ResourceLimitExceeded { resource: String, limit: usize },
    #[error("corrupt metadata: {message}")]
    CorruptMetadata { message: String },
    #[error("write failure: {message}")]
    WriteFailure { message: String },
}

pub type Result<T> = std::result::Result<T, MetraError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_tag_key_keeps_namespace_explicit() {
        let tag = Tag {
            namespace: "EXIF".to_owned(),
            group: "IFD0".to_owned(),
            id: Some(0x010F),
            name: "Make".to_owned(),
            description: None,
            raw_value: Some(b"Metra\0".to_vec()),
            value: TagValue::String("Metra".to_owned()),
            value_type: ValueType::String,
            source: Source::default(),
            writable: false,
        };

        assert_eq!(tag.key(), "EXIF:Make");
        assert_eq!(tag.display_value(), "Metra");
    }

    #[test]
    fn metadata_serializes_schema_and_typed_values() {
        let metadata = Metadata::new(FileInfo::new(
            PathBuf::from("photo.jpg"),
            42,
            FileFormat::Jpeg,
        ));
        let json = serde_json::to_string(&metadata).expect("core types should be serializable");
        assert!(json.contains("schema_version"));
        assert!(json.contains("JPEG"));
    }
}
