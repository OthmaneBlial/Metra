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

    /// Traverse directories recursively in deterministic path order.
    #[arg(short = 'r', long)]
    recursive: bool,

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

    let mut results = Vec::with_capacity(paths.len());
    for path in paths {
        results.push((
            path.clone(),
            metra::read_with_limits(&path, ParseLimits::default()),
        ));
    }

    let failures = results.iter().filter(|(_, result)| result.is_err()).count();
    if arguments.json || arguments.jsonl {
        emit_json(&results, arguments.jsonl);
    } else {
        for (_, result) in &results {
            if let Ok(metadata) = result {
                print_human(metadata);
            }
        }
        for (path, result) in &results {
            if let Err(error) = result {
                eprintln!("metra: {}: {error}", path.display());
            }
        }
    }

    if failures == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
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
        let metadata = entry
            .metadata()
            .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
        if metadata.is_dir() {
            collect_directory(&path, output)?;
        } else if metadata.is_file() {
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
