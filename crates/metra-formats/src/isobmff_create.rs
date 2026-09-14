use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use metra_core::{FileFormat, FileInfo, MetraError, ParseLimits, Result};

use crate::atomic::atomic_replace;
use crate::isobmff::read_isobmff;

const MP4_COMPATIBLE_BRANDS: &[[u8; 4]] = &[*b"isom", *b"iso2", *b"mp41"];
const MOV_COMPATIBLE_BRANDS: &[[u8; 4]] = &[*b"qt  "];
const M4A_COMPATIBLE_BRANDS: &[[u8; 4]] = &[*b"M4A ", *b"isom", *b"mp42"];
const HEIF_COMPATIBLE_BRANDS: &[[u8; 4]] = &[*b"mif1", *b"heic"];
const AVIF_COMPATIBLE_BRANDS: &[[u8; 4]] = &[*b"avif", *b"mif1"];

/// ISO-BMFF brand emitted by the bounded metadata-seed creator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IsobmffCreateKind {
    Mp4,
    Mov,
    M4a,
    Heif,
    Avif,
}

impl IsobmffCreateKind {
    fn file_format(self) -> FileFormat {
        match self {
            Self::Mp4 => FileFormat::Mp4,
            Self::Mov => FileFormat::Mov,
            Self::M4a => FileFormat::M4a,
            Self::Heif => FileFormat::Heif,
            Self::Avif => FileFormat::Avif,
        }
    }

    fn major_brand(self) -> &'static [u8; 4] {
        match self {
            Self::Mp4 => b"isom",
            Self::Mov => b"qt  ",
            Self::M4a => b"M4A ",
            Self::Heif => b"mif1",
            Self::Avif => b"avif",
        }
    }

    fn compatible_brands(self) -> &'static [[u8; 4]] {
        match self {
            Self::Mp4 => MP4_COMPATIBLE_BRANDS,
            Self::Mov => MOV_COMPATIBLE_BRANDS,
            Self::M4a => M4A_COMPATIBLE_BRANDS,
            Self::Heif => HEIF_COMPATIBLE_BRANDS,
            Self::Avif => AVIF_COMPATIBLE_BRANDS,
        }
    }
}

/// One bounded QuickTime-style text field for a new ISO-BMFF seed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IsobmffCreateEntry {
    pub key: String,
    pub value: String,
}

impl IsobmffCreateEntry {
    /// Create one text entry such as `Title` or `Artist`.
    pub fn text(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            value: value.into(),
        }
    }
}

/// Options for creating a metadata-only MP4, MOV, or M4A seed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IsobmffCreateOptions {
    pub kind: IsobmffCreateKind,
    pub entries: Vec<IsobmffCreateEntry>,
    pub width: u32,
    pub height: u32,
}

impl Default for IsobmffCreateOptions {
    fn default() -> Self {
        Self::new(IsobmffCreateKind::Mp4)
    }
}

impl IsobmffCreateOptions {
    /// Start a seed for the selected ISO-BMFF brand.
    pub fn new(kind: IsobmffCreateKind) -> Self {
        Self {
            kind,
            entries: Vec::new(),
            width: 1,
            height: 1,
        }
    }

    /// Add a text field and return the updated options.
    pub fn with_text(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.entries.push(IsobmffCreateEntry::text(key, value));
        self
    }

    /// Add a text field in place.
    pub fn push_text(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.entries.push(IsobmffCreateEntry::text(key, value));
    }

    /// Set bounded image dimensions for a HEIF or AVIF metadata seed.
    pub fn with_dimensions(mut self, width: u32, height: u32) -> Self {
        self.width = width;
        self.height = height;
        self
    }
}

/// Create a bounded metadata-only ISO-BMFF seed without media tracks or samples.
pub fn create_isobmff_to_vec(
    options: &IsobmffCreateOptions,
    limits: ParseLimits,
) -> Result<Vec<u8>> {
    if options.entries.len() > limits.max_ifd_entries {
        return Err(MetraError::ResourceLimitExceeded {
            resource: "ISO-BMFF creation metadata entries".to_owned(),
            limit: limits.max_ifd_entries,
        });
    }
    let image_kind = matches!(
        options.kind,
        IsobmffCreateKind::Heif | IsobmffCreateKind::Avif
    );
    if options.width == 0 || options.height == 0 {
        return Err(MetraError::InvalidTag {
            context: "ISO-BMFF creation".to_owned(),
            message: "image dimensions must be non-zero".to_owned(),
        });
    }
    if image_kind && !options.entries.is_empty() {
        return Err(MetraError::InvalidTag {
            context: "ISO-BMFF creation".to_owned(),
            message: "HEIF/AVIF seeds do not accept QuickTime text items".to_owned(),
        });
    }
    let mut items = Vec::with_capacity(options.entries.len());
    let mut seen = Vec::with_capacity(options.entries.len());
    for entry in &options.entries {
        let kind = text_item_kind(&entry.key).ok_or_else(|| MetraError::InvalidTag {
            context: "ISO-BMFF creation".to_owned(),
            message: format!("unsupported text creation key {}", entry.key),
        })?;
        if entry.value.contains('\0') {
            return Err(MetraError::InvalidTag {
                context: format!("ISO-BMFF creation {}", entry.key),
                message: "text values may not contain NUL bytes".to_owned(),
            });
        }
        if entry.value.len() > limits.max_value_bytes {
            return Err(MetraError::ResourceLimitExceeded {
                resource: format!("ISO-BMFF creation value {}", entry.key),
                limit: limits.max_value_bytes,
            });
        }
        if seen.contains(&kind) {
            return Err(MetraError::InvalidTag {
                context: "ISO-BMFF creation".to_owned(),
                message: format!("duplicate text creation key {}", entry.key),
            });
        }
        seen.push(kind);
        let data = [
            0_u32.to_be_bytes().as_slice(),
            0_u32.to_be_bytes().as_slice(),
            entry.value.as_bytes(),
        ]
        .concat();
        let data_box = box_with_kind(b"data", &data)?;
        items.extend_from_slice(&box_with_kind(&kind, &data_box)?);
    }

    let container = if image_kind {
        image_meta(options.width, options.height)?
    } else {
        let ilst = box_with_kind(b"ilst", &items)?;
        let udta = box_with_kind(b"udta", &ilst)?;
        let mut mvhd_data = vec![0_u8; 100];
        mvhd_data[12..16].copy_from_slice(&1_000_u32.to_be_bytes());
        let mvhd = box_with_kind(b"mvhd", &mvhd_data)?;
        box_with_kind(b"moov", &[mvhd, udta].concat())?
    };

    let mut ftyp_data = Vec::with_capacity(8 + options.kind.compatible_brands().len() * 4);
    ftyp_data.extend_from_slice(options.kind.major_brand());
    ftyp_data.extend_from_slice(&0_u32.to_be_bytes());
    for brand in options.kind.compatible_brands() {
        ftyp_data.extend_from_slice(brand);
    }
    let output = [box_with_kind(b"ftyp", &ftyp_data)?, container].concat();
    if output.len() > limits.max_metadata_bytes || output.len() > limits.max_value_bytes {
        return Err(MetraError::ResourceLimitExceeded {
            resource: "ISO-BMFF creation output".to_owned(),
            limit: limits.max_metadata_bytes.min(limits.max_value_bytes),
        });
    }
    validate_created_isobmff(&output, options.kind, limits)?;
    Ok(output)
}

fn image_meta(width: u32, height: u32) -> Result<Vec<u8>> {
    let mut hdlr_data = vec![0_u8; 12];
    hdlr_data[8..12].copy_from_slice(b"pict");
    let hdlr = box_with_kind(b"hdlr", &hdlr_data)?;
    let pitm = box_with_kind(b"pitm", &[0, 0, 0, 0, 0, 1])?;
    let ispe_data = [
        [0_u8; 4].as_slice(),
        width.to_be_bytes().as_slice(),
        height.to_be_bytes().as_slice(),
    ]
    .concat();
    let ispe = box_with_kind(b"ispe", &ispe_data)?;
    let pixi = box_with_kind(b"pixi", &[0, 0, 0, 0, 3, 8, 8, 8])?;
    let ipco = box_with_kind(b"ipco", &[ispe, pixi].concat())?;
    let iprp = box_with_kind(b"iprp", &ipco)?;
    let meta_data = [
        [0_u8; 4].as_slice(),
        hdlr.as_slice(),
        pitm.as_slice(),
        iprp.as_slice(),
    ]
    .concat();
    box_with_kind(b"meta", &meta_data)
}

/// Create a new ISO-BMFF path without overwriting an existing destination.
pub fn create_isobmff_path(
    path: impl AsRef<Path>,
    options: &IsobmffCreateOptions,
    limits: ParseLimits,
) -> Result<()> {
    let path = path.as_ref().to_path_buf();
    if path.exists() {
        return Err(MetraError::WriteFailure {
            message: format!("refusing to overwrite {}", path.display()),
        });
    }
    let bytes = create_isobmff_to_vec(options, limits)?;
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
        validate_created_isobmff(&bytes, options.kind, limits)?;
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

fn text_item_kind(key: &str) -> Option<[u8; 4]> {
    let key = key.strip_prefix("ISOBMFF:").unwrap_or(key);
    match key {
        "Title" => Some(*b"\xA9nam"),
        "Artist" => Some(*b"\xA9ART"),
        "Album" => Some(*b"\xA9alb"),
        "Year" => Some(*b"\xA9day"),
        "Comment" => Some(*b"\xA9cmt"),
        "AlbumArtist" => Some(*b"aART"),
        "Description" => Some(*b"desc"),
        "PurchaseDate" => Some(*b"purd"),
        "Encoder" => Some(*b"too "),
        _ => None,
    }
}

fn box_with_kind(kind: &[u8; 4], data: &[u8]) -> Result<Vec<u8>> {
    let size = u32::try_from(data.len().saturating_add(8)).map_err(|_| {
        MetraError::ResourceLimitExceeded {
            resource: "ISO-BMFF box size".to_owned(),
            limit: u32::MAX as usize,
        }
    })?;
    let mut output = Vec::with_capacity(size as usize);
    output.extend_from_slice(&size.to_be_bytes());
    output.extend_from_slice(kind);
    output.extend_from_slice(data);
    Ok(output)
}

fn validate_created_isobmff(
    bytes: &[u8],
    kind: IsobmffCreateKind,
    limits: ParseLimits,
) -> Result<()> {
    read_isobmff(
        &mut std::io::Cursor::new(bytes),
        FileInfo::new("created.mp4".into(), bytes.len() as u64, kind.file_format()),
        limits,
    )?;
    Ok(())
}

fn temporary_path(path: &Path) -> Result<PathBuf> {
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("created.mp4");
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
    fn creates_readable_mp4_seed_with_quicktime_text() {
        let options = IsobmffCreateOptions::new(IsobmffCreateKind::Mp4)
            .with_text("Title", "Metra")
            .with_text("Artist", "Othmane");
        let bytes = create_isobmff_to_vec(&options, ParseLimits::default()).unwrap();
        let metadata = read_isobmff(
            &mut std::io::Cursor::new(bytes.clone()),
            FileInfo::new("created.mp4".into(), bytes.len() as u64, FileFormat::Mp4),
            ParseLimits::default(),
        )
        .unwrap();
        assert_eq!(
            metadata.find("ISOBMFF:MajorBrand").unwrap().display_value(),
            "isom"
        );
        assert_eq!(
            metadata.find("ISOBMFF:Title").unwrap().display_value(),
            "Metra"
        );
        assert_eq!(
            metadata.find("ISOBMFF:Artist").unwrap().display_value(),
            "Othmane"
        );
    }

    #[test]
    fn creates_mov_and_m4a_brands() {
        for (kind, expected) in [
            (IsobmffCreateKind::Mov, FileFormat::Mov),
            (IsobmffCreateKind::M4a, FileFormat::M4a),
        ] {
            let bytes =
                create_isobmff_to_vec(&IsobmffCreateOptions::new(kind), ParseLimits::default())
                    .unwrap();
            assert_eq!(crate::detect_format(&bytes).unwrap().format, expected);
        }
    }

    #[test]
    fn creates_heif_and_avif_metadata_seeds_with_dimensions() {
        for (kind, expected) in [
            (IsobmffCreateKind::Heif, FileFormat::Heif),
            (IsobmffCreateKind::Avif, FileFormat::Avif),
        ] {
            let options = IsobmffCreateOptions::new(kind).with_dimensions(640, 480);
            let bytes = create_isobmff_to_vec(&options, ParseLimits::default()).unwrap();
            let metadata = read_isobmff(
                &mut std::io::Cursor::new(bytes.clone()),
                FileInfo::new("created.image".into(), bytes.len() as u64, expected),
                ParseLimits::default(),
            )
            .unwrap();
            assert_eq!(crate::detect_format(&bytes).unwrap().format, expected);
            assert_eq!(
                metadata.find("ISOBMFF:ImageWidth").unwrap().display_value(),
                "640"
            );
            assert_eq!(
                metadata
                    .find("ISOBMFF:ImageHeight")
                    .unwrap()
                    .display_value(),
                "480"
            );
        }
    }
}
