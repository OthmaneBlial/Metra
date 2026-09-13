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
        let failures = results.iter().filter(|(_, result)| result.is_err()).count();
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
        inspect_paths_streaming(&paths, arguments.jobs, |path, result| {
            if arguments.csv {
                emit_csv_record(&path, &result);
            } else if arguments.jsonl {
                emit_jsonl_record(&path, &result);
            } else if let Ok(metadata) = &result {
                print_human(metadata);
            } else if let Err(error) = &result {
                eprintln!("metra: {}: {error}", path.display());
            }
        })
    };

    if failures == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

enum EditRequest {
    Direct(Vec<metra::JpegEdit>),
    CopyComment { source: PathBuf },
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
            return Err(format!(
                "unsupported writable tag {key}; only JPEG:Comment is currently writable"
            ));
        }
        return Ok(Some(EditRequest::Direct(vec![
            metra::JpegEdit::SetComment(value.to_owned()),
        ])));
    }
    if let Some(key) = delete {
        if key != "JPEG:Comment" {
            return Err(format!(
                "unsupported writable tag {key}; only JPEG:Comment is currently writable"
            ));
        }
        return Ok(Some(EditRequest::Direct(vec![
            metra::JpegEdit::DeleteComments,
        ])));
    }
    if let Some(assignment) = copy {
        let (key, source) = assignment
            .split_once('=')
            .ok_or_else(|| "--copy expects KEY=SOURCE".to_owned())?;
        if key != "JPEG:Comment" {
            return Err(format!(
                "unsupported copied tag {key}; only JPEG:Comment is currently supported"
            ));
        }
        if source.is_empty() {
            return Err("--copy requires a non-empty source path".to_owned());
        }
        return Ok(Some(EditRequest::CopyComment {
            source: PathBuf::from(source),
        }));
    }
    Ok(None)
}

fn apply_request(paths: &[PathBuf], request: EditRequest) -> ExitCode {
    match request {
        EditRequest::Direct(edits) => apply_direct_edits(paths, &edits),
        EditRequest::CopyComment { source } => apply_comment_copy(paths, &source),
    }
}

fn apply_direct_edits(paths: &[PathBuf], edits: &[metra::JpegEdit]) -> ExitCode {
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

fn apply_comment_copy(paths: &[PathBuf], source: &Path) -> ExitCode {
    let source_metadata = match metra::read(source) {
        Ok(metadata) if metadata.file_info.format == metra::FileFormat::Jpeg => metadata,
        Ok(metadata) => {
            eprintln!(
                "metra: {}: source format {} is not JPEG",
                source.display(),
                metadata.file_info.format
            );
            return ExitCode::from(1);
        }
        Err(error) => {
            eprintln!("metra: {}: {error}", source.display());
            return ExitCode::from(1);
        }
    };
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
    apply_direct_edits(paths, &edits)
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

fn inspect_paths_streaming<F>(paths: &[PathBuf], jobs: usize, mut emit: F) -> usize
where
    F: FnMut(PathBuf, metra::Result<Metadata>),
{
    if jobs <= 1 || paths.len() <= 1 {
        let mut failures = 0;
        for path in paths {
            let result = metra::read_with_limits(path, ParseLimits::default());
            if result.is_err() {
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
                if result.is_err() {
                    failures += 1;
                }
                emit(path, result);
                next_to_emit += 1;
            }
        }
    });

    while let Some((path, result)) = pending.remove(&next_to_emit) {
        if result.is_err() {
            failures += 1;
        }
        emit(path, result);
        next_to_emit += 1;
    }
    failures
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
