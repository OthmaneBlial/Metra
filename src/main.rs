use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Parser;
use metra_core::{Metadata, ParseLimits};

#[derive(Debug, Parser)]
#[command(
    name = "metra",
    version,
    about = "Fast, safe metadata inspection powered by Rust",
    long_about = "Inspect file metadata without decoding image pixels."
)]
struct Arguments {
    /// Emit one structured JSON document (an array when multiple files are read).
    #[arg(long, conflicts_with = "jsonl")]
    json: bool,

    /// Emit one JSON metadata document per input file.
    #[arg(long, conflicts_with = "json")]
    jsonl: bool,

    /// Emit one CSV row per tag.
    #[arg(long, conflicts_with_all = ["json", "jsonl"])]
    csv: bool,

    /// Emit TOML metadata (a `files` table is used for multiple inputs).
    #[arg(long, conflicts_with_all = ["json", "jsonl", "csv", "yaml"])]
    toml: bool,

    /// Emit YAML metadata (a `files` list is used for multiple inputs).
    #[arg(long, conflicts_with_all = ["json", "jsonl", "csv", "toml"])]
    yaml: bool,

    /// Select tags by canonical key or display name for read-only output.
    #[arg(
        long = "tag",
        value_name = "NAME",
        action = clap::ArgAction::Append,
        conflicts_with_all = [
            "set",
            "delete",
            "copy",
            "create_tiff",
            "create_png",
            "create_xmp",
            "create_flac",
            "create_gif",
            "create_wav",
            "create_icc",
            "create_mp3",
            "create_ogg",
            "create_svg",
            "compare"
        ]
    )]
    tag: Vec<String>,

    /// Replace the value of a supported writable tag in place.
    #[arg(
        long = "set",
        value_name = "KEY=VALUE",
        conflicts_with = "delete",
        conflicts_with_all = ["json", "jsonl", "csv", "toml", "yaml"]
    )]
    set: Option<String>,

    /// Delete a supported writable tag in place.
    #[arg(
        long,
        value_name = "KEY",
        conflicts_with = "set",
        conflicts_with_all = ["json", "jsonl", "csv", "toml", "yaml"]
    )]
    delete: Option<String>,

    /// Copy a supported writable tag from SOURCE into each target file.
    #[arg(
        long,
        value_name = "KEY=SOURCE",
        conflicts_with_all = ["set", "delete", "json", "jsonl", "csv", "toml", "yaml"]
    )]
    copy: Option<String>,

    /// Create a new minimal TIFF with one or more EXIF ASCII seed fields.
    #[arg(
        long = "create-tiff",
        value_name = "KEY=VALUE",
        action = clap::ArgAction::Append,
        conflicts_with_all = [
            "set",
            "delete",
            "copy",
            "create_png",
            "create_xmp",
            "create_flac",
            "create_gif",
            "create_wav",
            "create_icc",
            "create_mp3",
            "create_ogg",
            "create_svg",
            "json",
            "jsonl",
            "csv",
            "toml",
            "yaml",
            "validate",
            "compare"
        ]
    )]
    create_tiff: Vec<String>,

    /// Create a new minimal JPEG metadata container with Comment/XMP fields.
    #[arg(
        long = "create-jpeg",
        value_name = "KEY=VALUE",
        action = clap::ArgAction::Append,
        conflicts_with_all = [
            "set",
            "delete",
            "copy",
            "create_tiff",
            "create_png",
            "create_xmp",
            "create_flac",
            "create_gif",
            "create_wav",
            "create_icc",
            "create_mp3",
            "create_ogg",
            "create_svg",
            "create_webp_xmp",
            "json",
            "jsonl",
            "csv",
            "toml",
            "yaml",
            "tag",
            "validate",
            "compare"
        ]
    )]
    create_jpeg: Vec<String>,

    /// Create a minimal PDF with bounded Info dictionary fields.
    #[arg(
        long = "create-pdf",
        value_name = "KEY=VALUE",
        action = clap::ArgAction::Append,
        conflicts_with_all = [
            "set",
            "delete",
            "copy",
            "create_tiff",
            "create_jpeg",
            "create_png",
            "create_xmp",
            "create_flac",
            "create_gif",
            "create_wav",
            "create_icc",
            "create_mp3",
            "create_ogg",
            "create_svg",
            "create_webp_xmp",
            "json",
            "jsonl",
            "csv",
            "toml",
            "yaml",
            "tag",
            "validate",
            "compare"
        ]
    )]
    create_pdf: Vec<String>,

    /// Create a minimal 1x1 RGB PSD with an optional XMP image resource.
    #[arg(
        long = "create-psd",
        value_name = "XMP=PACKET",
        action = clap::ArgAction::Append,
        conflicts_with_all = [
            "set",
            "delete",
            "copy",
            "create_tiff",
            "create_jpeg",
            "create_pdf",
            "create_avi",
            "create_mkv",
            "create_webm",
            "create_png",
            "create_xmp",
            "create_wav",
            "create_flac",
            "create_gif",
            "create_icc",
            "create_mp3",
            "create_ogg",
            "create_svg",
            "create_webp_xmp",
            "json",
            "jsonl",
            "csv",
            "toml",
            "yaml",
            "tag",
            "validate",
            "compare"
        ]
    )]
    create_psd: Vec<String>,

    /// Create a new minimal 1x1 AVI seed with one or more INFO fields.
    #[arg(
        long = "create-avi",
        value_name = "KEY=VALUE",
        action = clap::ArgAction::Append,
        conflicts_with_all = [
            "set",
            "delete",
            "copy",
            "create_tiff",
            "create_jpeg",
            "create_pdf",
            "create_png",
            "create_xmp",
            "create_wav",
            "create_flac",
            "create_gif",
            "create_icc",
            "create_mp3",
            "create_ogg",
            "create_svg",
            "create_webp_xmp",
            "json",
            "jsonl",
            "csv",
            "toml",
            "yaml",
            "tag",
            "validate",
            "compare"
        ]
    )]
    create_avi: Vec<String>,

    /// Create a bounded Matroska metadata seed with Info/SimpleTag fields.
    #[arg(
        long = "create-mkv",
        value_name = "KEY=VALUE",
        action = clap::ArgAction::Append,
        conflicts_with_all = [
            "set",
            "delete",
            "copy",
            "create_tiff",
            "create_jpeg",
            "create_pdf",
            "create_avi",
            "create_png",
            "create_xmp",
            "create_wav",
            "create_flac",
            "create_gif",
            "create_icc",
            "create_mp3",
            "create_ogg",
            "create_svg",
            "create_webp_xmp",
            "create_webm",
            "json",
            "jsonl",
            "csv",
            "toml",
            "yaml",
            "tag",
            "validate",
            "compare"
        ]
    )]
    create_mkv: Vec<String>,

    /// Create a bounded WebM metadata seed with Info/SimpleTag fields.
    #[arg(
        long = "create-webm",
        value_name = "KEY=VALUE",
        action = clap::ArgAction::Append,
        conflicts_with_all = [
            "set",
            "delete",
            "copy",
            "create_tiff",
            "create_jpeg",
            "create_pdf",
            "create_avi",
            "create_mkv",
            "create_png",
            "create_xmp",
            "create_wav",
            "create_flac",
            "create_gif",
            "create_icc",
            "create_mp3",
            "create_ogg",
            "create_svg",
            "create_webp_xmp",
            "json",
            "jsonl",
            "csv",
            "toml",
            "yaml",
            "tag",
            "validate",
            "compare"
        ]
    )]
    create_webm: Vec<String>,

    /// Create a new minimal PNG with one or more tEXt metadata fields.
    #[arg(
        long = "create-png",
        value_name = "KEY=VALUE",
        action = clap::ArgAction::Append,
        conflicts_with_all = [
            "set",
            "delete",
            "copy",
            "create_tiff",
            "create_xmp",
            "create_flac",
            "create_gif",
            "create_wav",
            "create_icc",
            "create_mp3",
            "create_ogg",
            "create_svg",
            "json",
            "jsonl",
            "csv",
            "toml",
            "yaml",
            "validate",
            "compare"
        ]
    )]
    create_png: Vec<String>,

    /// Create a new standalone XMP packet from validated XML.
    #[arg(
        long = "create-xmp",
        value_name = "PACKET",
        conflicts_with_all = [
            "set",
            "delete",
            "copy",
            "create_tiff",
            "create_png",
            "create_flac",
            "create_gif",
            "create_wav",
            "create_icc",
            "create_mp3",
            "create_ogg",
            "create_svg",
            "json",
            "jsonl",
            "csv",
            "toml",
            "yaml",
            "tag",
            "validate",
            "compare"
        ]
    )]
    create_xmp: Option<String>,

    /// Create a new minimal PCM WAV with one or more LIST/INFO fields.
    #[arg(
        long = "create-wav",
        value_name = "KEY=VALUE",
        action = clap::ArgAction::Append,
        conflicts_with_all = [
            "set",
            "delete",
            "copy",
            "create_tiff",
            "create_png",
            "create_xmp",
            "create_flac",
            "create_gif",
            "create_icc",
            "create_mp3",
            "create_ogg",
            "create_svg",
            "json",
            "jsonl",
            "csv",
            "toml",
            "yaml",
            "tag",
            "validate",
            "compare"
        ]
    )]
    create_wav: Vec<String>,

    /// Create a new metadata-only FLAC stream with Vorbis comments.
    #[arg(
        long = "create-flac",
        value_name = "KEY=VALUE",
        action = clap::ArgAction::Append,
        conflicts_with_all = [
            "set",
            "delete",
            "copy",
            "create_tiff",
            "create_png",
            "create_xmp",
            "create_wav",
            "create_gif",
            "create_icc",
            "create_mp3",
            "create_ogg",
            "create_svg",
            "json",
            "jsonl",
            "csv",
            "toml",
            "yaml",
            "tag",
            "validate",
            "compare"
        ]
    )]
    create_flac: Vec<String>,

    /// Create a new minimal 1x1 GIF with one or more comment extensions.
    #[arg(
        long = "create-gif",
        value_name = "COMMENT",
        action = clap::ArgAction::Append,
        conflicts_with_all = [
            "set",
            "delete",
            "copy",
            "create_tiff",
            "create_png",
            "create_xmp",
            "create_wav",
            "create_flac",
            "create_icc",
            "create_mp3",
            "create_ogg",
            "create_svg",
            "json",
            "jsonl",
            "csv",
            "toml",
            "yaml",
            "tag",
            "validate",
            "compare"
        ]
    )]
    create_gif: Vec<String>,

    /// Create a new minimal RGB ICC profile with one or more text tags.
    #[arg(
        long = "create-icc",
        value_name = "KEY=VALUE",
        action = clap::ArgAction::Append,
        conflicts_with_all = [
            "set",
            "delete",
            "copy",
            "create_tiff",
            "create_png",
            "create_xmp",
            "create_flac",
            "create_wav",
            "create_gif",
            "create_mp3",
            "create_ogg",
            "create_svg",
            "json",
            "jsonl",
            "csv",
            "toml",
            "yaml",
            "tag",
            "validate",
            "compare"
        ]
    )]
    create_icc: Vec<String>,

    /// Create a new minimal MP3 seed with ID3v2.4 text frames.
    #[arg(
        long = "create-mp3",
        value_name = "KEY=VALUE",
        action = clap::ArgAction::Append,
        conflicts_with_all = [
            "set",
            "delete",
            "copy",
            "create_tiff",
            "create_png",
            "create_xmp",
            "create_wav",
            "create_flac",
            "create_gif",
            "create_icc",
            "create_ogg",
            "create_svg",
            "json",
            "jsonl",
            "csv",
            "toml",
            "yaml",
            "tag",
            "validate",
            "compare"
        ]
    )]
    create_mp3: Vec<String>,

    /// Create a new minimal Ogg Opus seed with Vorbis-style comments.
    #[arg(
        long = "create-ogg",
        value_name = "KEY=VALUE",
        action = clap::ArgAction::Append,
        conflicts_with_all = [
            "set",
            "delete",
            "copy",
            "create_tiff",
            "create_png",
            "create_xmp",
            "create_wav",
            "create_flac",
            "create_gif",
            "create_icc",
            "create_mp3",
            "json",
            "jsonl",
            "csv",
            "toml",
            "yaml",
            "tag",
            "validate",
            "compare"
        ]
    )]
    create_ogg: Vec<String>,

    /// Create a new minimal SVG document with bounded document text metadata.
    #[arg(
        long = "create-svg",
        value_name = "KEY=VALUE",
        action = clap::ArgAction::Append,
        conflicts_with_all = [
            "set",
            "delete",
            "copy",
            "create_tiff",
            "create_png",
            "create_xmp",
            "create_wav",
            "create_flac",
            "create_gif",
            "create_icc",
            "create_mp3",
            "create_ogg",
            "json",
            "jsonl",
            "csv",
            "toml",
            "yaml",
            "tag",
            "validate",
            "compare"
        ]
    )]
    create_svg: Vec<String>,

    /// Create a new minimal 1x1 lossless WebP with an optional XMP packet.
    #[arg(
        long = "create-webp-xmp",
        value_name = "PACKET",
        conflicts_with_all = [
            "set",
            "delete",
            "copy",
            "create_tiff",
            "create_png",
            "create_xmp",
            "create_wav",
            "create_flac",
            "create_gif",
            "create_icc",
            "create_mp3",
            "create_ogg",
            "create_svg",
            "json",
            "jsonl",
            "csv",
            "toml",
            "yaml",
            "tag",
            "validate",
            "compare"
        ]
    )]
    create_webp_xmp: Option<String>,

    /// Validate inputs and return a failure when any warning is produced.
    #[arg(long, conflicts_with_all = ["set", "delete", "copy"])]
    validate: bool,

    /// Compare each positional file against this reference file.
    #[arg(
        long,
        value_name = "REFERENCE",
        conflicts_with_all = [
            "set",
            "delete",
            "copy",
            "validate",
            "json",
            "jsonl",
            "csv",
            "toml",
            "yaml"
        ]
    )]
    compare: Option<PathBuf>,

    /// Traverse directories recursively in deterministic path order.
    #[arg(short = 'r', long)]
    recursive: bool,

    /// Maximum number of files to inspect concurrently.
    #[arg(long, default_value_t = 1, value_parser = parse_jobs)]
    jobs: usize,

    /// Maximum total metadata bytes materialized per file.
    #[arg(long, value_parser = parse_positive_usize)]
    max_metadata_bytes: Option<usize>,

    /// Maximum bytes materialized for one metadata value.
    #[arg(long, value_parser = parse_positive_usize)]
    max_value_bytes: Option<usize>,

    /// Files to inspect.
    #[arg(value_name = "FILE", required = true)]
    files: Vec<PathBuf>,
}

fn main() -> ExitCode {
    let arguments = Arguments::parse_from(normalize_legacy_args(env::args_os()));
    let limits = parse_limits(arguments.max_metadata_bytes, arguments.max_value_bytes);
    if !arguments.create_jpeg.is_empty() {
        if arguments.files.len() != 1 {
            eprintln!("metra: --create-jpeg requires exactly one destination path");
            return ExitCode::from(2);
        }
        let options = match parse_jpeg_create(&arguments.create_jpeg) {
            Ok(options) => options,
            Err(message) => {
                eprintln!("metra: {message}");
                return ExitCode::from(2);
            }
        };
        return match metra::create_jpeg_path(&arguments.files[0], &options, limits) {
            Ok(()) => {
                println!("created: {}", arguments.files[0].display());
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("metra: {}: {error}", arguments.files[0].display());
                ExitCode::from(1)
            }
        };
    }
    if !arguments.create_pdf.is_empty() {
        if arguments.files.len() != 1 {
            eprintln!("metra: --create-pdf requires exactly one destination path");
            return ExitCode::from(2);
        }
        let options = match parse_pdf_create(&arguments.create_pdf) {
            Ok(options) => options,
            Err(message) => {
                eprintln!("metra: {message}");
                return ExitCode::from(2);
            }
        };
        return match metra::create_pdf_path(&arguments.files[0], &options, limits) {
            Ok(()) => {
                println!("created: {}", arguments.files[0].display());
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("metra: {}: {error}", arguments.files[0].display());
                ExitCode::from(1)
            }
        };
    }
    if !arguments.create_psd.is_empty() {
        if arguments.files.len() != 1 {
            eprintln!("metra: --create-psd requires exactly one destination path");
            return ExitCode::from(2);
        }
        let options = match parse_psd_create(&arguments.create_psd) {
            Ok(options) => options,
            Err(message) => {
                eprintln!("metra: {message}");
                return ExitCode::from(2);
            }
        };
        return match metra::create_psd_path(&arguments.files[0], &options, limits) {
            Ok(()) => {
                println!("created: {}", arguments.files[0].display());
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("metra: {}: {error}", arguments.files[0].display());
                ExitCode::from(1)
            }
        };
    }
    if !arguments.create_avi.is_empty() {
        if arguments.files.len() != 1 {
            eprintln!("metra: --create-avi requires exactly one destination path");
            return ExitCode::from(2);
        }
        let options = match parse_avi_create(&arguments.create_avi) {
            Ok(options) => options,
            Err(message) => {
                eprintln!("metra: {message}");
                return ExitCode::from(2);
            }
        };
        return match metra::create_avi_path(&arguments.files[0], &options, limits) {
            Ok(()) => {
                println!("created: {}", arguments.files[0].display());
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("metra: {}: {error}", arguments.files[0].display());
                ExitCode::from(1)
            }
        };
    }
    if !arguments.create_mkv.is_empty() {
        return run_matroska_create(
            &arguments.files,
            &arguments.create_mkv,
            metra::MatroskaCreateKind::Mkv,
            limits,
        );
    }
    if !arguments.create_webm.is_empty() {
        return run_matroska_create(
            &arguments.files,
            &arguments.create_webm,
            metra::MatroskaCreateKind::Webm,
            limits,
        );
    }
    if !arguments.create_tiff.is_empty() {
        if arguments.files.len() != 1 {
            eprintln!("metra: --create-tiff requires exactly one destination path");
            return ExitCode::from(2);
        }
        let options = match parse_tiff_create(&arguments.create_tiff) {
            Ok(options) => options,
            Err(message) => {
                eprintln!("metra: {message}");
                return ExitCode::from(2);
            }
        };
        return match metra::create_tiff_path(&arguments.files[0], &options, limits) {
            Ok(()) => {
                println!("created: {}", arguments.files[0].display());
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("metra: {}: {error}", arguments.files[0].display());
                ExitCode::from(1)
            }
        };
    }
    if !arguments.create_png.is_empty() {
        if arguments.files.len() != 1 {
            eprintln!("metra: --create-png requires exactly one destination path");
            return ExitCode::from(2);
        }
        let options = match parse_png_create(&arguments.create_png) {
            Ok(options) => options,
            Err(message) => {
                eprintln!("metra: {message}");
                return ExitCode::from(2);
            }
        };
        return match metra::create_png_path(&arguments.files[0], &options, limits) {
            Ok(()) => {
                println!("created: {}", arguments.files[0].display());
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("metra: {}: {error}", arguments.files[0].display());
                ExitCode::from(1)
            }
        };
    }
    if let Some(packet) = arguments.create_xmp.as_deref() {
        if arguments.files.len() != 1 {
            eprintln!("metra: --create-xmp requires exactly one destination path");
            return ExitCode::from(2);
        }
        return match metra::create_xmp_path(&arguments.files[0], packet, limits) {
            Ok(()) => {
                println!("created: {}", arguments.files[0].display());
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("metra: {}: {error}", arguments.files[0].display());
                ExitCode::from(1)
            }
        };
    }
    if !arguments.create_wav.is_empty() {
        if arguments.files.len() != 1 {
            eprintln!("metra: --create-wav requires exactly one destination path");
            return ExitCode::from(2);
        }
        let options = match parse_wav_create(&arguments.create_wav) {
            Ok(options) => options,
            Err(message) => {
                eprintln!("metra: {message}");
                return ExitCode::from(2);
            }
        };
        return match metra::create_wav_path(&arguments.files[0], &options, limits) {
            Ok(()) => {
                println!("created: {}", arguments.files[0].display());
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("metra: {}: {error}", arguments.files[0].display());
                ExitCode::from(1)
            }
        };
    }
    if !arguments.create_flac.is_empty() {
        if arguments.files.len() != 1 {
            eprintln!("metra: --create-flac requires exactly one destination path");
            return ExitCode::from(2);
        }
        let options = match parse_flac_create(&arguments.create_flac) {
            Ok(options) => options,
            Err(message) => {
                eprintln!("metra: {message}");
                return ExitCode::from(2);
            }
        };
        return match metra::create_flac_path(&arguments.files[0], &options, limits) {
            Ok(()) => {
                println!("created: {}", arguments.files[0].display());
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("metra: {}: {error}", arguments.files[0].display());
                ExitCode::from(1)
            }
        };
    }
    if !arguments.create_gif.is_empty() {
        if arguments.files.len() != 1 {
            eprintln!("metra: --create-gif requires exactly one destination path");
            return ExitCode::from(2);
        }
        let options = match parse_gif_create(&arguments.create_gif) {
            Ok(options) => options,
            Err(message) => {
                eprintln!("metra: {message}");
                return ExitCode::from(2);
            }
        };
        return match metra::create_gif_path(&arguments.files[0], &options, limits) {
            Ok(()) => {
                println!("created: {}", arguments.files[0].display());
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("metra: {}: {error}", arguments.files[0].display());
                ExitCode::from(1)
            }
        };
    }
    if !arguments.create_icc.is_empty() {
        if arguments.files.len() != 1 {
            eprintln!("metra: --create-icc requires exactly one destination path");
            return ExitCode::from(2);
        }
        let options = match parse_icc_create(&arguments.create_icc) {
            Ok(options) => options,
            Err(message) => {
                eprintln!("metra: {message}");
                return ExitCode::from(2);
            }
        };
        return match metra::create_icc_path(&arguments.files[0], &options, limits) {
            Ok(()) => {
                println!("created: {}", arguments.files[0].display());
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("metra: {}: {error}", arguments.files[0].display());
                ExitCode::from(1)
            }
        };
    }
    if !arguments.create_mp3.is_empty() {
        if arguments.files.len() != 1 {
            eprintln!("metra: --create-mp3 requires exactly one destination path");
            return ExitCode::from(2);
        }
        let options = match parse_mp3_create(&arguments.create_mp3) {
            Ok(options) => options,
            Err(message) => {
                eprintln!("metra: {message}");
                return ExitCode::from(2);
            }
        };
        return match metra::create_mp3_path(&arguments.files[0], &options, limits) {
            Ok(()) => {
                println!("created: {}", arguments.files[0].display());
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("metra: {}: {error}", arguments.files[0].display());
                ExitCode::from(1)
            }
        };
    }
    if !arguments.create_ogg.is_empty() {
        if arguments.files.len() != 1 {
            eprintln!("metra: --create-ogg requires exactly one destination path");
            return ExitCode::from(2);
        }
        let options = match parse_ogg_create(&arguments.create_ogg) {
            Ok(options) => options,
            Err(message) => {
                eprintln!("metra: {message}");
                return ExitCode::from(2);
            }
        };
        return match metra::create_ogg_path(&arguments.files[0], &options, limits) {
            Ok(()) => {
                println!("created: {}", arguments.files[0].display());
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("metra: {}: {error}", arguments.files[0].display());
                ExitCode::from(1)
            }
        };
    }
    if !arguments.create_svg.is_empty() {
        if arguments.files.len() != 1 {
            eprintln!("metra: --create-svg requires exactly one destination path");
            return ExitCode::from(2);
        }
        let options = match parse_svg_create(&arguments.create_svg) {
            Ok(options) => options,
            Err(message) => {
                eprintln!("metra: {message}");
                return ExitCode::from(2);
            }
        };
        return match metra::create_svg_path(&arguments.files[0], &options, limits) {
            Ok(()) => {
                println!("created: {}", arguments.files[0].display());
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("metra: {}: {error}", arguments.files[0].display());
                ExitCode::from(1)
            }
        };
    }
    if let Some(packet) = arguments.create_webp_xmp.as_deref() {
        if arguments.files.len() != 1 {
            eprintln!("metra: --create-webp-xmp requires exactly one destination path");
            return ExitCode::from(2);
        }
        let options = metra::WebpCreateOptions::new().with_xmp(packet);
        return match metra::create_webp_path(&arguments.files[0], &options, limits) {
            Ok(()) => {
                println!("created: {}", arguments.files[0].display());
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("metra: {}: {error}", arguments.files[0].display());
                ExitCode::from(1)
            }
        };
    }
    let paths = match collect_paths(&arguments.files, arguments.recursive) {
        Ok(paths) => paths,
        Err(message) => {
            eprintln!("metra: {message}");
            return ExitCode::from(2);
        }
    };

    let request = match parse_edits(
        arguments.set.as_deref(),
        arguments.delete.as_deref(),
        arguments.copy.as_deref(),
    ) {
        Ok(request) => request,
        Err(message) => {
            eprintln!("metra: {message}");
            return ExitCode::from(2);
        }
    };
    if let Some(request) = request {
        return apply_request(&paths, request, limits);
    }

    let cancellation = metra::CancellationToken::new();
    let handler_token = cancellation.clone();
    if let Err(error) = ctrlc::set_handler(move || handler_token.cancel()) {
        eprintln!("metra: cannot install Ctrl+C handler: {error}");
        return ExitCode::from(2);
    }
    if let Some(reference) = arguments.compare.as_deref() {
        return compare_paths(&paths, reference, limits, &cancellation);
    }

    let failures = if arguments.json || arguments.toml || arguments.yaml {
        let results = filter_results(
            inspect_paths(&paths, arguments.jobs, limits, &cancellation),
            &arguments.tag,
        );
        let failures = results
            .iter()
            .filter(|(_, result)| result_is_failure(result, arguments.validate))
            .count();
        if arguments.toml {
            emit_toml(&results);
        } else if arguments.yaml {
            emit_yaml(&results);
        } else {
            emit_json(&results, false);
        }
        failures
    } else {
        if arguments.csv {
            println!("path,format,namespace,group,id,name,value_type,value");
        }
        inspect_paths_streaming(
            &paths,
            arguments.jobs,
            arguments.validate,
            limits,
            &cancellation,
            |path, result| {
                let result = filter_result(result, &arguments.tag);
                if arguments.csv {
                    emit_csv_record(&path, &result);
                } else if arguments.jsonl {
                    emit_jsonl_record(&path, &result);
                } else if let Ok(metadata) = &result {
                    print_human(metadata);
                } else if let Err(error) = &result {
                    eprintln!("metra: {}: {error}", path.display());
                }
            },
        )
    };

    if cancellation.is_cancelled() {
        ExitCode::from(130)
    } else if failures == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn normalize_legacy_args<I>(args: I) -> Vec<OsString>
where
    I: IntoIterator<Item = OsString>,
{
    let mut normalized = Vec::new();
    let mut after_separator = false;
    for arg in args {
        if !after_separator && let Some(value) = arg.to_str() {
            if value == "--" {
                after_separator = true;
                normalized.push(arg);
                continue;
            }
            if value == "-json" {
                normalized.push(OsString::from("--json"));
                continue;
            }
            if value == "-jsonl" {
                normalized.push(OsString::from("--jsonl"));
                continue;
            }
            if let Some(tag) = legacy_tag_name(value) {
                normalized.push(OsString::from("--tag"));
                normalized.push(OsString::from(tag));
                continue;
            }
        }
        normalized.push(arg);
    }
    normalized
}

fn legacy_tag_name(value: &str) -> Option<&'static str> {
    match value {
        "-Make" => Some("Make"),
        "-Model" => Some("Model"),
        "-Artist" => Some("Artist"),
        "-Copyright" => Some("Copyright"),
        "-Software" => Some("Software"),
        "-ImageDescription" => Some("ImageDescription"),
        "-GPSLatitude" => Some("GPSLatitude"),
        "-GPSLongitude" => Some("GPSLongitude"),
        "-GPSAltitude" => Some("GPSAltitude"),
        "-DateTimeOriginal" => Some("DateTimeOriginal"),
        _ => None,
    }
}

fn filter_results(
    results: Vec<(PathBuf, metra::Result<Metadata>)>,
    selectors: &[String],
) -> Vec<(PathBuf, metra::Result<Metadata>)> {
    results
        .into_iter()
        .map(|(path, result)| (path, filter_result(result, selectors)))
        .collect()
}

fn filter_result(result: metra::Result<Metadata>, selectors: &[String]) -> metra::Result<Metadata> {
    result.map(|mut metadata| {
        if !selectors.is_empty() {
            metadata.tags.retain(|tag| {
                let key = tag.key();
                selectors
                    .iter()
                    .any(|selector| selector == &key || selector == &tag.name)
            });
        }
        metadata
    })
}

fn run_matroska_create(
    files: &[PathBuf],
    entries: &[String],
    kind: metra::MatroskaCreateKind,
    limits: ParseLimits,
) -> ExitCode {
    let option_name = match kind {
        metra::MatroskaCreateKind::Mkv => "--create-mkv",
        metra::MatroskaCreateKind::Webm => "--create-webm",
    };
    if files.len() != 1 {
        eprintln!("metra: {option_name} requires exactly one destination path");
        return ExitCode::from(2);
    }
    let options = match parse_matroska_create(entries, kind) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("metra: {message}");
            return ExitCode::from(2);
        }
    };
    match metra::create_matroska_path(&files[0], &options, limits) {
        Ok(()) => {
            println!("created: {}", files[0].display());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("metra: {}: {error}", files[0].display());
            ExitCode::from(1)
        }
    }
}

fn parse_matroska_create(
    entries: &[String],
    kind: metra::MatroskaCreateKind,
) -> Result<metra::MatroskaCreateOptions, String> {
    let option_name = match kind {
        metra::MatroskaCreateKind::Mkv => "--create-mkv",
        metra::MatroskaCreateKind::Webm => "--create-webm",
    };
    let mut options = metra::MatroskaCreateOptions::new(kind);
    for assignment in entries {
        let (raw_key, value) = assignment
            .split_once('=')
            .ok_or_else(|| format!("{option_name} expects KEY=VALUE"))?;
        let key = raw_key.strip_prefix("Matroska:").unwrap_or(raw_key);
        if key.is_empty() {
            return Err(format!("{option_name} requires a non-empty KEY"));
        }
        if let Some(name) = key.strip_prefix("Tag:") {
            if name.is_empty() {
                return Err(format!("{option_name} Tag requires a non-empty name"));
            }
            options.push_tag(name, value);
        } else if matches!(key, "Title" | "MuxingApp" | "WritingApp") {
            options.push_info(key, value);
        } else {
            return Err(format!(
                "unsupported {option_name} field {raw_key}; use Title, MuxingApp, WritingApp, or Tag:<name>"
            ));
        }
    }
    Ok(options)
}

fn parse_tiff_create(entries: &[String]) -> Result<metra::TiffCreateOptions, String> {
    let mut options = metra::TiffCreateOptions::new();
    for assignment in entries {
        let (key, value) = assignment
            .split_once('=')
            .ok_or_else(|| "--create-tiff expects KEY=VALUE".to_owned())?;
        if key.is_empty() {
            return Err("--create-tiff requires a non-empty KEY".to_owned());
        }
        options.push_ascii(key, value);
    }
    Ok(options)
}

fn parse_jpeg_create(entries: &[String]) -> Result<metra::JpegCreateOptions, String> {
    let mut options = metra::JpegCreateOptions::new();
    for assignment in entries {
        let (key, value) = assignment
            .split_once('=')
            .ok_or_else(|| "--create-jpeg expects KEY=VALUE".to_owned())?;
        let key = key.strip_prefix("JPEG:").unwrap_or(key);
        match key {
            "Comment" => {
                if options.comment.is_some() {
                    return Err("--create-jpeg accepts only one Comment field".to_owned());
                }
                options.set_comment(value);
            }
            "XMP" => {
                if options.xmp.is_some() {
                    return Err("--create-jpeg accepts only one XMP field".to_owned());
                }
                options.set_xmp(value);
            }
            _ => return Err(format!("unsupported --create-jpeg field {key}")),
        }
    }
    Ok(options)
}

fn parse_pdf_create(entries: &[String]) -> Result<metra::PdfCreateOptions, String> {
    let mut options = metra::PdfCreateOptions::new();
    for assignment in entries {
        let (key, value) = assignment
            .split_once('=')
            .ok_or_else(|| "--create-pdf expects KEY=VALUE".to_owned())?;
        let key = key.strip_prefix("PDF:").unwrap_or(key);
        if key.is_empty() {
            return Err("--create-pdf requires a non-empty KEY".to_owned());
        }
        options.push_info(key, value);
    }
    Ok(options)
}

fn parse_psd_create(entries: &[String]) -> Result<metra::PsdCreateOptions, String> {
    let mut options = metra::PsdCreateOptions::new();
    for assignment in entries {
        let (key, value) = assignment
            .split_once('=')
            .ok_or_else(|| "--create-psd expects XMP=PACKET".to_owned())?;
        let key = key.strip_prefix("PSD:").unwrap_or(key);
        if key != "XMP" {
            return Err(format!(
                "unsupported --create-psd field {key}; use XMP=PACKET"
            ));
        }
        if options.xmp.is_some() {
            return Err("--create-psd accepts only one XMP field".to_owned());
        }
        options.set_xmp(value);
    }
    Ok(options)
}

fn parse_png_create(entries: &[String]) -> Result<metra::PngCreateOptions, String> {
    let mut options = metra::PngCreateOptions::new();
    for assignment in entries {
        let (keyword, value) = assignment
            .split_once('=')
            .ok_or_else(|| "--create-png expects KEY=VALUE".to_owned())?;
        if keyword.is_empty() {
            return Err("--create-png requires a non-empty KEY".to_owned());
        }
        options.push_text(keyword, value);
    }
    Ok(options)
}

fn parse_wav_create(entries: &[String]) -> Result<metra::WavCreateOptions, String> {
    let mut options = metra::WavCreateOptions::new();
    for assignment in entries {
        let (key, value) = assignment
            .split_once('=')
            .ok_or_else(|| "--create-wav expects KEY=VALUE".to_owned())?;
        if key.is_empty() {
            return Err("--create-wav requires a non-empty KEY".to_owned());
        }
        options.push_info(key, value);
    }
    Ok(options)
}

fn parse_flac_create(entries: &[String]) -> Result<metra::FlacCreateOptions, String> {
    let mut options = metra::FlacCreateOptions::new();
    for assignment in entries {
        let (key, value) = assignment
            .split_once('=')
            .ok_or_else(|| "--create-flac expects KEY=VALUE".to_owned())?;
        if key.is_empty() {
            return Err("--create-flac requires a non-empty KEY".to_owned());
        }
        options.push_comment(key, value);
    }
    Ok(options)
}

fn parse_gif_create(entries: &[String]) -> Result<metra::GifCreateOptions, String> {
    let mut options = metra::GifCreateOptions::new();
    for comment in entries {
        options.push_comment(comment);
    }
    Ok(options)
}

fn parse_icc_create(entries: &[String]) -> Result<metra::IccCreateOptions, String> {
    let mut options = metra::IccCreateOptions::new();
    for assignment in entries {
        let (key, value) = assignment
            .split_once('=')
            .ok_or_else(|| "--create-icc expects KEY=VALUE".to_owned())?;
        if key.is_empty() {
            return Err("--create-icc requires a non-empty KEY".to_owned());
        }
        options.push_text(key, value);
    }
    Ok(options)
}

fn parse_avi_create(entries: &[String]) -> Result<metra::AviCreateOptions, String> {
    let mut options = metra::AviCreateOptions::new();
    for assignment in entries {
        let (key, value) = assignment
            .split_once('=')
            .ok_or_else(|| "--create-avi expects KEY=VALUE".to_owned())?;
        let key = key.strip_prefix("AVI:").unwrap_or(key);
        if key.is_empty() {
            return Err("--create-avi requires a non-empty KEY".to_owned());
        }
        options.push_info(key, value);
    }
    Ok(options)
}

fn parse_mp3_create(entries: &[String]) -> Result<metra::Mp3CreateOptions, String> {
    let mut options = metra::Mp3CreateOptions::new();
    for assignment in entries {
        let (key, value) = assignment
            .split_once('=')
            .ok_or_else(|| "--create-mp3 expects KEY=VALUE".to_owned())?;
        let key = key.strip_prefix("ID3:").unwrap_or(key);
        if key.is_empty() {
            return Err("--create-mp3 requires a non-empty KEY".to_owned());
        }
        if key == "Comment" {
            if options.comment.is_some() {
                return Err("--create-mp3 accepts only one Comment field".to_owned());
            }
            options.set_comment(value);
        } else {
            options.push_text(key, value);
        }
    }
    Ok(options)
}

fn parse_ogg_create(entries: &[String]) -> Result<metra::OggCreateOptions, String> {
    let mut options = metra::OggCreateOptions::new();
    for assignment in entries {
        let (key, value) = assignment
            .split_once('=')
            .ok_or_else(|| "--create-ogg expects KEY=VALUE".to_owned())?;
        if key.is_empty() {
            return Err("--create-ogg requires a non-empty KEY".to_owned());
        }
        options.push_comment(key, value);
    }
    Ok(options)
}

fn parse_svg_create(entries: &[String]) -> Result<metra::SvgCreateOptions, String> {
    let mut options = metra::SvgCreateOptions::new();
    for assignment in entries {
        let (key, value) = assignment
            .split_once('=')
            .ok_or_else(|| "--create-svg expects KEY=VALUE".to_owned())?;
        let key = key.strip_prefix("SVG:").unwrap_or(key);
        if key.is_empty() {
            return Err("--create-svg requires a non-empty KEY".to_owned());
        }
        match key {
            "Title" => {
                if options.title.is_some() {
                    return Err("--create-svg accepts only one Title field".to_owned());
                }
                options.set_title(value);
            }
            "Description" => {
                if options.description.is_some() {
                    return Err("--create-svg accepts only one Description field".to_owned());
                }
                options.set_description(value);
            }
            "Comment" => options.push_comment(value),
            _ => return Err(format!("unsupported --create-svg field {key}")),
        }
    }
    Ok(options)
}

fn compare_paths(
    paths: &[PathBuf],
    reference: &Path,
    limits: ParseLimits,
    cancellation: &metra::CancellationToken,
) -> ExitCode {
    let baseline = match metra::read_with_limits(reference, limits) {
        Ok(metadata) => metadata,
        Err(error) => {
            eprintln!("metra: {}: {error}", reference.display());
            return ExitCode::from(1);
        }
    };
    let mut failures = 0_usize;
    for path in paths {
        if cancellation.is_cancelled() {
            eprintln!("metra: operation cancelled");
            return ExitCode::from(130);
        }
        match metra::read_with_limits(path, limits) {
            Ok(metadata) => {
                let diff = baseline.diff(&metadata);
                print_diff(path, reference, &diff);
                if !diff.added.is_empty() || !diff.removed.is_empty() || !diff.changed.is_empty() {
                    failures += 1;
                }
            }
            Err(error) => {
                eprintln!("metra: {}: {error}", path.display());
                failures += 1;
            }
        }
    }
    if failures == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn print_diff(path: &Path, reference: &Path, diff: &metra::MetadataDiff) {
    println!("{} (against {})", path.display(), reference.display());
    for tag in &diff.added {
        println!("  added: {} = {}", tag.key(), tag.display_value());
    }
    for tag in &diff.removed {
        println!("  removed: {} = {}", tag.key(), tag.display_value());
    }
    for change in &diff.changed {
        println!(
            "  changed: {}: {} -> {}",
            change.key,
            display_values(&change.before),
            display_values(&change.after)
        );
    }
    if diff.added.is_empty() && diff.removed.is_empty() && diff.changed.is_empty() {
        println!("  no metadata differences");
    }
}

fn display_values(values: &[metra::TagValue]) -> String {
    values
        .iter()
        .map(metra::TagValue::to_display_string)
        .collect::<Vec<_>>()
        .join(" | ")
}

enum EditRequest {
    DirectJpeg(Vec<metra::JpegEdit>),
    DirectTiff(Vec<metra::TiffEdit>),
    DirectIsobmff(Vec<metra::IsobmffEdit>),
    DirectPng(Vec<metra::PngEdit>),
    DirectWav(Vec<metra::WavEdit>),
    DirectFlac(Vec<metra::FlacEdit>),
    DirectOgg(Vec<metra::OggEdit>),
    DirectPdf(Vec<metra::PdfEdit>),
    DirectPsd(Vec<metra::PsdEdit>),
    DirectIcc(Vec<metra::IccEdit>),
    DirectAvi(Vec<metra::AviEdit>),
    DirectMatroska(Vec<metra::MatroskaEdit>),
    DirectMp3(Vec<metra::Mp3Edit>),
    DirectGif(Vec<metra::GifEdit>),
    DirectWebp(Vec<metra::WebpEdit>),
    DirectSvg(Vec<metra::SvgEdit>),
    DirectXmp(Vec<metra::XmpEdit>),
    Copy { key: CopyKey, source: PathBuf },
}

#[derive(Debug)]
enum CopyKey {
    JpegComment,
    JpegExifAscii(String),
    JpegXmp,
    JpegIptc(String),
    TiffAscii(String),
    IsobmffText(String),
    PngText(String),
    PngXmp,
    WavInfo(String),
    FlacComment(String),
    OggComment(String),
    PdfInfo(String),
    IccText(String),
    PsdXmp,
    AviInfo(String),
    MatroskaTag(String),
    MatroskaString(String),
    Mp3Text(String),
    Mp3Comment,
    GifComment,
    WebpXmp,
    SvgText(SvgTextKey),
    XmpPacket,
}

#[derive(Debug)]
enum SvgTextKey {
    Title,
    Description,
    Comment,
}

fn parse_edits(
    set: Option<&str>,
    delete: Option<&str>,
    copy: Option<&str>,
) -> Result<Option<EditRequest>, String> {
    if let Some(assignment) = set {
        let (key, value) = assignment
            .split_once('=')
            .ok_or_else(|| "--set expects KEY=VALUE".to_owned())?;
        if key != "JPEG:Comment" {
            if let Some(exif_key) = jpeg_exif_ascii_key(key) {
                return Ok(Some(EditRequest::DirectJpeg(vec![
                    metra::JpegEdit::SetExifAscii {
                        key: exif_key.to_owned(),
                        value: value.to_owned(),
                    },
                ])));
            }
            if let Some(tiff_key) = tiff_ascii_key(key) {
                return Ok(Some(EditRequest::DirectTiff(vec![
                    metra::TiffEdit::SetAscii {
                        key: tiff_key.to_owned(),
                        value: value.to_owned(),
                    },
                ])));
            }
            if let Some(name) = pdf_info_name(key) {
                return Ok(Some(EditRequest::DirectPdf(vec![
                    metra::PdfEdit::SetInfo {
                        name: name.to_owned(),
                        value: value.to_owned(),
                    },
                ])));
            }
            if let Some(name) = icc_text_name(key) {
                return Ok(Some(EditRequest::DirectIcc(vec![
                    metra::IccEdit::SetText {
                        name: name.to_owned(),
                        value: value.to_owned(),
                    },
                ])));
            }
            if psd_xmp_key(key) {
                return Ok(Some(EditRequest::DirectPsd(vec![metra::PsdEdit::SetXmp(
                    value.to_owned(),
                )])));
            }
            if key == "XMP:Packet" {
                return Ok(Some(EditRequest::DirectXmp(vec![
                    metra::XmpEdit::SetPacket(value.to_owned()),
                ])));
            }
            if let Some(name) = avi_info_name(key) {
                return Ok(Some(EditRequest::DirectAvi(vec![
                    metra::AviEdit::SetInfo {
                        name: name.to_owned(),
                        value: value.to_owned(),
                    },
                ])));
            }
            if let Some(name) = matroska_tag_name(key) {
                return Ok(Some(EditRequest::DirectMatroska(vec![
                    metra::MatroskaEdit::SetTag {
                        name: name.to_owned(),
                        value: value.to_owned(),
                    },
                ])));
            }
            if matroska_info_key(key) {
                return Ok(Some(EditRequest::DirectMatroska(vec![
                    metra::MatroskaEdit::SetString {
                        key: key.to_owned(),
                        value: value.to_owned(),
                    },
                ])));
            }
            if isobmff_text_key(key) {
                return Ok(Some(EditRequest::DirectIsobmff(vec![
                    metra::IsobmffEdit::SetText {
                        key: key.to_owned(),
                        value: value.to_owned(),
                    },
                ])));
            }
            if jpeg_xmp_key(key) {
                return Ok(Some(EditRequest::DirectJpeg(vec![
                    metra::JpegEdit::SetXmp(value.to_owned()),
                ])));
            }
            if let Some(name) = iptc_name(key) {
                return Ok(Some(EditRequest::DirectJpeg(vec![
                    metra::JpegEdit::SetIptc {
                        name: name.to_owned(),
                        value: value.to_owned(),
                    },
                ])));
            }
            if let Some(edit) = svg_set_edit(key, value) {
                return Ok(Some(EditRequest::DirectSvg(vec![edit])));
            }
            if png_xmp_key(key) {
                return Ok(Some(EditRequest::DirectPng(vec![metra::PngEdit::SetXmp(
                    value.to_owned(),
                )])));
            }
            if let Some(keyword) = png_text_keyword(key) {
                return Ok(Some(EditRequest::DirectPng(vec![
                    metra::PngEdit::SetText {
                        keyword: keyword.to_owned(),
                        value: value.to_owned(),
                    },
                ])));
            }
            if let Some(name) = wav_info_name(key) {
                return Ok(Some(EditRequest::DirectWav(vec![
                    metra::WavEdit::SetInfo {
                        name: name.to_owned(),
                        value: value.to_owned(),
                    },
                ])));
            }
            if let Some(name) = flac_comment_name(key) {
                return Ok(Some(EditRequest::DirectFlac(vec![
                    metra::FlacEdit::SetComment {
                        key: name.to_owned(),
                        value: value.to_owned(),
                    },
                ])));
            }
            if let Some(name) = ogg_comment_name(key) {
                return Ok(Some(EditRequest::DirectOgg(vec![
                    metra::OggEdit::SetComment {
                        key: name.to_owned(),
                        value: value.to_owned(),
                    },
                ])));
            }
            if key == "ID3:Comment" {
                return Ok(Some(EditRequest::DirectMp3(vec![
                    metra::Mp3Edit::SetComment(value.to_owned()),
                ])));
            }
            if let Some(name) = mp3_text_name(key) {
                return Ok(Some(EditRequest::DirectMp3(vec![
                    metra::Mp3Edit::SetText {
                        name: name.to_owned(),
                        value: value.to_owned(),
                    },
                ])));
            }
            if key == "GIF:Comment" {
                return Ok(Some(EditRequest::DirectGif(vec![
                    metra::GifEdit::SetComment(value.to_owned()),
                ])));
            }
            if webp_xmp_key(key) {
                return Ok(Some(EditRequest::DirectWebp(vec![
                    metra::WebpEdit::SetXmp(value.to_owned()),
                ])));
            }
            return Err(unsupported_edit_message(key));
        }
        return Ok(Some(EditRequest::DirectJpeg(vec![
            metra::JpegEdit::SetComment(value.to_owned()),
        ])));
    }
    if let Some(key) = delete {
        if key != "JPEG:Comment" {
            if let Some(tiff_key) = tiff_ascii_key(key) {
                return Ok(Some(EditRequest::DirectTiff(vec![
                    metra::TiffEdit::DeleteAscii {
                        key: tiff_key.to_owned(),
                    },
                ])));
            }
            if let Some(name) = pdf_info_name(key) {
                return Ok(Some(EditRequest::DirectPdf(vec![
                    metra::PdfEdit::DeleteInfo {
                        name: name.to_owned(),
                    },
                ])));
            }
            if let Some(name) = icc_text_name(key) {
                return Ok(Some(EditRequest::DirectIcc(vec![
                    metra::IccEdit::DeleteText {
                        name: name.to_owned(),
                    },
                ])));
            }
            if psd_xmp_key(key) {
                return Ok(Some(EditRequest::DirectPsd(vec![
                    metra::PsdEdit::DeleteXmp,
                ])));
            }
            if key == "XMP:Packet" {
                return Ok(Some(EditRequest::DirectXmp(vec![
                    metra::XmpEdit::DeletePacket,
                ])));
            }
            if jpeg_xmp_key(key) {
                return Ok(Some(EditRequest::DirectJpeg(vec![
                    metra::JpegEdit::DeleteXmp,
                ])));
            }
            if let Some(name) = iptc_name(key) {
                return Ok(Some(EditRequest::DirectJpeg(vec![
                    metra::JpegEdit::DeleteIptc {
                        name: name.to_owned(),
                    },
                ])));
            }
            if let Some(edit) = svg_delete_edit(key) {
                return Ok(Some(EditRequest::DirectSvg(vec![edit])));
            }
            if png_xmp_key(key) {
                return Ok(Some(EditRequest::DirectPng(vec![
                    metra::PngEdit::DeleteXmp,
                ])));
            }
            if let Some(keyword) = png_text_keyword(key) {
                return Ok(Some(EditRequest::DirectPng(vec![
                    metra::PngEdit::DeleteText {
                        keyword: keyword.to_owned(),
                    },
                ])));
            }
            if let Some(name) = wav_info_name(key) {
                return Ok(Some(EditRequest::DirectWav(vec![
                    metra::WavEdit::DeleteInfo {
                        name: name.to_owned(),
                    },
                ])));
            }
            if let Some(name) = avi_info_name(key) {
                return Ok(Some(EditRequest::DirectAvi(vec![
                    metra::AviEdit::DeleteInfo {
                        name: name.to_owned(),
                    },
                ])));
            }
            if let Some(name) = matroska_tag_name(key) {
                return Ok(Some(EditRequest::DirectMatroska(vec![
                    metra::MatroskaEdit::DeleteTag {
                        name: name.to_owned(),
                    },
                ])));
            }
            if matroska_info_key(key) {
                return Ok(Some(EditRequest::DirectMatroska(vec![
                    metra::MatroskaEdit::DeleteString {
                        key: key.to_owned(),
                    },
                ])));
            }
            if isobmff_text_key(key) {
                return Ok(Some(EditRequest::DirectIsobmff(vec![
                    metra::IsobmffEdit::DeleteText {
                        key: key.to_owned(),
                    },
                ])));
            }
            if let Some(name) = flac_comment_name(key) {
                return Ok(Some(EditRequest::DirectFlac(vec![
                    metra::FlacEdit::DeleteComment {
                        key: name.to_owned(),
                    },
                ])));
            }
            if let Some(name) = ogg_comment_name(key) {
                return Ok(Some(EditRequest::DirectOgg(vec![
                    metra::OggEdit::DeleteComment {
                        key: name.to_owned(),
                    },
                ])));
            }
            if key == "ID3:Comment" {
                return Ok(Some(EditRequest::DirectMp3(vec![
                    metra::Mp3Edit::DeleteComments,
                ])));
            }
            if let Some(name) = mp3_text_name(key) {
                return Ok(Some(EditRequest::DirectMp3(vec![
                    metra::Mp3Edit::DeleteText {
                        name: name.to_owned(),
                    },
                ])));
            }
            if key == "GIF:Comment" {
                return Ok(Some(EditRequest::DirectGif(vec![
                    metra::GifEdit::DeleteComments,
                ])));
            }
            if webp_xmp_key(key) {
                return Ok(Some(EditRequest::DirectWebp(vec![
                    metra::WebpEdit::DeleteXmp,
                ])));
            }
            return Err(unsupported_edit_message(key));
        }
        return Ok(Some(EditRequest::DirectJpeg(vec![
            metra::JpegEdit::DeleteComments,
        ])));
    }
    if let Some(assignment) = copy {
        let (key, source) = assignment
            .split_once('=')
            .ok_or_else(|| "--copy expects KEY=SOURCE".to_owned())?;
        if source.is_empty() {
            return Err("--copy requires a non-empty source path".to_owned());
        }
        let key = if key == "JPEG:Comment" {
            CopyKey::JpegComment
        } else if let Some(exif_key) = jpeg_exif_ascii_key(key) {
            CopyKey::JpegExifAscii(exif_key.to_owned())
        } else if let Some(tiff_key) = tiff_ascii_key(key) {
            CopyKey::TiffAscii(tiff_key.to_owned())
        } else if let Some(name) = pdf_info_name(key) {
            CopyKey::PdfInfo(name.to_owned())
        } else if let Some(name) = icc_text_name(key) {
            CopyKey::IccText(name.to_owned())
        } else if key == "XMP:Packet" {
            CopyKey::XmpPacket
        } else if psd_xmp_key(key) {
            CopyKey::PsdXmp
        } else if let Some(name) = avi_info_name(key) {
            CopyKey::AviInfo(name.to_owned())
        } else if let Some(name) = matroska_tag_name(key) {
            CopyKey::MatroskaTag(name.to_owned())
        } else if matroska_info_key(key) {
            CopyKey::MatroskaString(key.to_owned())
        } else if isobmff_text_key(key) {
            CopyKey::IsobmffText(key.to_owned())
        } else if jpeg_xmp_key(key) {
            CopyKey::JpegXmp
        } else if let Some(name) = iptc_name(key) {
            CopyKey::JpegIptc(name.to_owned())
        } else if let Some(svg_key) = svg_text_key(key) {
            CopyKey::SvgText(svg_key)
        } else if png_xmp_key(key) {
            CopyKey::PngXmp
        } else if let Some(keyword) = png_text_keyword(key) {
            CopyKey::PngText(keyword.to_owned())
        } else if let Some(name) = wav_info_name(key) {
            CopyKey::WavInfo(name.to_owned())
        } else if let Some(name) = flac_comment_name(key) {
            CopyKey::FlacComment(name.to_owned())
        } else if let Some(name) = ogg_comment_name(key) {
            CopyKey::OggComment(name.to_owned())
        } else if key == "ID3:Comment" {
            CopyKey::Mp3Comment
        } else if let Some(name) = mp3_text_name(key) {
            CopyKey::Mp3Text(name.to_owned())
        } else if key == "GIF:Comment" {
            CopyKey::GifComment
        } else if webp_xmp_key(key) {
            CopyKey::WebpXmp
        } else {
            return Err(unsupported_edit_message(key));
        };
        return Ok(Some(EditRequest::Copy {
            key,
            source: PathBuf::from(source),
        }));
    }
    Ok(None)
}

fn png_text_keyword(key: &str) -> Option<&str> {
    let keyword = key.strip_prefix("PNG:Text:")?;
    (!keyword.is_empty()).then_some(keyword)
}

fn tiff_ascii_key(key: &str) -> Option<&str> {
    let key = key.strip_prefix("TIFF:")?;
    ["EXIF:", "GPS:", "Interop:", "DNG:"]
        .iter()
        .any(|prefix| key.starts_with(prefix))
        .then_some(key)
}

fn pdf_info_name(key: &str) -> Option<&str> {
    let name = key.strip_prefix("PDF:")?;
    matches!(
        name,
        "Title"
            | "Author"
            | "Subject"
            | "Keywords"
            | "Creator"
            | "Producer"
            | "CreationDate"
            | "ModifyDate"
    )
    .then_some(name)
}

fn icc_text_name(key: &str) -> Option<&str> {
    let name = key.strip_prefix("ICC:")?;
    matches!(
        name,
        "Description" | "Copyright" | "ManufacturerDescription" | "ModelDescription"
    )
    .then_some(name)
}

fn psd_xmp_key(key: &str) -> bool {
    matches!(key, "PSD:XMP" | "PSD:ImageResources:XMP")
}

fn avi_info_name(key: &str) -> Option<&str> {
    let name = key.strip_prefix("AVI:")?;
    matches!(
        name,
        "Title"
            | "Artist"
            | "Comment"
            | "Copyright"
            | "Software"
            | "Genre"
            | "Product"
            | "Keywords"
            | "DateTime"
    )
    .then_some(name)
}

fn matroska_tag_name(key: &str) -> Option<&str> {
    let name = key.strip_prefix("Matroska:Tag:")?;
    (!name.is_empty()).then_some(name)
}

fn matroska_info_key(key: &str) -> bool {
    matches!(
        key,
        "Matroska:Title" | "Matroska:MuxingApp" | "Matroska:WritingApp"
    )
}

fn jpeg_exif_ascii_key(key: &str) -> Option<&str> {
    let key = key.strip_prefix("JPEG:").unwrap_or(key);
    key.starts_with("EXIF:").then_some(key)
}

fn isobmff_text_key(key: &str) -> bool {
    matches!(
        key,
        "ISOBMFF:Title"
            | "ISOBMFF:Artist"
            | "ISOBMFF:Album"
            | "ISOBMFF:Year"
            | "ISOBMFF:Comment"
            | "ISOBMFF:AlbumArtist"
            | "ISOBMFF:Description"
            | "ISOBMFF:PurchaseDate"
            | "ISOBMFF:Encoder"
    )
}

fn is_isobmff_format(format: metra::FileFormat) -> bool {
    matches!(
        format,
        metra::FileFormat::Heif
            | metra::FileFormat::Avif
            | metra::FileFormat::Mp4
            | metra::FileFormat::Mov
            | metra::FileFormat::M4a
    )
}

fn is_cr3_metadata(metadata: &metra::Metadata) -> bool {
    metadata.find("RAW:Variant").is_some_and(
        |tag| matches!(&tag.value, metra::TagValue::String(variant) if variant == "CR3"),
    )
}

fn png_xmp_key(key: &str) -> bool {
    matches!(key, "PNG:XMP" | "PNG:iTXt:XMP")
}

fn jpeg_xmp_key(key: &str) -> bool {
    matches!(key, "JPEG:XMP" | "JPEG:APP1:XMP")
}

fn iptc_name(key: &str) -> Option<&str> {
    let name = key.strip_prefix("IPTC:")?;
    matches!(
        name,
        "ObjectName"
            | "EditStatus"
            | "Urgency"
            | "Category"
            | "SupplementalCategories"
            | "Keywords"
            | "DateCreated"
            | "TimeCreated"
            | "Byline"
            | "BylineTitle"
            | "City"
            | "SubLocation"
            | "ProvinceState"
            | "CountryCode"
            | "Country"
            | "Headline"
            | "Credit"
            | "Source"
            | "CopyrightNotice"
            | "CaptionAbstract"
            | "WriterEditor"
    )
    .then_some(name)
}

fn svg_set_edit(key: &str, value: &str) -> Option<metra::SvgEdit> {
    match key {
        "SVG:Title" => Some(metra::SvgEdit::SetTitle(value.to_owned())),
        "SVG:Description" => Some(metra::SvgEdit::SetDescription(value.to_owned())),
        "SVG:Comment" => Some(metra::SvgEdit::SetComment(value.to_owned())),
        _ => None,
    }
}

fn svg_delete_edit(key: &str) -> Option<metra::SvgEdit> {
    match key {
        "SVG:Title" => Some(metra::SvgEdit::DeleteTitles),
        "SVG:Description" => Some(metra::SvgEdit::DeleteDescriptions),
        "SVG:Comment" => Some(metra::SvgEdit::DeleteComments),
        _ => None,
    }
}

fn svg_text_key(key: &str) -> Option<SvgTextKey> {
    match key {
        "SVG:Title" => Some(SvgTextKey::Title),
        "SVG:Description" => Some(SvgTextKey::Description),
        "SVG:Comment" => Some(SvgTextKey::Comment),
        _ => None,
    }
}

fn unsupported_edit_message(key: &str) -> String {
    format!(
        "unsupported metadata key {key}; writable keys are JPEG:Comment, JPEG:EXIF:<ASCII tag>, JPEG:XMP, IPTC:<dataset>, TIFF:EXIF:<ASCII tag>, PDF:<Info field>, PSD:XMP, AVI:<INFO field>, Matroska:Title/MuxingApp/WritingApp, Matroska:Tag:<name>, ISOBMFF:<text field>, PNG:XMP, PNG:Text:<keyword>, WAV:<INFO field>, FLAC:<Vorbis field>, Ogg:<Vorbis field>, ID3:<text field>, GIF:Comment, WebP:XMP, or SVG:Title/Description/Comment"
    )
}

fn mp3_text_name(key: &str) -> Option<&str> {
    let name = key.strip_prefix("ID3:")?;
    matches!(
        name,
        "Title"
            | "Artist"
            | "AlbumArtist"
            | "Album"
            | "RecordingDate"
            | "Genre"
            | "TrackNumber"
            | "DiscNumber"
            | "Composer"
            | "BPM"
            | "DurationMilliseconds"
            | "Copyright"
            | "Publisher"
            | "EncodedBy"
            | "EncoderSettings"
            | "AlbumSortOrder"
            | "ArtistSortOrder"
            | "TitleSortOrder"
            | "OriginalReleaseDate"
            | "ReleaseDate"
            | "InitialKey"
            | "Language"
            | "ContentGroup"
            | "Subtitle"
            | "FileType"
            | "MediaType"
    )
    .then_some(name)
}

fn flac_comment_name(key: &str) -> Option<&str> {
    let name = key.strip_prefix("FLAC:")?;
    matches!(
        name,
        "Title"
            | "Artist"
            | "Album"
            | "AlbumArtist"
            | "Date"
            | "Genre"
            | "TrackNumber"
            | "DiscNumber"
            | "Comment"
            | "Composer"
            | "Copyright"
            | "Description"
            | "Encoder"
            | "License"
            | "Organization"
            | "ISRC"
    )
    .then_some(name)
}

fn ogg_comment_name(key: &str) -> Option<&str> {
    let name = key.strip_prefix("Ogg:")?;
    let name = name.strip_prefix("Comment:").unwrap_or(name);
    matches!(
        name,
        "Title"
            | "Artist"
            | "Album"
            | "AlbumArtist"
            | "Date"
            | "Genre"
            | "TrackNumber"
            | "DiscNumber"
            | "Comment"
            | "Composer"
            | "Copyright"
            | "Description"
            | "Encoder"
            | "License"
            | "Organization"
            | "ISRC"
    )
    .then_some(name)
}

fn webp_xmp_key(key: &str) -> bool {
    matches!(key, "WebP:XMP" | "WEBP:XMP")
}

fn wav_info_name(key: &str) -> Option<&str> {
    let name = key.strip_prefix("WAV:")?;
    matches!(
        name,
        "Title"
            | "Artist"
            | "Product"
            | "Comment"
            | "CreationDate"
            | "Genre"
            | "Engineer"
            | "Software"
            | "Copyright"
            | "Technician"
            | "Subject"
            | "Source"
    )
    .then_some(name)
}

fn apply_request(paths: &[PathBuf], request: EditRequest, limits: ParseLimits) -> ExitCode {
    match request {
        EditRequest::DirectJpeg(edits) => apply_jpeg_edits(paths, &edits, limits),
        EditRequest::DirectTiff(edits) => apply_tiff_edits(paths, &edits, limits),
        EditRequest::DirectIsobmff(edits) => apply_isobmff_edits(paths, &edits, limits),
        EditRequest::DirectPng(edits) => apply_png_edits(paths, &edits, limits),
        EditRequest::DirectWav(edits) => apply_wav_edits(paths, &edits, limits),
        EditRequest::DirectFlac(edits) => apply_flac_edits(paths, &edits, limits),
        EditRequest::DirectOgg(edits) => apply_ogg_edits(paths, &edits, limits),
        EditRequest::DirectPdf(edits) => apply_pdf_edits(paths, &edits, limits),
        EditRequest::DirectPsd(edits) => apply_psd_edits(paths, &edits, limits),
        EditRequest::DirectIcc(edits) => apply_icc_edits(paths, &edits, limits),
        EditRequest::DirectAvi(edits) => apply_avi_edits(paths, &edits, limits),
        EditRequest::DirectMatroska(edits) => apply_matroska_edits(paths, &edits, limits),
        EditRequest::DirectMp3(edits) => apply_mp3_edits(paths, &edits, limits),
        EditRequest::DirectGif(edits) => apply_gif_edits(paths, &edits, limits),
        EditRequest::DirectWebp(edits) => apply_webp_edits(paths, &edits, limits),
        EditRequest::DirectSvg(edits) => apply_svg_edits(paths, &edits, limits),
        EditRequest::DirectXmp(edits) => apply_xmp_edits(paths, &edits, limits),
        EditRequest::Copy { key, source } => apply_copy(paths, key, &source, limits),
    }
}

fn apply_jpeg_edits(paths: &[PathBuf], edits: &[metra::JpegEdit], limits: ParseLimits) -> ExitCode {
    let mut failures = 0_usize;
    for path in paths {
        match metra::read_with_limits(path, limits) {
            Ok(metadata) if metadata.file_info.format == metra::FileFormat::Jpeg => {
                if let Err(error) = metra::rewrite_jpeg_path(path, limits, edits) {
                    eprintln!("metra: {}: {error}", path.display());
                    failures += 1;
                } else {
                    println!("updated: {}", path.display());
                }
            }
            Ok(metadata) => {
                eprintln!(
                    "metra: {}: {} edits are supported only for JPEG files",
                    path.display(),
                    metadata.file_info.format
                );
                failures += 1;
            }
            Err(error) => {
                eprintln!("metra: {}: {error}", path.display());
                failures += 1;
            }
        }
    }
    if failures == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn apply_tiff_edits(paths: &[PathBuf], edits: &[metra::TiffEdit], limits: ParseLimits) -> ExitCode {
    let mut failures = 0_usize;
    for path in paths {
        match metra::read_with_limits(path, limits) {
            Ok(metadata)
                if matches!(
                    metadata.file_info.format,
                    metra::FileFormat::Tiff | metra::FileFormat::Raw
                ) =>
            {
                let result = if metadata.file_info.format == metra::FileFormat::Raw {
                    metra::rewrite_raw_tiff_path(path, limits, edits)
                } else {
                    metra::rewrite_tiff_path(path, limits, edits)
                };
                if let Err(error) = result {
                    eprintln!("metra: {}: {error}", path.display());
                    failures += 1;
                } else {
                    println!("updated: {}", path.display());
                }
            }
            Ok(metadata) => {
                eprintln!(
                    "metra: {}: {} edits are supported only for TIFF or TIFF-like RAW files",
                    path.display(),
                    metadata.file_info.format
                );
                failures += 1;
            }
            Err(error) => {
                eprintln!("metra: {}: {error}", path.display());
                failures += 1;
            }
        }
    }
    if failures == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn apply_isobmff_edits(
    paths: &[PathBuf],
    edits: &[metra::IsobmffEdit],
    limits: ParseLimits,
) -> ExitCode {
    let mut failures = 0_usize;
    for path in paths {
        match metra::read_with_limits(path, limits) {
            Ok(metadata)
                if is_isobmff_format(metadata.file_info.format)
                    || (metadata.file_info.format == metra::FileFormat::Raw
                        && is_cr3_metadata(&metadata)) =>
            {
                let result = if metadata.file_info.format == metra::FileFormat::Raw {
                    metra::rewrite_raw_cr3_path(path, limits, edits)
                } else {
                    metra::rewrite_isobmff_path(path, limits, edits)
                };
                if let Err(error) = result {
                    eprintln!("metra: {}: {error}", path.display());
                    failures += 1;
                } else {
                    println!("updated: {}", path.display());
                }
            }
            Ok(metadata) => {
                eprintln!(
                    "metra: {}: {} edits are supported only for ISO-BMFF files",
                    path.display(),
                    metadata.file_info.format
                );
                failures += 1;
            }
            Err(error) => {
                eprintln!("metra: {}: {error}", path.display());
                failures += 1;
            }
        }
    }
    if failures == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn apply_png_edits(paths: &[PathBuf], edits: &[metra::PngEdit], limits: ParseLimits) -> ExitCode {
    let mut failures = 0_usize;
    for path in paths {
        match metra::read_with_limits(path, limits) {
            Ok(metadata) if metadata.file_info.format == metra::FileFormat::Png => {
                if let Err(error) = metra::rewrite_png_path(path, limits, edits) {
                    eprintln!("metra: {}: {error}", path.display());
                    failures += 1;
                } else {
                    println!("updated: {}", path.display());
                }
            }
            Ok(metadata) => {
                eprintln!(
                    "metra: {}: {} edits are supported only for PNG files",
                    path.display(),
                    metadata.file_info.format
                );
                failures += 1;
            }
            Err(error) => {
                eprintln!("metra: {}: {error}", path.display());
                failures += 1;
            }
        }
    }
    if failures == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn apply_svg_edits(paths: &[PathBuf], edits: &[metra::SvgEdit], limits: ParseLimits) -> ExitCode {
    let mut failures = 0_usize;
    for path in paths {
        match metra::read_with_limits(path, limits) {
            Ok(metadata) if metadata.file_info.format == metra::FileFormat::Svg => {
                if let Err(error) = metra::rewrite_svg_path(path, limits, edits) {
                    eprintln!("metra: {}: {error}", path.display());
                    failures += 1;
                } else {
                    println!("updated: {}", path.display());
                }
            }
            Ok(metadata) => {
                eprintln!(
                    "metra: {}: {} edits are supported only for SVG files",
                    path.display(),
                    metadata.file_info.format
                );
                failures += 1;
            }
            Err(error) => {
                eprintln!("metra: {}: {error}", path.display());
                failures += 1;
            }
        }
    }
    if failures == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn apply_pdf_edits(paths: &[PathBuf], edits: &[metra::PdfEdit], limits: ParseLimits) -> ExitCode {
    let mut failures = 0_usize;
    for path in paths {
        match metra::read_with_limits(path, limits) {
            Ok(metadata) if metadata.file_info.format == metra::FileFormat::Pdf => {
                if let Err(error) = metra::rewrite_pdf_path(path, limits, edits) {
                    eprintln!("metra: {}: {error}", path.display());
                    failures += 1;
                } else {
                    println!("updated: {}", path.display());
                }
            }
            Ok(metadata) => {
                eprintln!(
                    "metra: {}: {} edits are supported only for PDF files",
                    path.display(),
                    metadata.file_info.format
                );
                failures += 1;
            }
            Err(error) => {
                eprintln!("metra: {}: {error}", path.display());
                failures += 1;
            }
        }
    }
    if failures == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn apply_xmp_edits(paths: &[PathBuf], edits: &[metra::XmpEdit], limits: ParseLimits) -> ExitCode {
    let mut failures = 0_usize;
    for path in paths {
        match metra::read_with_limits(path, limits) {
            Ok(metadata) if metadata.file_info.format == metra::FileFormat::Xmp => {
                if let Err(error) = metra::rewrite_xmp_path(path, limits, edits) {
                    eprintln!("metra: {}: {error}", path.display());
                    failures += 1;
                } else {
                    println!("updated: {}", path.display());
                }
            }
            Ok(metadata) => {
                eprintln!(
                    "metra: {}: XMP packet edits are supported only for standalone XMP files, not {}",
                    path.display(),
                    metadata.file_info.format
                );
                failures += 1;
            }
            Err(error) => {
                eprintln!("metra: {}: {error}", path.display());
                failures += 1;
            }
        }
    }
    if failures == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn apply_icc_edits(paths: &[PathBuf], edits: &[metra::IccEdit], limits: ParseLimits) -> ExitCode {
    let mut failures = 0_usize;
    for path in paths {
        match metra::read_with_limits(path, limits) {
            Ok(metadata) if metadata.file_info.format == metra::FileFormat::Icc => {
                if let Err(error) = metra::rewrite_icc_path(path, limits, edits) {
                    eprintln!("metra: {}: {error}", path.display());
                    failures += 1;
                } else {
                    println!("updated: {}", path.display());
                }
            }
            Ok(metadata) => {
                eprintln!(
                    "metra: {}: ICC edits are supported only for standalone ICC files, not {}",
                    path.display(),
                    metadata.file_info.format
                );
                failures += 1;
            }
            Err(error) => {
                eprintln!("metra: {}: {error}", path.display());
                failures += 1;
            }
        }
    }
    if failures == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn apply_psd_edits(paths: &[PathBuf], edits: &[metra::PsdEdit], limits: ParseLimits) -> ExitCode {
    let mut failures = 0_usize;
    for path in paths {
        match metra::read_with_limits(path, limits) {
            Ok(metadata) if metadata.file_info.format == metra::FileFormat::Psd => {
                if let Err(error) = metra::rewrite_psd_path(path, limits, edits) {
                    eprintln!("metra: {}: {error}", path.display());
                    failures += 1;
                } else {
                    println!("updated: {}", path.display());
                }
            }
            Ok(metadata) => {
                eprintln!(
                    "metra: {}: {} edits are supported only for PSD/PSB files",
                    path.display(),
                    metadata.file_info.format
                );
                failures += 1;
            }
            Err(error) => {
                eprintln!("metra: {}: {error}", path.display());
                failures += 1;
            }
        }
    }
    if failures == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn apply_avi_edits(paths: &[PathBuf], edits: &[metra::AviEdit], limits: ParseLimits) -> ExitCode {
    let mut failures = 0_usize;
    for path in paths {
        match metra::read_with_limits(path, limits) {
            Ok(metadata) if metadata.file_info.format == metra::FileFormat::Avi => {
                if let Err(error) = metra::rewrite_avi_path(path, limits, edits) {
                    eprintln!("metra: {}: {error}", path.display());
                    failures += 1;
                } else {
                    println!("updated: {}", path.display());
                }
            }
            Ok(metadata) => {
                eprintln!(
                    "metra: {}: {} edits are supported only for AVI files",
                    path.display(),
                    metadata.file_info.format
                );
                failures += 1;
            }
            Err(error) => {
                eprintln!("metra: {}: {error}", path.display());
                failures += 1;
            }
        }
    }
    if failures == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn apply_matroska_edits(
    paths: &[PathBuf],
    edits: &[metra::MatroskaEdit],
    limits: ParseLimits,
) -> ExitCode {
    let mut failures = 0_usize;
    for path in paths {
        match metra::read_with_limits(path, limits) {
            Ok(metadata)
                if matches!(
                    metadata.file_info.format,
                    metra::FileFormat::Mkv | metra::FileFormat::Webm
                ) =>
            {
                if let Err(error) = metra::rewrite_matroska_path(path, limits, edits) {
                    eprintln!("metra: {}: {error}", path.display());
                    failures += 1;
                } else {
                    println!("updated: {}", path.display());
                }
            }
            Ok(metadata) => {
                eprintln!(
                    "metra: {}: {} edits are supported only for MKV/WebM files",
                    path.display(),
                    metadata.file_info.format
                );
                failures += 1;
            }
            Err(error) => {
                eprintln!("metra: {}: {error}", path.display());
                failures += 1;
            }
        }
    }
    if failures == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn apply_copy(paths: &[PathBuf], key: CopyKey, source: &Path, limits: ParseLimits) -> ExitCode {
    let source_metadata = match metra::read_with_limits(source, limits) {
        Ok(metadata) => metadata,
        Err(error) => {
            eprintln!("metra: {}: {error}", source.display());
            return ExitCode::from(1);
        }
    };
    match key {
        CopyKey::JpegComment => {
            if source_metadata.file_info.format != metra::FileFormat::Jpeg {
                eprintln!(
                    "metra: {}: source format {} is not JPEG",
                    source.display(),
                    source_metadata.file_info.format
                );
                return ExitCode::from(1);
            }
            let Some(comment) = source_metadata.find("JPEG:Comment") else {
                eprintln!(
                    "metra: {}: source does not contain JPEG:Comment",
                    source.display()
                );
                return ExitCode::from(1);
            };
            let metra::TagValue::String(comment) = &comment.value else {
                eprintln!("metra: {}: JPEG:Comment is not a string", source.display());
                return ExitCode::from(1);
            };
            let edits = [metra::JpegEdit::SetComment(comment.clone())];
            apply_jpeg_edits(paths, &edits, limits)
        }
        CopyKey::JpegExifAscii(key) => {
            if source_metadata.file_info.format != metra::FileFormat::Jpeg {
                eprintln!(
                    "metra: {}: source format {} is not JPEG",
                    source.display(),
                    source_metadata.file_info.format
                );
                return ExitCode::from(1);
            }
            let Some(tag) = source_metadata.find(&key) else {
                eprintln!("metra: {}: source does not contain {key}", source.display());
                return ExitCode::from(1);
            };
            let metra::TagValue::String(value) = &tag.value else {
                eprintln!("metra: {}: {key} is not an ASCII string", source.display());
                return ExitCode::from(1);
            };
            let edits = [metra::JpegEdit::SetExifAscii {
                key,
                value: value.clone(),
            }];
            apply_jpeg_edits(paths, &edits, limits)
        }
        CopyKey::JpegXmp => {
            if source_metadata.file_info.format != metra::FileFormat::Jpeg {
                eprintln!(
                    "metra: {}: source format {} is not JPEG",
                    source.display(),
                    source_metadata.file_info.format
                );
                return ExitCode::from(1);
            }
            let Some(packet) = source_metadata.find("XMP:Packet") else {
                eprintln!(
                    "metra: {}: source does not contain an XMP packet",
                    source.display()
                );
                return ExitCode::from(1);
            };
            let metra::TagValue::Bytes(packet) = &packet.value else {
                eprintln!("metra: {}: XMP:Packet is not raw bytes", source.display());
                return ExitCode::from(1);
            };
            let Ok(packet) = String::from_utf8(packet.clone()) else {
                eprintln!("metra: {}: XMP:Packet is not valid UTF-8", source.display());
                return ExitCode::from(1);
            };
            let edits = [metra::JpegEdit::SetXmp(packet)];
            apply_jpeg_edits(paths, &edits, limits)
        }
        CopyKey::JpegIptc(name) => {
            if source_metadata.file_info.format != metra::FileFormat::Jpeg {
                eprintln!(
                    "metra: {}: source format {} is not JPEG",
                    source.display(),
                    source_metadata.file_info.format
                );
                return ExitCode::from(1);
            }
            let key = format!("IPTC:{name}");
            let Some(value) = source_metadata.find(&key) else {
                eprintln!("metra: {}: source does not contain {key}", source.display());
                return ExitCode::from(1);
            };
            let metra::TagValue::String(value) = &value.value else {
                eprintln!(
                    "metra: {}: {key} contains multiple values; copy a single IPTC dataset",
                    source.display()
                );
                return ExitCode::from(1);
            };
            let edits = [metra::JpegEdit::SetIptc {
                name,
                value: value.clone(),
            }];
            apply_jpeg_edits(paths, &edits, limits)
        }
        CopyKey::TiffAscii(key) => {
            if !matches!(
                source_metadata.file_info.format,
                metra::FileFormat::Tiff | metra::FileFormat::Raw
            ) {
                eprintln!(
                    "metra: {}: source format {} is not TIFF or RAW",
                    source.display(),
                    source_metadata.file_info.format
                );
                return ExitCode::from(1);
            }
            let Some(tag) = source_metadata.find(&key) else {
                eprintln!("metra: {}: source does not contain {key}", source.display());
                return ExitCode::from(1);
            };
            let metra::TagValue::String(value) = &tag.value else {
                eprintln!("metra: {}: {key} is not a string", source.display());
                return ExitCode::from(1);
            };
            let edits = [metra::TiffEdit::SetAscii {
                key,
                value: value.clone(),
            }];
            apply_tiff_edits(paths, &edits, limits)
        }
        CopyKey::PdfInfo(name) => {
            if source_metadata.file_info.format != metra::FileFormat::Pdf {
                eprintln!(
                    "metra: {}: source format {} is not PDF",
                    source.display(),
                    source_metadata.file_info.format
                );
                return ExitCode::from(1);
            }
            let key = format!("PDF:{name}");
            let Some(tag) = source_metadata.find(&key) else {
                eprintln!("metra: {}: source does not contain {key}", source.display());
                return ExitCode::from(1);
            };
            let metra::TagValue::String(value) = &tag.value else {
                eprintln!("metra: {}: {key} is not a string", source.display());
                return ExitCode::from(1);
            };
            let edits = [metra::PdfEdit::SetInfo {
                name,
                value: value.clone(),
            }];
            apply_pdf_edits(paths, &edits, limits)
        }
        CopyKey::IccText(name) => {
            if source_metadata.file_info.format != metra::FileFormat::Icc {
                eprintln!(
                    "metra: {}: source format {} is not standalone ICC",
                    source.display(),
                    source_metadata.file_info.format
                );
                return ExitCode::from(1);
            }
            let key = format!("ICC:{name}");
            let Some(tag) = source_metadata.find(&key) else {
                eprintln!("metra: {}: source does not contain {key}", source.display());
                return ExitCode::from(1);
            };
            let metra::TagValue::String(value) = &tag.value else {
                eprintln!("metra: {}: {key} is not a string", source.display());
                return ExitCode::from(1);
            };
            let edits = [metra::IccEdit::SetText {
                name,
                value: value.clone(),
            }];
            apply_icc_edits(paths, &edits, limits)
        }
        CopyKey::XmpPacket => {
            if source_metadata.file_info.format != metra::FileFormat::Xmp {
                eprintln!(
                    "metra: {}: source format {} is not standalone XMP",
                    source.display(),
                    source_metadata.file_info.format
                );
                return ExitCode::from(1);
            }
            let Some(packet) = source_metadata.find("XMP:Packet") else {
                eprintln!(
                    "metra: {}: source does not contain an XMP packet",
                    source.display()
                );
                return ExitCode::from(1);
            };
            let metra::TagValue::Bytes(packet) = &packet.value else {
                eprintln!("metra: {}: XMP:Packet is not raw bytes", source.display());
                return ExitCode::from(1);
            };
            let Ok(packet) = String::from_utf8(packet.clone()) else {
                eprintln!("metra: {}: XMP:Packet is not valid UTF-8", source.display());
                return ExitCode::from(1);
            };
            let edits = [metra::XmpEdit::SetPacket(packet)];
            apply_xmp_edits(paths, &edits, limits)
        }
        CopyKey::PsdXmp => {
            if source_metadata.file_info.format != metra::FileFormat::Psd {
                eprintln!(
                    "metra: {}: source format {} is not PSD/PSB",
                    source.display(),
                    source_metadata.file_info.format
                );
                return ExitCode::from(1);
            }
            let Some(packet) = source_metadata.find("XMP:Packet") else {
                eprintln!(
                    "metra: {}: source does not contain an XMP packet",
                    source.display()
                );
                return ExitCode::from(1);
            };
            let metra::TagValue::Bytes(packet) = &packet.value else {
                eprintln!("metra: {}: XMP:Packet is not raw bytes", source.display());
                return ExitCode::from(1);
            };
            let Ok(packet) = String::from_utf8(packet.clone()) else {
                eprintln!("metra: {}: XMP:Packet is not valid UTF-8", source.display());
                return ExitCode::from(1);
            };
            let edits = [metra::PsdEdit::SetXmp(packet)];
            apply_psd_edits(paths, &edits, limits)
        }
        CopyKey::AviInfo(name) => {
            if source_metadata.file_info.format != metra::FileFormat::Avi {
                eprintln!(
                    "metra: {}: source format {} is not AVI",
                    source.display(),
                    source_metadata.file_info.format
                );
                return ExitCode::from(1);
            }
            let key = format!("AVI:{name}");
            let Some(tag) = source_metadata.find(&key) else {
                eprintln!("metra: {}: source does not contain {key}", source.display());
                return ExitCode::from(1);
            };
            let metra::TagValue::String(value) = &tag.value else {
                eprintln!("metra: {}: {key} is not a string", source.display());
                return ExitCode::from(1);
            };
            let edits = [metra::AviEdit::SetInfo {
                name,
                value: value.clone(),
            }];
            apply_avi_edits(paths, &edits, limits)
        }
        CopyKey::MatroskaTag(name) => {
            if !matches!(
                source_metadata.file_info.format,
                metra::FileFormat::Mkv | metra::FileFormat::Webm
            ) {
                eprintln!(
                    "metra: {}: source format {} is not MKV/WebM",
                    source.display(),
                    source_metadata.file_info.format
                );
                return ExitCode::from(1);
            }
            let key = format!("Matroska:Tag:{name}");
            let Some(tag) = source_metadata.find(&key) else {
                eprintln!("metra: {}: source does not contain {key}", source.display());
                return ExitCode::from(1);
            };
            let metra::TagValue::String(value) = &tag.value else {
                eprintln!("metra: {}: {key} is not a string", source.display());
                return ExitCode::from(1);
            };
            let edits = [metra::MatroskaEdit::SetTag {
                name,
                value: value.clone(),
            }];
            apply_matroska_edits(paths, &edits, limits)
        }
        CopyKey::MatroskaString(key) => {
            if !matches!(
                source_metadata.file_info.format,
                metra::FileFormat::Mkv | metra::FileFormat::Webm
            ) {
                eprintln!(
                    "metra: {}: source format {} is not MKV/WebM",
                    source.display(),
                    source_metadata.file_info.format
                );
                return ExitCode::from(1);
            }
            let Some(tag) = source_metadata.find(&key) else {
                eprintln!("metra: {}: source does not contain {key}", source.display());
                return ExitCode::from(1);
            };
            let metra::TagValue::String(value) = &tag.value else {
                eprintln!("metra: {}: {key} is not a string", source.display());
                return ExitCode::from(1);
            };
            let edits = [metra::MatroskaEdit::SetString {
                key,
                value: value.clone(),
            }];
            apply_matroska_edits(paths, &edits, limits)
        }
        CopyKey::IsobmffText(key) => {
            if !(is_isobmff_format(source_metadata.file_info.format)
                || (source_metadata.file_info.format == metra::FileFormat::Raw
                    && is_cr3_metadata(&source_metadata)))
            {
                eprintln!(
                    "metra: {}: source format {} is not ISO-BMFF or CR3",
                    source.display(),
                    source_metadata.file_info.format
                );
                return ExitCode::from(1);
            }
            let Some(tag) = source_metadata.find(&key) else {
                eprintln!("metra: {}: source does not contain {key}", source.display());
                return ExitCode::from(1);
            };
            let metra::TagValue::String(value) = &tag.value else {
                eprintln!("metra: {}: {key} is not a string", source.display());
                return ExitCode::from(1);
            };
            let edits = [metra::IsobmffEdit::SetText {
                key,
                value: value.clone(),
            }];
            apply_isobmff_edits(paths, &edits, limits)
        }
        CopyKey::PngText(keyword) => {
            if source_metadata.file_info.format != metra::FileFormat::Png {
                eprintln!(
                    "metra: {}: source format {} is not PNG",
                    source.display(),
                    source_metadata.file_info.format
                );
                return ExitCode::from(1);
            }
            let key = format!("PNG:Text:{keyword}");
            let Some(text) = source_metadata.find(&key) else {
                eprintln!("metra: {}: source does not contain {key}", source.display());
                return ExitCode::from(1);
            };
            let metra::TagValue::String(text) = &text.value else {
                eprintln!("metra: {}: {key} is not a string", source.display());
                return ExitCode::from(1);
            };
            let edits = [metra::PngEdit::SetText {
                keyword,
                value: text.clone(),
            }];
            apply_png_edits(paths, &edits, limits)
        }
        CopyKey::PngXmp => {
            if source_metadata.file_info.format != metra::FileFormat::Png {
                eprintln!(
                    "metra: {}: source format {} is not PNG",
                    source.display(),
                    source_metadata.file_info.format
                );
                return ExitCode::from(1);
            }
            let Some(packet) = source_metadata.find("XMP:Packet") else {
                eprintln!(
                    "metra: {}: source does not contain an XMP packet",
                    source.display()
                );
                return ExitCode::from(1);
            };
            let metra::TagValue::Bytes(packet) = &packet.value else {
                eprintln!("metra: {}: XMP:Packet is not raw bytes", source.display());
                return ExitCode::from(1);
            };
            let Ok(packet) = String::from_utf8(packet.clone()) else {
                eprintln!("metra: {}: XMP:Packet is not valid UTF-8", source.display());
                return ExitCode::from(1);
            };
            let edits = [metra::PngEdit::SetXmp(packet)];
            apply_png_edits(paths, &edits, limits)
        }
        CopyKey::SvgText(kind) => {
            if source_metadata.file_info.format != metra::FileFormat::Svg {
                eprintln!(
                    "metra: {}: source format {} is not SVG",
                    source.display(),
                    source_metadata.file_info.format
                );
                return ExitCode::from(1);
            }
            let key = match kind {
                SvgTextKey::Title => "SVG:Title",
                SvgTextKey::Description => "SVG:Description",
                SvgTextKey::Comment => "SVG:Comment",
            };
            let Some(text) = source_metadata.find(key) else {
                eprintln!("metra: {}: source does not contain {key}", source.display());
                return ExitCode::from(1);
            };
            let metra::TagValue::String(text) = &text.value else {
                eprintln!("metra: {}: {key} is not a string", source.display());
                return ExitCode::from(1);
            };
            let edit = match kind {
                SvgTextKey::Title => metra::SvgEdit::SetTitle(text.clone()),
                SvgTextKey::Description => metra::SvgEdit::SetDescription(text.clone()),
                SvgTextKey::Comment => metra::SvgEdit::SetComment(text.clone()),
            };
            apply_svg_edits(paths, &[edit], limits)
        }
        CopyKey::WavInfo(name) => {
            if source_metadata.file_info.format != metra::FileFormat::Wav {
                eprintln!(
                    "metra: {}: source format {} is not WAV",
                    source.display(),
                    source_metadata.file_info.format
                );
                return ExitCode::from(1);
            }
            let key = format!("WAV:{name}");
            let Some(text) = source_metadata.find(&key) else {
                eprintln!("metra: {}: source does not contain {key}", source.display());
                return ExitCode::from(1);
            };
            let metra::TagValue::String(text) = &text.value else {
                eprintln!("metra: {}: {key} is not a string", source.display());
                return ExitCode::from(1);
            };
            let edits = [metra::WavEdit::SetInfo {
                name,
                value: text.clone(),
            }];
            apply_wav_edits(paths, &edits, limits)
        }
        CopyKey::FlacComment(name) => {
            if source_metadata.file_info.format != metra::FileFormat::Flac {
                eprintln!(
                    "metra: {}: source format {} is not FLAC",
                    source.display(),
                    source_metadata.file_info.format
                );
                return ExitCode::from(1);
            }
            let key = format!("FLAC:{name}");
            let Some(text) = source_metadata.find(&key) else {
                eprintln!("metra: {}: source does not contain {key}", source.display());
                return ExitCode::from(1);
            };
            let metra::TagValue::String(text) = &text.value else {
                eprintln!("metra: {}: {key} is not a string", source.display());
                return ExitCode::from(1);
            };
            let edits = [metra::FlacEdit::SetComment {
                key: name,
                value: text.clone(),
            }];
            apply_flac_edits(paths, &edits, limits)
        }
        CopyKey::OggComment(name) => {
            if source_metadata.file_info.format != metra::FileFormat::Ogg {
                eprintln!(
                    "metra: {}: source format {} is not Ogg",
                    source.display(),
                    source_metadata.file_info.format
                );
                return ExitCode::from(1);
            }
            let key = format!("Ogg:{name}");
            let Some(text) = source_metadata.find(&key) else {
                eprintln!("metra: {}: source does not contain {key}", source.display());
                return ExitCode::from(1);
            };
            let metra::TagValue::String(text) = &text.value else {
                eprintln!("metra: {}: {key} is not a string", source.display());
                return ExitCode::from(1);
            };
            let edits = [metra::OggEdit::SetComment {
                key: name,
                value: text.clone(),
            }];
            apply_ogg_edits(paths, &edits, limits)
        }
        CopyKey::Mp3Comment => {
            if source_metadata.file_info.format != metra::FileFormat::Mp3 {
                eprintln!(
                    "metra: {}: source format {} is not MP3",
                    source.display(),
                    source_metadata.file_info.format
                );
                return ExitCode::from(1);
            }
            let Some(comment) = find_mp3_tag(&source_metadata, "ID3:Comment") else {
                eprintln!(
                    "metra: {}: source does not contain ID3:Comment",
                    source.display()
                );
                return ExitCode::from(1);
            };
            let metra::TagValue::String(comment) = &comment.value else {
                eprintln!("metra: {}: ID3:Comment is not a string", source.display());
                return ExitCode::from(1);
            };
            let edits = [metra::Mp3Edit::SetComment(comment.clone())];
            apply_mp3_edits(paths, &edits, limits)
        }
        CopyKey::Mp3Text(name) => {
            if source_metadata.file_info.format != metra::FileFormat::Mp3 {
                eprintln!(
                    "metra: {}: source format {} is not MP3",
                    source.display(),
                    source_metadata.file_info.format
                );
                return ExitCode::from(1);
            }
            let key = format!("ID3:{name}");
            let Some(text) = find_mp3_tag(&source_metadata, &key) else {
                eprintln!("metra: {}: source does not contain {key}", source.display());
                return ExitCode::from(1);
            };
            let metra::TagValue::String(text) = &text.value else {
                eprintln!("metra: {}: {key} is not a string", source.display());
                return ExitCode::from(1);
            };
            let edits = [metra::Mp3Edit::SetText {
                name,
                value: text.clone(),
            }];
            apply_mp3_edits(paths, &edits, limits)
        }
        CopyKey::GifComment => {
            if source_metadata.file_info.format != metra::FileFormat::Gif {
                eprintln!(
                    "metra: {}: source format {} is not GIF",
                    source.display(),
                    source_metadata.file_info.format
                );
                return ExitCode::from(1);
            }
            let Some(comment) = source_metadata.find("GIF:Comment") else {
                eprintln!(
                    "metra: {}: source does not contain GIF:Comment",
                    source.display()
                );
                return ExitCode::from(1);
            };
            let metra::TagValue::String(comment) = &comment.value else {
                eprintln!("metra: {}: GIF:Comment is not a string", source.display());
                return ExitCode::from(1);
            };
            let edits = [metra::GifEdit::SetComment(comment.clone())];
            apply_gif_edits(paths, &edits, limits)
        }
        CopyKey::WebpXmp => {
            if source_metadata.file_info.format != metra::FileFormat::Webp {
                eprintln!(
                    "metra: {}: source format {} is not WebP",
                    source.display(),
                    source_metadata.file_info.format
                );
                return ExitCode::from(1);
            }
            let Some(packet) = source_metadata.find("XMP:Packet") else {
                eprintln!(
                    "metra: {}: source does not contain an XMP packet",
                    source.display()
                );
                return ExitCode::from(1);
            };
            let metra::TagValue::Bytes(packet) = &packet.value else {
                eprintln!("metra: {}: XMP:Packet is not raw bytes", source.display());
                return ExitCode::from(1);
            };
            let Ok(packet) = String::from_utf8(packet.clone()) else {
                eprintln!("metra: {}: XMP:Packet is not valid UTF-8", source.display());
                return ExitCode::from(1);
            };
            let edits = [metra::WebpEdit::SetXmp(packet)];
            apply_webp_edits(paths, &edits, limits)
        }
    }
}

fn apply_wav_edits(paths: &[PathBuf], edits: &[metra::WavEdit], limits: ParseLimits) -> ExitCode {
    let mut failures = 0_usize;
    for path in paths {
        match metra::read_with_limits(path, limits) {
            Ok(metadata) if metadata.file_info.format == metra::FileFormat::Wav => {
                if let Err(error) = metra::rewrite_wav_path(path, limits, edits) {
                    eprintln!("metra: {}: {error}", path.display());
                    failures += 1;
                } else {
                    println!("updated: {}", path.display());
                }
            }
            Ok(metadata) => {
                eprintln!(
                    "metra: {}: {} edits are supported only for WAV files",
                    path.display(),
                    metadata.file_info.format
                );
                failures += 1;
            }
            Err(error) => {
                eprintln!("metra: {}: {error}", path.display());
                failures += 1;
            }
        }
    }
    if failures == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn apply_flac_edits(paths: &[PathBuf], edits: &[metra::FlacEdit], limits: ParseLimits) -> ExitCode {
    let mut failures = 0_usize;
    for path in paths {
        match metra::read_with_limits(path, limits) {
            Ok(metadata) if metadata.file_info.format == metra::FileFormat::Flac => {
                if let Err(error) = metra::rewrite_flac_path(path, limits, edits) {
                    eprintln!("metra: {}: {error}", path.display());
                    failures += 1;
                } else {
                    println!("updated: {}", path.display());
                }
            }
            Ok(metadata) => {
                eprintln!(
                    "metra: {}: {} edits are supported only for FLAC files",
                    path.display(),
                    metadata.file_info.format
                );
                failures += 1;
            }
            Err(error) => {
                eprintln!("metra: {}: {error}", path.display());
                failures += 1;
            }
        }
    }
    if failures == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn apply_ogg_edits(paths: &[PathBuf], edits: &[metra::OggEdit], limits: ParseLimits) -> ExitCode {
    let mut failures = 0_usize;
    for path in paths {
        match metra::read_with_limits(path, limits) {
            Ok(metadata) if metadata.file_info.format == metra::FileFormat::Ogg => {
                if let Err(error) = metra::rewrite_ogg_path(path, limits, edits) {
                    eprintln!("metra: {}: {error}", path.display());
                    failures += 1;
                } else {
                    println!("updated: {}", path.display());
                }
            }
            Ok(metadata) => {
                eprintln!(
                    "metra: {}: {} edits are supported only for Ogg files",
                    path.display(),
                    metadata.file_info.format
                );
                failures += 1;
            }
            Err(error) => {
                eprintln!("metra: {}: {error}", path.display());
                failures += 1;
            }
        }
    }
    if failures == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn apply_mp3_edits(paths: &[PathBuf], edits: &[metra::Mp3Edit], limits: ParseLimits) -> ExitCode {
    let mut failures = 0_usize;
    for path in paths {
        match metra::read_with_limits(path, limits) {
            Ok(metadata) if metadata.file_info.format == metra::FileFormat::Mp3 => {
                if let Err(error) = metra::rewrite_mp3_path(path, limits, edits) {
                    eprintln!("metra: {}: {error}", path.display());
                    failures += 1;
                } else {
                    println!("updated: {}", path.display());
                }
            }
            Ok(metadata) => {
                eprintln!(
                    "metra: {}: {} edits are supported only for MP3 files",
                    path.display(),
                    metadata.file_info.format
                );
                failures += 1;
            }
            Err(error) => {
                eprintln!("metra: {}: {error}", path.display());
                failures += 1;
            }
        }
    }
    if failures == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn find_mp3_tag<'a>(metadata: &'a Metadata, key: &str) -> Option<&'a metra::Tag> {
    metadata
        .tags()
        .iter()
        .find(|tag| tag.key() == key && tag.group.starts_with("ID3v"))
        .or_else(|| metadata.tags().iter().find(|tag| tag.key() == key))
}

fn apply_gif_edits(paths: &[PathBuf], edits: &[metra::GifEdit], limits: ParseLimits) -> ExitCode {
    let mut failures = 0_usize;
    for path in paths {
        match metra::read_with_limits(path, limits) {
            Ok(metadata) if metadata.file_info.format == metra::FileFormat::Gif => {
                if let Err(error) = metra::rewrite_gif_path(path, limits, edits) {
                    eprintln!("metra: {}: {error}", path.display());
                    failures += 1;
                } else {
                    println!("updated: {}", path.display());
                }
            }
            Ok(metadata) => {
                eprintln!(
                    "metra: {}: {} edits are supported only for GIF files",
                    path.display(),
                    metadata.file_info.format
                );
                failures += 1;
            }
            Err(error) => {
                eprintln!("metra: {}: {error}", path.display());
                failures += 1;
            }
        }
    }
    if failures == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn apply_webp_edits(paths: &[PathBuf], edits: &[metra::WebpEdit], limits: ParseLimits) -> ExitCode {
    let mut failures = 0_usize;
    for path in paths {
        match metra::read_with_limits(path, limits) {
            Ok(metadata) if metadata.file_info.format == metra::FileFormat::Webp => {
                if let Err(error) = metra::rewrite_webp_path(path, limits, edits) {
                    eprintln!("metra: {}: {error}", path.display());
                    failures += 1;
                } else {
                    println!("updated: {}", path.display());
                }
            }
            Ok(metadata) => {
                eprintln!(
                    "metra: {}: {} edits are supported only for WebP files",
                    path.display(),
                    metadata.file_info.format
                );
                failures += 1;
            }
            Err(error) => {
                eprintln!("metra: {}: {error}", path.display());
                failures += 1;
            }
        }
    }
    if failures == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn parse_jobs(value: &str) -> std::result::Result<usize, String> {
    let jobs = value
        .parse::<usize>()
        .map_err(|error| format!("invalid jobs value: {error}"))?;
    if jobs == 0 {
        Err("jobs must be at least 1".to_owned())
    } else {
        Ok(jobs)
    }
}

fn parse_positive_usize(value: &str) -> std::result::Result<usize, String> {
    let value = value
        .parse::<usize>()
        .map_err(|error| format!("invalid positive integer: {error}"))?;
    if value == 0 {
        Err("value must be at least 1".to_owned())
    } else {
        Ok(value)
    }
}

fn parse_limits(max_metadata_bytes: Option<usize>, max_value_bytes: Option<usize>) -> ParseLimits {
    let mut limits = ParseLimits::default();
    if let Some(value) = max_metadata_bytes {
        limits.max_metadata_bytes = value;
    }
    if let Some(value) = max_value_bytes {
        limits.max_value_bytes = value;
    }
    limits
}

fn inspect_paths(
    paths: &[PathBuf],
    jobs: usize,
    limits: ParseLimits,
    cancellation: &metra::CancellationToken,
) -> Vec<(PathBuf, metra::Result<Metadata>)> {
    metra::read_many_with_cancellation(paths, metra::BatchOptions { jobs, limits }, cancellation)
        .into_iter()
        .map(|item| (item.path, item.result))
        .collect()
}

fn inspect_paths_streaming<F>(
    paths: &[PathBuf],
    jobs: usize,
    validate: bool,
    limits: ParseLimits,
    cancellation: &metra::CancellationToken,
    mut emit: F,
) -> usize
where
    F: FnMut(PathBuf, metra::Result<Metadata>),
{
    let mut failures = 0_usize;

    metra::read_many_streaming_with_cancellation(
        paths,
        metra::BatchOptions { jobs, limits },
        cancellation,
        |item| {
            if result_is_failure(&item.result, validate) {
                failures += 1;
            }
            emit(item.path, item.result);
        },
    );
    failures
}

fn result_is_failure(result: &metra::Result<Metadata>, validate: bool) -> bool {
    result
        .as_ref()
        .map_or(true, |metadata| validate && !metadata.warnings.is_empty())
}

fn collect_paths(inputs: &[PathBuf], recursive: bool) -> Result<Vec<PathBuf>, String> {
    let mut paths = Vec::new();
    for input in inputs {
        let metadata = fs::metadata(input)
            .map_err(|error| format!("cannot inspect {}: {error}", input.display()))?;
        if metadata.is_dir() {
            if !recursive {
                return Err(format!(
                    "{} is a directory; pass --recursive to traverse it",
                    input.display()
                ));
            }
            collect_directory(input, &mut paths)?;
        } else {
            paths.push(input.clone());
        }
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

fn collect_directory(directory: &Path, output: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = fs::read_dir(directory)
        .map_err(|error| format!("cannot read {}: {error}", directory.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("cannot read directory entry: {error}"))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
        if file_type.is_dir() {
            collect_directory(&path, output)?;
        } else if file_type.is_file() {
            output.push(path);
        }
    }
    Ok(())
}

fn emit_json(results: &[(PathBuf, metra::Result<Metadata>)], jsonl: bool) {
    if jsonl {
        for (path, result) in results {
            if let Ok(metadata) = result {
                match serde_json::to_string(metadata) {
                    Ok(line) => println!("{line}"),
                    Err(error) => eprintln!("metra: cannot serialize {}: {error}", path.display()),
                }
            } else if let Err(error) = result {
                eprintln!("metra: {}: {error}", path.display());
            }
        }
        return;
    }

    let successful = results
        .iter()
        .filter_map(|(_, result)| result.as_ref().ok())
        .collect::<Vec<_>>();
    let serialized = if successful.len() == 1 {
        serde_json::to_string_pretty(successful[0])
    } else {
        serde_json::to_string_pretty(&successful)
    };
    match serialized {
        Ok(document) => println!("{document}"),
        Err(error) => eprintln!("metra: cannot serialize JSON output: {error}"),
    }
    for (path, result) in results {
        if let Err(error) = result {
            eprintln!("metra: {}: {error}", path.display());
        }
    }
}

fn emit_csv_record(path: &Path, result: &metra::Result<Metadata>) {
    let Ok(metadata) = result else {
        if let Err(error) = result {
            eprintln!("metra: {}: {error}", path.display());
        }
        return;
    };
    if metadata.tags.is_empty() {
        println!(
            "{},{},,,,,,",
            csv_field(&path.display().to_string()),
            csv_field(&metadata.file_info.format.to_string())
        );
        return;
    }
    for tag in &metadata.tags {
        let id = tag
            .id
            .map(|value| format!("0x{value:08X}"))
            .unwrap_or_default();
        let value = serde_json::to_string(&tag.value)
            .unwrap_or_else(|_| format!("\"{}\"", tag.display_value()));
        println!(
            "{},{},{},{},{},{},{},{}",
            csv_field(&path.display().to_string()),
            csv_field(&metadata.file_info.format.to_string()),
            csv_field(&tag.namespace),
            csv_field(&tag.group),
            csv_field(&id),
            csv_field(&tag.name),
            csv_field(&format!("{:?}", tag.value_type)),
            csv_field(&value)
        );
    }
}

fn emit_jsonl_record(path: &Path, result: &metra::Result<Metadata>) {
    if let Ok(metadata) = result {
        match serde_json::to_string(metadata) {
            Ok(line) => println!("{line}"),
            Err(error) => eprintln!("metra: cannot serialize {}: {error}", path.display()),
        }
    } else if let Err(error) = result {
        eprintln!("metra: {}: {error}", path.display());
    }
}

#[derive(serde::Serialize)]
struct DocumentCollection<'a> {
    files: Vec<&'a Metadata>,
}

fn emit_toml(results: &[(PathBuf, metra::Result<Metadata>)]) {
    let successful = results
        .iter()
        .filter_map(|(_, result)| result.as_ref().ok())
        .collect::<Vec<_>>();
    let serialized = if successful.len() == 1 {
        toml::to_string_pretty(successful[0])
    } else {
        toml::to_string_pretty(&DocumentCollection { files: successful })
    };
    match serialized {
        Ok(document) => print!("{document}"),
        Err(error) => eprintln!("metra: cannot serialize TOML output: {error}"),
    }
    emit_errors(results);
}

fn emit_yaml(results: &[(PathBuf, metra::Result<Metadata>)]) {
    let successful = results
        .iter()
        .filter_map(|(_, result)| result.as_ref().ok())
        .collect::<Vec<_>>();
    let serialized = if successful.len() == 1 {
        serde_yaml::to_string(successful[0])
    } else {
        serde_yaml::to_string(&DocumentCollection { files: successful })
    };
    match serialized {
        Ok(document) => print!("{document}"),
        Err(error) => eprintln!("metra: cannot serialize YAML output: {error}"),
    }
    emit_errors(results);
}

fn emit_errors(results: &[(PathBuf, metra::Result<Metadata>)]) {
    for (path, result) in results {
        if let Err(error) = result {
            eprintln!("metra: {}: {error}", path.display());
        }
    }
}

fn csv_field(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

fn print_human(metadata: &Metadata) {
    println!("File: {}", metadata.file_info.path.display());
    println!("Format: {}", metadata.file_info.format);
    println!("Size: {} bytes", metadata.file_info.size);
    if metadata.tags.is_empty() {
        println!("Tags: none");
    } else {
        println!("Tags:");
        for tag in &metadata.tags {
            let id = tag
                .id
                .map(|value| format!(" [0x{value:04X}]"))
                .unwrap_or_default();
            println!(
                "  {}:{}{} = {}",
                tag.namespace,
                tag.name,
                id,
                tag.display_value()
            );
        }
    }
    if !metadata.warnings.is_empty() {
        println!("Warnings:");
        for warning in &metadata.warnings {
            if let Some(offset) = warning.offset {
                println!("  [{} at 0x{offset:X}] {}", warning.code, warning.message);
            } else {
                println!("  [{}] {}", warning.code, warning.message);
            }
        }
    }
    println!();
}
