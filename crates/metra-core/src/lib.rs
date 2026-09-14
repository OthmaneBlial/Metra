//! Stable, format-independent data structures used by Metra readers and
//! consumers.

use std::collections::BTreeMap;
use std::fmt;
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const SCHEMA_VERSION: u8 = 1;

/// Stable definition for a format-level metadata tag.
///
/// Definitions are kept separate from display values so readers and clients
/// can retain the numeric identifier while using a canonical name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TagDefinition {
    pub namespace: &'static str,
    pub id: u32,
    pub name: &'static str,
    pub description: &'static str,
}

include!(concat!(env!("OUT_DIR"), "/tag_definitions.rs"));

/// Return the catalog of definitions currently shared by the readers.
pub fn tag_definitions() -> &'static [TagDefinition] {
    TAG_DEFINITIONS
}

/// Resolve a namespace/id pair while preserving a stable fallback for unknown tags.
pub fn tag_definition(namespace: &str, id: u32) -> TagDefinition {
    TAG_DEFINITIONS
        .iter()
        .find(|definition| definition.namespace == namespace && definition.id == id)
        .copied()
        .unwrap_or(TagDefinition {
            namespace: match namespace {
                "GPS" => "GPS",
                "Interop" => "Interop",
                "IPTC" => "IPTC",
                "ICC" => "ICC",
                "MakerNotes" => "MakerNotes",
                "DNG" => "DNG",
                _ => "EXIF",
            },
            id,
            name: "Unknown",
            description: "Unknown metadata tag",
        })
}

/// A detected container or file family. More formats can be added without
/// changing the shape of a metadata record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum FileFormat {
    Jpeg,
    Tiff,
    Png,
    Webp,
    Heif,
    Avif,
    Mp4,
    Mov,
    M4a,
    Pdf,
    Gif,
    Mp3,
    Flac,
    Ogg,
    Wav,
    Svg,
    Icc,
    Xmp,
    Psd,
    Avi,
    Mkv,
    Webm,
    Raw,
    Unknown,
}

impl FileFormat {
    pub const fn mime_type(self) -> Option<&'static str> {
        match self {
            Self::Jpeg => Some("image/jpeg"),
            Self::Tiff => Some("image/tiff"),
            Self::Png => Some("image/png"),
            Self::Webp => Some("image/webp"),
            Self::Heif => Some("image/heif"),
            Self::Avif => Some("image/avif"),
            Self::Mp4 => Some("video/mp4"),
            Self::Mov => Some("video/quicktime"),
            Self::M4a => Some("audio/mp4"),
            Self::Pdf => Some("application/pdf"),
            Self::Gif => Some("image/gif"),
            Self::Mp3 => Some("audio/mpeg"),
            Self::Flac => Some("audio/flac"),
            Self::Ogg => Some("audio/ogg"),
            Self::Wav => Some("audio/wav"),
            Self::Svg => Some("image/svg+xml"),
            Self::Icc => Some("application/vnd.iccprofile"),
            Self::Xmp => Some("application/rdf+xml"),
            Self::Psd => Some("image/vnd.adobe.photoshop"),
            Self::Avi => Some("video/x-msvideo"),
            Self::Mkv => Some("video/x-matroska"),
            Self::Webm => Some("video/webm"),
            Self::Raw => Some("image/x-raw"),
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
            Self::Heif => "HEIF",
            Self::Avif => "AVIF",
            Self::Mp4 => "MP4",
            Self::Mov => "MOV",
            Self::M4a => "M4A",
            Self::Pdf => "PDF",
            Self::Gif => "GIF",
            Self::Mp3 => "MP3",
            Self::Flac => "FLAC",
            Self::Ogg => "OGG",
            Self::Wav => "WAV",
            Self::Svg => "SVG",
            Self::Icc => "ICC",
            Self::Xmp => "XMP",
            Self::Psd => "PSD",
            Self::Avi => "AVI",
            Self::Mkv => "MKV",
            Self::Webm => "WebM",
            Self::Raw => "RAW",
            Self::Unknown => "Unknown",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CapabilityStatus {
    Supported,
    Partial,
    Planned,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormatCapabilities {
    pub format: FileFormat,
    pub read: CapabilityStatus,
    pub write: CapabilityStatus,
    pub create: CapabilityStatus,
    pub delete: CapabilityStatus,
    pub lossless_rewrite: CapabilityStatus,
    pub streaming: CapabilityStatus,
}

const FORMAT_CAPABILITIES: &[FormatCapabilities] = &[
    FormatCapabilities {
        format: FileFormat::Jpeg,
        read: CapabilityStatus::Partial,
        write: CapabilityStatus::Partial,
        create: CapabilityStatus::Planned,
        delete: CapabilityStatus::Partial,
        lossless_rewrite: CapabilityStatus::Partial,
        streaming: CapabilityStatus::Partial,
    },
    FormatCapabilities {
        format: FileFormat::Tiff,
        read: CapabilityStatus::Partial,
        write: CapabilityStatus::Partial,
        create: CapabilityStatus::Partial,
        delete: CapabilityStatus::Partial,
        lossless_rewrite: CapabilityStatus::Partial,
        streaming: CapabilityStatus::Partial,
    },
    FormatCapabilities {
        format: FileFormat::Png,
        read: CapabilityStatus::Partial,
        write: CapabilityStatus::Partial,
        create: CapabilityStatus::Partial,
        delete: CapabilityStatus::Partial,
        lossless_rewrite: CapabilityStatus::Partial,
        streaming: CapabilityStatus::Partial,
    },
    FormatCapabilities {
        format: FileFormat::Webp,
        read: CapabilityStatus::Partial,
        write: CapabilityStatus::Partial,
        create: CapabilityStatus::Planned,
        delete: CapabilityStatus::Partial,
        lossless_rewrite: CapabilityStatus::Partial,
        streaming: CapabilityStatus::Partial,
    },
    FormatCapabilities {
        format: FileFormat::Heif,
        read: CapabilityStatus::Partial,
        write: CapabilityStatus::Partial,
        create: CapabilityStatus::Planned,
        delete: CapabilityStatus::Planned,
        lossless_rewrite: CapabilityStatus::Partial,
        streaming: CapabilityStatus::Partial,
    },
    FormatCapabilities {
        format: FileFormat::Avif,
        read: CapabilityStatus::Partial,
        write: CapabilityStatus::Partial,
        create: CapabilityStatus::Planned,
        delete: CapabilityStatus::Planned,
        lossless_rewrite: CapabilityStatus::Partial,
        streaming: CapabilityStatus::Partial,
    },
    FormatCapabilities {
        format: FileFormat::Mp4,
        read: CapabilityStatus::Partial,
        write: CapabilityStatus::Partial,
        create: CapabilityStatus::Planned,
        delete: CapabilityStatus::Planned,
        lossless_rewrite: CapabilityStatus::Partial,
        streaming: CapabilityStatus::Partial,
    },
    FormatCapabilities {
        format: FileFormat::Mov,
        read: CapabilityStatus::Partial,
        write: CapabilityStatus::Partial,
        create: CapabilityStatus::Planned,
        delete: CapabilityStatus::Planned,
        lossless_rewrite: CapabilityStatus::Partial,
        streaming: CapabilityStatus::Partial,
    },
    FormatCapabilities {
        format: FileFormat::M4a,
        read: CapabilityStatus::Partial,
        write: CapabilityStatus::Partial,
        create: CapabilityStatus::Planned,
        delete: CapabilityStatus::Planned,
        lossless_rewrite: CapabilityStatus::Partial,
        streaming: CapabilityStatus::Partial,
    },
    FormatCapabilities {
        format: FileFormat::Pdf,
        read: CapabilityStatus::Partial,
        write: CapabilityStatus::Partial,
        create: CapabilityStatus::Planned,
        delete: CapabilityStatus::Planned,
        lossless_rewrite: CapabilityStatus::Partial,
        streaming: CapabilityStatus::Partial,
    },
    FormatCapabilities {
        format: FileFormat::Gif,
        read: CapabilityStatus::Partial,
        write: CapabilityStatus::Partial,
        create: CapabilityStatus::Partial,
        delete: CapabilityStatus::Partial,
        lossless_rewrite: CapabilityStatus::Partial,
        streaming: CapabilityStatus::Partial,
    },
    FormatCapabilities {
        format: FileFormat::Mp3,
        read: CapabilityStatus::Partial,
        write: CapabilityStatus::Partial,
        create: CapabilityStatus::Partial,
        delete: CapabilityStatus::Partial,
        lossless_rewrite: CapabilityStatus::Partial,
        streaming: CapabilityStatus::Partial,
    },
    FormatCapabilities {
        format: FileFormat::Flac,
        read: CapabilityStatus::Partial,
        write: CapabilityStatus::Partial,
        create: CapabilityStatus::Partial,
        delete: CapabilityStatus::Partial,
        lossless_rewrite: CapabilityStatus::Partial,
        streaming: CapabilityStatus::Partial,
    },
    FormatCapabilities {
        format: FileFormat::Ogg,
        read: CapabilityStatus::Partial,
        write: CapabilityStatus::Partial,
        create: CapabilityStatus::Partial,
        delete: CapabilityStatus::Partial,
        lossless_rewrite: CapabilityStatus::Partial,
        streaming: CapabilityStatus::Partial,
    },
    FormatCapabilities {
        format: FileFormat::Wav,
        read: CapabilityStatus::Partial,
        write: CapabilityStatus::Partial,
        create: CapabilityStatus::Partial,
        delete: CapabilityStatus::Partial,
        lossless_rewrite: CapabilityStatus::Partial,
        streaming: CapabilityStatus::Partial,
    },
    FormatCapabilities {
        format: FileFormat::Svg,
        read: CapabilityStatus::Partial,
        write: CapabilityStatus::Partial,
        create: CapabilityStatus::Partial,
        delete: CapabilityStatus::Partial,
        lossless_rewrite: CapabilityStatus::Partial,
        streaming: CapabilityStatus::Partial,
    },
    FormatCapabilities {
        format: FileFormat::Icc,
        read: CapabilityStatus::Partial,
        write: CapabilityStatus::Planned,
        create: CapabilityStatus::Partial,
        delete: CapabilityStatus::Planned,
        lossless_rewrite: CapabilityStatus::Planned,
        streaming: CapabilityStatus::Partial,
    },
    FormatCapabilities {
        format: FileFormat::Xmp,
        read: CapabilityStatus::Partial,
        write: CapabilityStatus::Planned,
        create: CapabilityStatus::Partial,
        delete: CapabilityStatus::Planned,
        lossless_rewrite: CapabilityStatus::Planned,
        streaming: CapabilityStatus::Partial,
    },
    FormatCapabilities {
        format: FileFormat::Psd,
        read: CapabilityStatus::Partial,
        write: CapabilityStatus::Partial,
        create: CapabilityStatus::Planned,
        delete: CapabilityStatus::Planned,
        lossless_rewrite: CapabilityStatus::Partial,
        streaming: CapabilityStatus::Partial,
    },
    FormatCapabilities {
        format: FileFormat::Avi,
        read: CapabilityStatus::Partial,
        write: CapabilityStatus::Partial,
        create: CapabilityStatus::Planned,
        delete: CapabilityStatus::Partial,
        lossless_rewrite: CapabilityStatus::Partial,
        streaming: CapabilityStatus::Partial,
    },
    FormatCapabilities {
        format: FileFormat::Mkv,
        read: CapabilityStatus::Partial,
        write: CapabilityStatus::Partial,
        create: CapabilityStatus::Planned,
        delete: CapabilityStatus::Partial,
        lossless_rewrite: CapabilityStatus::Partial,
        streaming: CapabilityStatus::Partial,
    },
    FormatCapabilities {
        format: FileFormat::Webm,
        read: CapabilityStatus::Partial,
        write: CapabilityStatus::Partial,
        create: CapabilityStatus::Planned,
        delete: CapabilityStatus::Partial,
        lossless_rewrite: CapabilityStatus::Partial,
        streaming: CapabilityStatus::Partial,
    },
    FormatCapabilities {
        format: FileFormat::Raw,
        read: CapabilityStatus::Partial,
        write: CapabilityStatus::Partial,
        create: CapabilityStatus::Planned,
        delete: CapabilityStatus::Partial,
        lossless_rewrite: CapabilityStatus::Partial,
        streaming: CapabilityStatus::Partial,
    },
];

pub fn format_capabilities(format: FileFormat) -> FormatCapabilities {
    FORMAT_CAPABILITIES
        .iter()
        .find(|capabilities| capabilities.format == format)
        .copied()
        .unwrap_or(FormatCapabilities {
            format,
            read: CapabilityStatus::Unsupported,
            write: CapabilityStatus::Unsupported,
            create: CapabilityStatus::Unsupported,
            delete: CapabilityStatus::Unsupported,
            lossless_rewrite: CapabilityStatus::Unsupported,
            streaming: CapabilityStatus::Unsupported,
        })
}

pub fn format_capabilities_all() -> &'static [FormatCapabilities] {
    FORMAT_CAPABILITIES
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileInfo {
    pub path: PathBuf,
    pub size: u64,
    pub format: FileFormat,
    pub mime_type: Option<String>,
}

/// Stable identity of a tag independent from its localized display value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TagIdentifier {
    pub namespace: String,
    pub id: Option<u32>,
    pub name: String,
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

    /// Find every tag with the same canonical namespace/name key.
    pub fn find_all(&self, key: &str) -> Vec<&Tag> {
        self.tags.iter().filter(|tag| tag.key() == key).collect()
    }

    /// Find a tag by its format-level numeric identifier.
    pub fn find_by_id(&self, namespace: &str, id: u32) -> Option<&Tag> {
        self.tags
            .iter()
            .find(|tag| tag.namespace == namespace && tag.id == Some(id))
    }

    /// Find every tag with the same format-level numeric identifier.
    pub fn find_all_by_id(&self, namespace: &str, id: u32) -> Vec<&Tag> {
        self.tags
            .iter()
            .filter(|tag| tag.namespace == namespace && tag.id == Some(id))
            .collect()
    }

    /// Compare tag values while preserving duplicate tags under each key.
    /// `self` is the baseline and `other` is the candidate document.
    pub fn diff(&self, other: &Self) -> MetadataDiff {
        let mut keys = BTreeMap::new();
        for tag in self.tags.iter().chain(other.tags.iter()) {
            keys.insert(tag.key(), ());
        }

        let mut diff = MetadataDiff::default();
        for key in keys.into_keys() {
            let before_tags = self
                .tags
                .iter()
                .filter(|tag| tag.key() == key)
                .collect::<Vec<_>>();
            let after_tags = other
                .tags
                .iter()
                .filter(|tag| tag.key() == key)
                .collect::<Vec<_>>();
            match (before_tags.is_empty(), after_tags.is_empty()) {
                (true, false) => diff.added.extend(after_tags.into_iter().cloned()),
                (false, true) => diff.removed.extend(before_tags.into_iter().cloned()),
                (false, false) => {
                    let before = before_tags
                        .iter()
                        .map(|tag| tag.value.clone())
                        .collect::<Vec<_>>();
                    let after = after_tags
                        .iter()
                        .map(|tag| tag.value.clone())
                        .collect::<Vec<_>>();
                    if before != after {
                        diff.changed.push(TagDifference { key, before, after });
                    }
                }
                (true, true) => unreachable!("a key comes from at least one metadata tag"),
            }
        }
        diff
    }
}

/// Deterministic value-level comparison between two metadata documents.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct MetadataDiff {
    pub added: Vec<Tag>,
    pub removed: Vec<Tag>,
    pub changed: Vec<TagDifference>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TagDifference {
    pub key: String,
    pub before: Vec<TagValue>,
    pub after: Vec<TagValue>,
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

    pub fn identifier(&self) -> TagIdentifier {
        TagIdentifier {
            namespace: self.namespace.clone(),
            id: self.id,
            name: self.name.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TagValue {
    String(String),
    Unsigned(u64),
    Signed(i64),
    Float(f64),
    Date {
        year: u16,
        month: u8,
        day: u8,
    },
    Time {
        hour: u8,
        minute: u8,
        second: u8,
        nanosecond: u32,
    },
    DateTime {
        year: u16,
        month: u8,
        day: u8,
        hour: u8,
        minute: u8,
        second: u8,
        nanosecond: u32,
        offset_minutes: Option<i16>,
    },
    Rational {
        numerator: i64,
        denominator: i64,
    },
    UnsignedRational {
        numerator: u64,
        denominator: u64,
    },
    Bytes(Vec<u8>),
    Array(Vec<TagValue>),
    Structure(BTreeMap<String, TagValue>),
    Unknown {
        type_id: u16,
        bytes: Vec<u8>,
    },
}

impl TagValue {
    pub fn to_display_string(&self) -> String {
        match self {
            Self::String(value) => value.clone(),
            Self::Unsigned(value) => value.to_string(),
            Self::Signed(value) => value.to_string(),
            Self::Float(value) => value.to_string(),
            Self::Date { year, month, day } => format!("{year:04}-{month:02}-{day:02}"),
            Self::Time {
                hour,
                minute,
                second,
                nanosecond,
            } => format_time(*hour, *minute, *second, *nanosecond),
            Self::DateTime {
                year,
                month,
                day,
                hour,
                minute,
                second,
                nanosecond,
                offset_minutes,
            } => {
                let suffix = match offset_minutes {
                    Some(offset) => format_offset(*offset),
                    None => String::new(),
                };
                format!(
                    "{year:04}:{month:02}:{day:02} {time}{suffix}",
                    time = format_time(*hour, *minute, *second, *nanosecond),
                )
            }
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

fn format_time(hour: u8, minute: u8, second: u8, nanosecond: u32) -> String {
    if nanosecond == 0 {
        format!("{hour:02}:{minute:02}:{second:02}")
    } else {
        let fractional = format!("{nanosecond:09}");
        format!(
            "{hour:02}:{minute:02}:{second:02}.{}",
            fractional.trim_end_matches('0')
        )
    }
}

fn format_offset(offset_minutes: i16) -> String {
    if offset_minutes == 0 {
        return "Z".to_owned();
    }
    let sign = if offset_minutes.is_negative() {
        '-'
    } else {
        '+'
    };
    let absolute = offset_minutes.unsigned_abs();
    format!("{sign}{:02}:{:02}", absolute / 60, absolute % 60)
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
    Date,
    Time,
    DateTime,
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
    pub max_xmp_nodes: usize,
}

impl Default for ParseLimits {
    fn default() -> Self {
        Self {
            max_metadata_bytes: 16 * 1024 * 1024,
            max_value_bytes: 4 * 1024 * 1024,
            max_ifd_entries: 16_384,
            max_recursion_depth: 16,
            max_jpeg_segments: 4_096,
            max_xmp_nodes: 10_000,
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
    #[error("invalid XML metadata: {message}")]
    InvalidXml { message: String },
    #[error("resource limit exceeded for {resource}: {limit}")]
    ResourceLimitExceeded { resource: String, limit: usize },
    #[error("corrupt metadata: {message}")]
    CorruptMetadata { message: String },
    #[error("write failure: {message}")]
    WriteFailure { message: String },
    #[error("operation cancelled")]
    Cancelled,
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

    #[test]
    fn temporal_values_keep_structured_components_and_stable_display() {
        let date = TagValue::Date {
            year: 2026,
            month: 9,
            day: 13,
        };
        let time = TagValue::Time {
            hour: 12,
            minute: 34,
            second: 56,
            nanosecond: 125_000_000,
        };
        let date_time = TagValue::DateTime {
            year: 2026,
            month: 9,
            day: 13,
            hour: 12,
            minute: 34,
            second: 56,
            nanosecond: 0,
            offset_minutes: Some(120),
        };

        assert_eq!(date.to_display_string(), "2026-09-13");
        assert_eq!(time.to_display_string(), "12:34:56.125");
        assert_eq!(date_time.to_display_string(), "2026:09:13 12:34:56+02:00");
        let json = serde_json::to_string(&date_time).expect("temporal values should serialize");
        assert!(json.contains("date_time"));
        assert!(json.contains("offset_minutes"));
    }

    #[test]
    fn tag_catalog_and_numeric_lookup_are_stable() {
        let definition = tag_definition("EXIF", 0x010F);
        assert_eq!(definition.name, "Make");
        assert!(tag_definitions().iter().any(|item| item.name == "Make"));
        assert_eq!(tag_definition("GPS", 0x000D).name, "GPSSpeed");
        assert_eq!(tag_definition("IPTC", 25).name, "Keywords");
        assert_eq!(tag_definition("EXIF", 0x9286).name, "UserComment");
        assert_eq!(
            tag_definition("ICC", u32::from_be_bytes(*b"wtpt")).name,
            "MediaWhitePoint"
        );
        assert_eq!(tag_definition("EXIF", 0x829A).name, "ExposureTime");
        assert_eq!(tag_definition("EXIF", 0x0102).name, "BitsPerSample");
        assert_eq!(
            tag_definition("EXIF", 0x8831).name,
            "StandardOutputSensitivity"
        );
        assert_eq!(tag_definition("EXIF", 0xA432).name, "LensSpecification");
        assert_eq!(tag_definition("EXIF", 0xA434).name, "LensModel");
        assert_eq!(tag_definition("MakerNotes", 0x0002).name, "Nikon:ISO");
        assert_eq!(tag_definition("DNG", 0xC612).name, "DNGVersion");

        let tag = Tag {
            namespace: "EXIF".to_owned(),
            group: "IFD0".to_owned(),
            id: Some(0x010F),
            name: "Make".to_owned(),
            description: None,
            raw_value: None,
            value: TagValue::String("Metra".to_owned()),
            value_type: ValueType::String,
            source: Source::default(),
            writable: false,
        };
        let mut metadata = Metadata::new(FileInfo::new(
            PathBuf::from("photo.jpg"),
            1,
            FileFormat::Jpeg,
        ));
        metadata.add_tag(tag);
        assert_eq!(metadata.find_by_id("EXIF", 0x010F).unwrap().name, "Make");
        assert_eq!(
            metadata.find("EXIF:Make").unwrap().identifier().id,
            Some(0x010F)
        );
        assert_eq!(metadata.find_all("EXIF:Make").len(), 1);
        assert_eq!(metadata.find_all_by_id("EXIF", 0x010F).len(), 1);
    }

    #[test]
    fn metadata_diff_preserves_additions_removals_and_changes() {
        let tag = |name: &str, value: &str| Tag {
            namespace: "XMP".to_owned(),
            group: "RDF/Description".to_owned(),
            id: None,
            name: name.to_owned(),
            description: None,
            raw_value: None,
            value: TagValue::String(value.to_owned()),
            value_type: ValueType::String,
            source: Source::default(),
            writable: false,
        };
        let mut before = Metadata::new(FileInfo::new(
            PathBuf::from("before.jpg"),
            1,
            FileFormat::Jpeg,
        ));
        before.add_tag(tag("Title", "old"));
        before.add_tag(tag("Removed", "value"));
        let mut after = Metadata::new(FileInfo::new(
            PathBuf::from("after.jpg"),
            1,
            FileFormat::Jpeg,
        ));
        after.add_tag(tag("Title", "new"));
        after.add_tag(tag("Added", "value"));

        let diff = before.diff(&after);
        assert_eq!(diff.added[0].key(), "XMP:Added");
        assert_eq!(diff.removed[0].key(), "XMP:Removed");
        assert_eq!(diff.changed[0].key, "XMP:Title");
        assert_eq!(diff.changed[0].before, vec![TagValue::String("old".into())]);
        assert_eq!(diff.changed[0].after, vec![TagValue::String("new".into())]);
    }

    #[test]
    fn format_capability_matrix_matches_current_writer_surface() {
        let jpeg = format_capabilities(FileFormat::Jpeg);
        assert_eq!(jpeg.read, CapabilityStatus::Partial);
        assert_eq!(jpeg.write, CapabilityStatus::Partial);
        assert_eq!(jpeg.create, CapabilityStatus::Planned);

        let ogg = format_capabilities(FileFormat::Ogg);
        assert_eq!(ogg.write, CapabilityStatus::Partial);
        assert_eq!(ogg.delete, CapabilityStatus::Partial);
        assert_eq!(ogg.lossless_rewrite, CapabilityStatus::Partial);

        let unknown = format_capabilities(FileFormat::Unknown);
        assert_eq!(unknown.read, CapabilityStatus::Unsupported);
        assert_eq!(
            format_capabilities(FileFormat::Icc).read,
            CapabilityStatus::Partial
        );
        assert_eq!(
            format_capabilities(FileFormat::Xmp).read,
            CapabilityStatus::Partial
        );
        let raw = format_capabilities(FileFormat::Raw);
        assert_eq!(raw.write, CapabilityStatus::Partial);
        assert_eq!(raw.delete, CapabilityStatus::Partial);
        assert_eq!(raw.lossless_rewrite, CapabilityStatus::Partial);
        assert_eq!(
            format_capabilities(FileFormat::Tiff).delete,
            CapabilityStatus::Partial
        );
        assert_eq!(
            format_capabilities(FileFormat::Tiff).create,
            CapabilityStatus::Partial
        );
        assert_eq!(
            format_capabilities(FileFormat::Png).create,
            CapabilityStatus::Partial
        );
        assert_eq!(
            format_capabilities(FileFormat::Xmp).create,
            CapabilityStatus::Partial
        );
        assert_eq!(
            format_capabilities(FileFormat::Icc).create,
            CapabilityStatus::Partial
        );
        assert_eq!(
            format_capabilities(FileFormat::Flac).create,
            CapabilityStatus::Partial
        );
        assert_eq!(
            format_capabilities(FileFormat::Gif).create,
            CapabilityStatus::Partial
        );
        assert_eq!(
            format_capabilities(FileFormat::Wav).create,
            CapabilityStatus::Partial
        );
        assert_eq!(
            format_capabilities(FileFormat::Mp3).create,
            CapabilityStatus::Partial
        );
        assert_eq!(
            format_capabilities(FileFormat::Ogg).create,
            CapabilityStatus::Partial
        );
        assert_eq!(
            format_capabilities(FileFormat::Svg).create,
            CapabilityStatus::Partial
        );
        assert_eq!(
            format_capabilities(FileFormat::Avi).delete,
            CapabilityStatus::Partial
        );
        assert_eq!(
            format_capabilities(FileFormat::Mkv).delete,
            CapabilityStatus::Partial
        );
        assert_eq!(
            format_capabilities(FileFormat::Webm).delete,
            CapabilityStatus::Partial
        );
        assert_eq!(format_capabilities_all().len(), 23);
    }
}
