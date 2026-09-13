use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;

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

    /// Validate inputs and return a failure when any warning is produced.
    #[arg(long, conflicts_with_all = ["set", "delete", "copy"])]
    validate: bool,

    /// Traverse directories recursively in deterministic path order.
    #[arg(short = 'r', long)]
    recursive: bool,

    /// Maximum number of files to inspect concurrently.
    #[arg(long, default_value_t = 1, value_parser = parse_jobs)]
    jobs: usize,

    /// Files to inspect.
    #[arg(value_name = "FILE", required = true)]
    files: Vec<PathBuf>,
}

fn main() -> ExitCode {
    let arguments = Arguments::parse();
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
        return apply_request(&paths, request);
    }

    let failures = if arguments.json || arguments.toml || arguments.yaml {
        let results = inspect_paths(&paths, arguments.jobs);
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
            |path, result| {
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

    if failures == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

enum EditRequest {
    DirectJpeg(Vec<metra::JpegEdit>),
    DirectPng(Vec<metra::PngEdit>),
    DirectWav(Vec<metra::WavEdit>),
    DirectFlac(Vec<metra::FlacEdit>),
    DirectMp3(Vec<metra::Mp3Edit>),
    DirectGif(Vec<metra::GifEdit>),
    DirectWebp(Vec<metra::WebpEdit>),
    DirectSvg(Vec<metra::SvgEdit>),
    Copy { key: CopyKey, source: PathBuf },
}

#[derive(Debug)]
enum CopyKey {
    JpegComment,
    JpegXmp,
    JpegIptc(String),
    PngText(String),
    PngXmp,
    WavInfo(String),
    FlacComment(String),
    Mp3Text(String),
    Mp3Comment,
    GifComment,
    WebpXmp,
    SvgText(SvgTextKey),
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
            if let Some(name) = flac_comment_name(key) {
                return Ok(Some(EditRequest::DirectFlac(vec![
                    metra::FlacEdit::DeleteComment {
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
        "unsupported metadata key {key}; writable keys are JPEG:Comment, JPEG:XMP, IPTC:<dataset>, PNG:XMP, PNG:Text:<keyword>, WAV:<INFO field>, FLAC:<Vorbis field>, ID3:<text field>, GIF:Comment, WebP:XMP, or SVG:Title/Description/Comment"
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

fn apply_request(paths: &[PathBuf], request: EditRequest) -> ExitCode {
    match request {
        EditRequest::DirectJpeg(edits) => apply_jpeg_edits(paths, &edits),
        EditRequest::DirectPng(edits) => apply_png_edits(paths, &edits),
        EditRequest::DirectWav(edits) => apply_wav_edits(paths, &edits),
        EditRequest::DirectFlac(edits) => apply_flac_edits(paths, &edits),
        EditRequest::DirectMp3(edits) => apply_mp3_edits(paths, &edits),
        EditRequest::DirectGif(edits) => apply_gif_edits(paths, &edits),
        EditRequest::DirectWebp(edits) => apply_webp_edits(paths, &edits),
        EditRequest::DirectSvg(edits) => apply_svg_edits(paths, &edits),
        EditRequest::Copy { key, source } => apply_copy(paths, key, &source),
    }
}

fn apply_jpeg_edits(paths: &[PathBuf], edits: &[metra::JpegEdit]) -> ExitCode {
    let mut failures = 0_usize;
    for path in paths {
        match metra::read(path) {
            Ok(metadata) if metadata.file_info.format == metra::FileFormat::Jpeg => {
                if let Err(error) = metra::rewrite_jpeg_path(path, ParseLimits::default(), edits) {
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

fn apply_png_edits(paths: &[PathBuf], edits: &[metra::PngEdit]) -> ExitCode {
    let mut failures = 0_usize;
    for path in paths {
        match metra::read(path) {
            Ok(metadata) if metadata.file_info.format == metra::FileFormat::Png => {
                if let Err(error) = metra::rewrite_png_path(path, ParseLimits::default(), edits) {
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

fn apply_svg_edits(paths: &[PathBuf], edits: &[metra::SvgEdit]) -> ExitCode {
    let mut failures = 0_usize;
    for path in paths {
        match metra::read(path) {
            Ok(metadata) if metadata.file_info.format == metra::FileFormat::Svg => {
                if let Err(error) = metra::rewrite_svg_path(path, ParseLimits::default(), edits) {
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

fn apply_copy(paths: &[PathBuf], key: CopyKey, source: &Path) -> ExitCode {
    let source_metadata = match metra::read(source) {
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
            apply_jpeg_edits(paths, &edits)
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
            apply_jpeg_edits(paths, &edits)
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
            apply_jpeg_edits(paths, &edits)
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
            apply_png_edits(paths, &edits)
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
            apply_png_edits(paths, &edits)
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
            apply_svg_edits(paths, &[edit])
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
            apply_wav_edits(paths, &edits)
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
            apply_flac_edits(paths, &edits)
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
            apply_mp3_edits(paths, &edits)
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
            apply_mp3_edits(paths, &edits)
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
            apply_gif_edits(paths, &edits)
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
            apply_webp_edits(paths, &edits)
        }
    }
}

fn apply_wav_edits(paths: &[PathBuf], edits: &[metra::WavEdit]) -> ExitCode {
    let mut failures = 0_usize;
    for path in paths {
        match metra::read(path) {
            Ok(metadata) if metadata.file_info.format == metra::FileFormat::Wav => {
                if let Err(error) = metra::rewrite_wav_path(path, ParseLimits::default(), edits) {
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

fn apply_flac_edits(paths: &[PathBuf], edits: &[metra::FlacEdit]) -> ExitCode {
    let mut failures = 0_usize;
    for path in paths {
        match metra::read(path) {
            Ok(metadata) if metadata.file_info.format == metra::FileFormat::Flac => {
                if let Err(error) = metra::rewrite_flac_path(path, ParseLimits::default(), edits) {
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

fn apply_mp3_edits(paths: &[PathBuf], edits: &[metra::Mp3Edit]) -> ExitCode {
    let mut failures = 0_usize;
    for path in paths {
        match metra::read(path) {
            Ok(metadata) if metadata.file_info.format == metra::FileFormat::Mp3 => {
                if let Err(error) = metra::rewrite_mp3_path(path, ParseLimits::default(), edits) {
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

fn apply_gif_edits(paths: &[PathBuf], edits: &[metra::GifEdit]) -> ExitCode {
    let mut failures = 0_usize;
    for path in paths {
        match metra::read(path) {
            Ok(metadata) if metadata.file_info.format == metra::FileFormat::Gif => {
                if let Err(error) = metra::rewrite_gif_path(path, ParseLimits::default(), edits) {
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

fn apply_webp_edits(paths: &[PathBuf], edits: &[metra::WebpEdit]) -> ExitCode {
    let mut failures = 0_usize;
    for path in paths {
        match metra::read(path) {
            Ok(metadata) if metadata.file_info.format == metra::FileFormat::Webp => {
                if let Err(error) = metra::rewrite_webp_path(path, ParseLimits::default(), edits) {
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

fn inspect_paths(paths: &[PathBuf], jobs: usize) -> Vec<(PathBuf, metra::Result<Metadata>)> {
    if jobs <= 1 || paths.len() <= 1 {
        return paths
            .iter()
            .map(|path| {
                (
                    path.clone(),
                    metra::read_with_limits(path, ParseLimits::default()),
                )
            })
            .collect();
    }

    let shared_paths = Arc::new(paths.to_vec());
    let next_index = Arc::new(AtomicUsize::new(0));
    let (sender, receiver) = mpsc::channel();
    let worker_count = jobs.min(paths.len());
    let mut slots = (0..paths.len())
        .map(|_| None)
        .collect::<Vec<Option<(PathBuf, metra::Result<Metadata>)>>>();

    thread::scope(|scope| {
        for _ in 0..worker_count {
            let paths = Arc::clone(&shared_paths);
            let next_index = Arc::clone(&next_index);
            let sender = sender.clone();
            scope.spawn(move || {
                loop {
                    let index = next_index.fetch_add(1, Ordering::Relaxed);
                    let Some(path) = paths.get(index).cloned() else {
                        break;
                    };
                    let result = metra::read_with_limits(&path, ParseLimits::default());
                    if sender.send((index, path, result)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(sender);
        for (index, path, result) in receiver {
            slots[index] = Some((path, result));
        }
    });

    slots
        .into_iter()
        .map(|slot| slot.expect("every scheduled path should produce a result"))
        .collect()
}

fn inspect_paths_streaming<F>(paths: &[PathBuf], jobs: usize, validate: bool, mut emit: F) -> usize
where
    F: FnMut(PathBuf, metra::Result<Metadata>),
{
    if jobs <= 1 || paths.len() <= 1 {
        let mut failures = 0;
        for path in paths {
            let result = metra::read_with_limits(path, ParseLimits::default());
            if result_is_failure(&result, validate) {
                failures += 1;
            }
            emit(path.clone(), result);
        }
        return failures;
    }

    let shared_paths = Arc::new(paths.to_vec());
    let next_index = Arc::new(AtomicUsize::new(0));
    let (sender, receiver) = mpsc::channel();
    let worker_count = jobs.min(paths.len());
    let mut pending = BTreeMap::new();
    let mut next_to_emit = 0_usize;
    let mut failures = 0_usize;

    thread::scope(|scope| {
        for _ in 0..worker_count {
            let paths = Arc::clone(&shared_paths);
            let next_index = Arc::clone(&next_index);
            let sender = sender.clone();
            scope.spawn(move || {
                loop {
                    let index = next_index.fetch_add(1, Ordering::Relaxed);
                    let Some(path) = paths.get(index).cloned() else {
                        break;
                    };
                    let result = metra::read_with_limits(&path, ParseLimits::default());
                    if sender.send((index, path, result)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(sender);
        for (index, path, result) in receiver {
            pending.insert(index, (path, result));
            while let Some((path, result)) = pending.remove(&next_to_emit) {
                if result_is_failure(&result, validate) {
                    failures += 1;
                }
                emit(path, result);
                next_to_emit += 1;
            }
        }
    });

    while let Some((path, result)) = pending.remove(&next_to_emit) {
        if result_is_failure(&result, validate) {
            failures += 1;
        }
        emit(path, result);
        next_to_emit += 1;
    }
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
